import io
import random
import subprocess
import sys

import pytest
import rkdl
from conftest import run_threads

SOURCE = """\
// a header comment
server "web" port=0x1F90 {
    /* block comment */ path #"C:\\raw\\path"#
    /-disabled 1
    mode 0o755 0b1010 // trailing comment
}
/-gone {
    child
}
empty {}
"""


def test_round_trip_is_byte_for_byte():
    assert str(rkdl.parse(SOURCE)) == SOURCE


def test_parser_parses_like_parse():
    assert str(rkdl.Parser().parse(SOURCE)) == SOURCE


def test_load_dump_round_trip():
    doc = rkdl.load(io.StringIO(SOURCE))
    out = io.StringIO()
    rkdl.dump(doc, out)
    assert out.getvalue() == SOURCE


def test_version():
    assert isinstance(rkdl.__version__, str)


# rkdl refuses deeper documents before kdl-rs's parser, which recurses per level, can run out of stack.
MAX_DEPTH = 1000


def nested(depth):
    return "a{" * depth + "}" * depth


def parse_isolated(text):
    """Parses `text` in a fresh interpreter, so a crash fails the test instead of killing pytest. Returns the
    ParseError's first message, line and column, or None if the text parsed."""
    code = """
import sys, rkdl
try:
    rkdl.parse(sys.stdin.read())
except rkdl.ParseError as e:
    print(e.diagnostics[0].message, e.line, e.column, sep="|")
"""
    result = subprocess.run(
        [sys.executable, "-c", code], input=text, capture_output=True, text=True, timeout=120, check=False
    )
    assert result.returncode == 0, f"exit code {result.returncode}: {result.stderr[-1000:]}"
    out = result.stdout.strip()
    if not out:
        return None
    message, line, column = out.split("|")
    return message, int(line), int(column)


def test_nesting_up_to_the_limit_parses():
    text = nested(MAX_DEPTH)
    doc = rkdl.parse(text)
    assert str(doc) == text
    doc.fmt()


@pytest.mark.parametrize("text", [nested(MAX_DEPTH + 1), "a{" * (MAX_DEPTH + 1), nested(100_000)])
def test_nesting_past_the_limit_is_a_parse_error(text):
    # The first `{` past the limit is at column 2 * MAX_DEPTH + 2.
    assert parse_isolated(text) == ("Document nests too deeply", 1, 2 * MAX_DEPTH + 2)


@pytest.mark.parametrize(
    "text",
    [
        # kdl-rs recurses per piece of a block comment, and per stray character it skips after some errors.
        "/*" + "*" * 1_000_000 + "*/",
        "/*" * 100_000,
        ")" * 1_000_000,
        "a\n" + "}" * 1_000_000,
        "a =1\n" * 200_000,
        # Error recovery inside a bad string reads the rest of it as structure.
        '"\\q {' * 100_000,
    ],
)
def test_text_that_would_overflow_kdl_rs_is_a_parse_error(text):
    assert parse_isolated(text) is not None


def test_braces_in_strings_and_comments_dont_count():
    braces = "{" * 10 * MAX_DEPTH
    text = f'a "{braces}" #"{braces}"# /* {braces} */ // {braces}\n'
    assert str(rkdl.parse(text)) == text


def test_parse_on_threads():
    text = nested(MAX_DEPTH)
    assert run_threads(*[lambda: str(rkdl.parse(text))] * 4) == [text] * 4


# kdl-rs drops the text between a block's `}` and the end of its line (notes/upstream.md); rkdl puts it back.

AFTER_BLOCK = ["", " ", "\t", " \t ", " /* c */", "/* c */ ", " /* } */\t", " /-{ x }", " /-{} /-{ y {} } "]
LINE_COMMENTS = [" // c", "\t// c }", "// c"]


def generated_block(rng, depth):
    """Nodes for one block, with spaces, tabs and comments after every `}` and every node."""
    out = []
    count = rng.randint(1, 3)
    for i in range(count):
        line = rng.choice(["a", "b 1", "(t)c k=v", '"q" #true'])
        if depth < 4 and rng.random() < 0.7:
            inner = generated_block(rng, depth + 1) if rng.random() < 0.8 else ""
            line += rng.choice([" {", "{"]) + rng.choice(["", " ", "\n"]) + inner + rng.choice(["", " "]) + "}"
        if rng.random() < 0.3:
            line += rng.choice(LINE_COMMENTS) + rng.choice(["\n", "\r\n"])
        else:
            line += rng.choice(AFTER_BLOCK)
            last = i == count - 1
            line += rng.choice(["\n", "\r\n", ";", ""] if last else ["\n", "\r\n", ";"])
        out.append(line)
    return "".join(out)


def test_text_after_a_closing_brace_reads_back_at_every_level():
    rng = random.Random(31)
    for _ in range(3000):
        text = generated_block(rng, 0) + rng.choice(["", "\n", " ", "// end"])
        assert str(rkdl.parse(text)) == text


@pytest.mark.parametrize(
    "text",
    ["a {}  \nb", "a {} // c\nb", "pkg {\n    a {\n    } // end a\n} // end pkg\n", "a { b { c {} } }", "a {} ;b"],
)
def test_text_after_a_closing_brace_survives(text):
    doc = rkdl.parse(text)
    assert str(doc) == text
    # Printing and parsing again changes nothing, however many times.
    assert str(rkdl.parse(str(rkdl.parse(str(doc))))) == text


def test_text_after_a_closing_brace_survives_edits():
    doc = rkdl.parse("a {} // c\nb { x {}  \n}\n")
    doc["b"].children.append(rkdl.Node("y"))
    doc["b"]["x"].args.append(1)
    assert str(doc) == "a {} // c\nb { x 1 {}  \n    y\n}\n"


def test_text_after_a_closing_brace_survives_an_escape_starting_a_string_line():
    text = 'a {\n  b\n} // after\nc """\n  \\  x\n  """\n'
    doc = rkdl.parse(text)
    assert str(doc) == text
    assert doc["c"].args[0] == "x"


# A `\` and the spaces and line breaks after it are left out of a quoted string. In a multi-line string that happens
# before the lines are dedented. In a raw string `\` is just text.
ESCAPED_WHITESPACE = [
    ('"x\\  y"', "xy"),
    ('"\\  x"', "x"),
    ('"x\\\t "', "x"),
    ('"x\\\n    y"', "xy"),
    ('"x\\\r\n\r\n y"', "xy"),
    ('"""\n    \\  x\n    """', "x"),
    ('"""\n    \\\n  x\n    """', "x"),
    ('"""\n    x\\   y\n    """', "xy"),
    ('"""\n    x\\ \n    y\n    """', "xy"),
    ('"""\n    x \\\n\n    y\n    z\n    """', "x y\nz"),
    ('"""\n    x\n    \\  """', "x"),
    ('"""\n    x\n  \\\n    """', "  x"),
    ('#"x\\  y"#', "x\\  y"),
    ('#"""\n    \\  x\n    """#', "\\  x"),
]


@pytest.mark.parametrize(("string", "value"), ESCAPED_WHITESPACE)
def test_escaped_whitespace_round_trips(string, value):
    text = (
        "outer {\n"
        "    inner {\n"
        f"        deep {string} {{ leaf }}  // after deep\n"
        "    } /* after inner */ ;\n"
        f"    s {string}\n"
        "}  // after outer\n"
        f"tail k={string}\n"
    )
    doc = rkdl.parse(text)
    assert str(doc) == text
    assert doc["outer"]["inner"]["deep"].args[0] == value
    assert doc["outer"]["s"].args[0] == value
    assert doc["tail"].props["k"] == value
    assert str(rkdl.parse(str(doc))) == text


@pytest.mark.parametrize(
    "string",
    [
        # The escape takes the line break before the closing quotes, so they end a line with `x` on it.
        '"""\n    x\\\n    """',
        # Without the escape, `x` doesn't start with the closing quotes' four spaces.
        '"""\n\\    x\n    """',
        '"""\n  \\\n    x\n    """',
    ],
)
def test_escaped_whitespace_counts_before_dedenting(string):
    with pytest.raises(rkdl.ParseError):
        rkdl.parse(f"a {{}} // c\nb {string}\n")


# kdl-rs reads what's inside a slashdashed block again for every level of them around it, doubling its time per
# level (notes/upstream.md). rkdl refuses text it would spend too long on.

DASHED_SHAPES = [
    lambda n: "a /-{" * n + "}" * n,
    lambda n: "a /-{" * n,
    lambda n: "x;/-a{" * n + "}" * n,
    lambda n: "a{x;/-b " * n + "}" * n,
    lambda n: "/- a { /-{ b {" * n + "} } }" * n,
]


@pytest.mark.parametrize("shape", DASHED_SHAPES)
def test_nested_slashdashed_blocks_past_the_limit_are_a_parse_error(shape):
    message, _, _ = parse_isolated(shape(40))
    assert message == "Slashdashed blocks nest too deeply"


def test_slashdashed_block_refusal_points_at_the_slashdash():
    text = "a /-{" * 40 + "}" * 40
    message, line, column = parse_isolated(text)
    assert (message, line) == ("Slashdashed blocks nest too deeply", 1)
    assert text[column - 1 : column + 1] == "/-"


def test_text_inside_nested_slashdashed_blocks_counts():
    content = "b 1 2\n" * 5_000
    # One level is fine at any size: it only doubles the time.
    assert parse_isolated("a /-{\n" + content + "}\n") is None
    message, _, _ = parse_isolated("a /-{ c /-{ d /-{\n" + content + "} } }\n")
    assert message == "Slashdashed blocks nest too deeply"


def test_slashdashes_after_an_error_count():
    # After an error the scan can't follow kdl-rs's reading, so every `/-` counts as one more level.
    text = ") " + "a /-{" * 40 + "}" * 40
    message, line, column = parse_isolated(text)
    assert (message, line, column) == ("Unexpected `)`: no type annotation is open", 1, 1)


# kdl-rs's printing, cloning and formatting recurse once per level too, so for deep documents rkdl runs them on a
# thread with a big stack. A thread's stack can be small: 512 KB is macOS's default for threads other than the main
# one.


@pytest.mark.parametrize(
    "op",
    [
        "copy.copy(doc)",
        "copy.deepcopy(doc)",
        "str(doc)",
        "doc.fmt()",
        "str(doc.children[0])",
        "doc.children[0].copy()",
        "del doc",
        "rkdl.Node('x', children=[doc.children[0].detach()])",
        "e = rkdl.parse('p { q }'); e['p'].children.append(doc.children[0].detach()); str(e)",
    ],
)
def test_deep_documents_work_on_a_small_stack(op):
    code = f"""
import copy, threading, rkdl
threading.stack_size(512 * 1024)
def run():
    global doc
    doc = rkdl.parse("a {{\\n" * {MAX_DEPTH - 1} + "}}\\n" * {MAX_DEPTH - 1})
    {op}
    print("ok")
thread = threading.Thread(target=run)
thread.start()
thread.join()
"""
    result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=120, check=False)
    assert (result.returncode, result.stdout) == (0, "ok\n"), result.stderr[-1000:]


def deepest(doc):
    return list(doc.iter())[-1]


def test_building_deeper_than_parse_reads_raises():
    doc = rkdl.parse(nested(MAX_DEPTH))
    bottom = deepest(doc)
    # A node without a block still fits at the bottom.
    bottom.children.append(rkdl.Node("leaf"))
    leaf = bottom["leaf"]
    text = str(doc)
    with pytest.raises(ValueError, match="more than 1000 levels"):
        leaf.children = []
    with pytest.raises(ValueError, match="more than 1000 levels"):
        bottom.children.append(rkdl.Node("x", children=[]))
    assert str(doc) == text
    assert str(rkdl.parse(text)) == text


def test_moving_a_parsed_chunk_deeper_than_parse_reads_raises():
    doc = rkdl.parse(nested(600))
    chunk = rkdl.parse(nested(400)).children[0].detach()
    deepest(doc).children.append(chunk)
    text = str(doc)
    assert str(rkdl.parse(text)) == text
    more = rkdl.parse(nested(1)).children[0].detach()
    new = rkdl.Node("new")
    # All or nothing: neither node goes in.
    with pytest.raises(ValueError, match="more than 1000 levels"):
        deepest(doc).children.extend([new, more])
    assert new.parent is None and more.parent is None
    assert str(doc) == text
    # Where there's room, it goes in.
    doc.children[0].children.append(more)
    assert more.parent == doc.children[0]
