"""Layout: where line breaks and indentation go when nodes move in and out of blocks.

The rule is in src/layout.rs. Most cases here are the reproductions from the hardening sweep's rewrite diff.
"""

import pytest
import rkdl
from conftest import is_valid_case, read_case, spec_inputs, walk

N = rkdl.Node


def reads_back(text):
    assert str(rkdl.parse(text)) == text


# Every line break belongs to the thing before it


def test_a_node_without_a_final_newline_gets_one_when_a_node_follows():
    d = rkdl.parse("a\nb")
    d.children.append(N("c"))
    assert str(d) == "a\nb\nc\n"
    # So taking b out leaves no blank line.
    d.children.remove(d["b"])
    assert str(d) == "a\nc\n"


def test_replacing_a_node_after_an_unterminated_one():
    d = rkdl.parse("a\nb")
    d.children.append(N("c"))
    d.children[1] = N("z")
    assert str(d) == "a\nz\nc\n"


def test_popping_the_first_after_appending_to_a_one_node_file():
    d = rkdl.parse("a")
    d.children.append(N("b"))
    assert str(d) == "a\nb\n"
    d.children.pop(0)
    assert str(d) == "b\n"


@pytest.mark.parametrize(
    ("edit", "expected"),
    [
        (lambda d: d.children.__setitem__(0, N("c")), "c\nb\n"),
        (lambda d: d.children.insert(0, d.children.pop(1)), "b\na\n"),
        (lambda d: d.children.reverse(), "b\na\n"),
    ],
)
def test_edits_after_appending_to_a_one_node_file(edit, expected):
    d = rkdl.parse("a")
    d.children.append(N("b"))
    edit(d)
    assert str(d) == expected


def test_moving_out_the_node_that_had_no_newline():
    d = rkdl.parse("a\nb")
    d.children.append(N("c"))
    other = rkdl.parse("q\n")
    other.children.append(d.children.pop(1))
    assert str(d) == "a\nc\n"
    assert str(other) == "q\nb\n"


def test_unterminated_last_child():
    d = rkdl.parse("a {\n    b\n    c}\n")
    d["a"].children.append(N("x"))
    assert str(d) == "a {\n    b\n    c\n    x\n}\n"
    d["a"].children.pop(1)
    assert str(d) == "a {\n    b\n    x\n}\n"


def test_closing_brace_on_the_last_childs_line():
    d = rkdl.parse("a {\n    b }\n")
    d["a"].children.append(N("x"))
    # No space left at the end of b's line, and `}` lines up with its parent.
    assert str(d) == "a {\n    b\n    x\n}\n"
    d["a"].children.append(N("y"))
    assert str(d) == "a {\n    b\n    x\n    y\n}\n"
    d["a"].children.pop(0)
    assert str(d) == "a {\n    x\n    y\n}\n"


def test_closing_brace_lines_up_with_an_indented_parent():
    d = rkdl.parse("  p {\n    a }\n")
    d["p"].children.append(N("x"))
    assert str(d) == "  p {\n    a\n    x\n  }\n"


def test_a_block_on_a_node_that_used_to_be_last():
    d = rkdl.parse("a {\n    b }\n")
    d["a"].children.append(N("x"))
    d["a"]["b"].children = []
    # b isn't last any more, so no `;` after its block.
    assert str(d) == "a {\n    b {}\n    x\n}\n"


def test_a_block_on_a_node_ending_in_a_continuation_at_the_end_of_the_file():
    # The continuation's line ending was the last thing in the file. The block goes in front of it, so the line
    # ending goes after the block.
    d = rkdl.parse("x 1 \\\n")
    d["x"].children = [N("y")]
    assert str(d) == "x 1 \\\n{\n    y\n}\n"
    reads_back(str(d))


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a;\nb; c\n", "a;\nc\n"),
        ("p {\n    a;\n    b; c\n}\n", "p {\n    a;\n    c\n}\n"),
    ],
)
def test_taking_out_the_first_of_a_shared_line_after_a_semicolon(text, expected):
    # a keeps its line ending, and c, now first on its line, takes over b's start.
    d = rkdl.parse(text)
    view = d["p"].children if text.startswith("p") else d.children
    view.pop(1)
    assert str(d) == expected


def test_a_comment_at_the_end_of_an_emptied_block_stays_at_the_end():
    d = rkdl.parse("a {\n    b\n    // end\n}\n")
    d["a"].children.pop()
    assert str(d) == "a {\n    // end\n}\n"
    d["a"].children.append(N("x"))
    assert str(d) == "a {\n    x\n    // end\n}\n"


def test_taking_the_last_node_off_a_shared_line():
    d = rkdl.parse("a {\n    b\n    c }\n")
    d["a"].children.pop()
    assert str(d) == "a {\n    b\n}\n"


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a {\n    b; c\n}\n", "a {\n    c\n}\n"),
        ("a; b\n", "b\n"),
        ("a {b;c;}\n", "a {c;}\n"),
    ],
)
def test_the_node_that_becomes_first_on_a_line_takes_its_start(text, expected):
    d = rkdl.parse(text)
    (d["a"] if text.startswith("a {") else d).children.pop(0)
    assert str(d) == expected


def test_popping_the_second_of_a_shared_line_keeps_the_rest_on_lines():
    d = rkdl.parse("a; b\nc\n")
    d.children.pop(1)
    assert str(d) == "a;\nc\n"


def test_a_continuation_closed_with_a_semicolon_moves_whole():
    d = rkdl.parse("node \\")
    d.children.append(N("x"))
    assert str(d) == "node \\\n;\nx\n"
    node = d.children.pop(0)
    empty = rkdl.parse("")
    empty.children.append(node)
    assert str(empty) == "node \\\n;\n"
    reads_back(str(empty))


# Line ending style lives on the document


@pytest.mark.parametrize(
    "text",
    # A document of spaces: its start starts a line, and spaces don't end it, so the node goes on that line.
    ["\n", "\n\n", "  \n", "\ufeff\n", "// c\n", "\r\n", "\r\n\r\n", "\ufeff\r\n", "   ", "\t", "\ufeff  "],
)
def test_appending_keeps_the_users_text(text):
    d = rkdl.parse(text)
    d.children.append(N("x"))
    eol = "\r\n" if "\r\n" in text else "\n"
    assert str(d) == text + "x" + eol


def test_insert_then_pop_gives_back_the_text():
    d = rkdl.parse("\n")
    d.children.insert(0, N("x"))
    d.children.pop()
    assert str(d) == "\n"


def test_an_emptied_crlf_document():
    d = rkdl.parse("a\r\n")
    d.children.pop()
    assert str(d) == ""
    d.children.append(N("x"))
    assert str(d) == "x\r\n"
    d.children.pop()
    d.children.append(N("y"))
    assert str(d) == "y\r\n"


@pytest.mark.parametrize("how", ["pop", "clear"])
def test_an_emptied_crlf_document_extends_with_crlf(how):
    d = rkdl.parse("a\r\nb\r\n")
    if how == "pop":
        d.children.pop()
        d.children.pop()
    else:
        d.children.clear()
    d.children.extend([N("x"), N("y")])
    assert str(d) == "x\r\ny\r\n"


def test_an_emptied_crlf_document_restyles_what_moves_in():
    d = rkdl.parse("a\r\n")
    d.children.pop()
    d.children.append(rkdl.parse("// c\nx {\n    y\n}\n").children.pop())
    assert str(d) == "// c\r\nx {\r\n    y\r\n}\r\n"


def test_an_emptied_crlf_document_nests_with_crlf():
    d = rkdl.parse("a\r\n")
    d.children.pop()
    d.children.append(N("x"))
    d["x"].children = []
    d["x"].children.append(N("y"))
    assert str(d) == "x {\r\n    y\r\n}\r\n"


def test_closing_the_only_crlf_block_keeps_crlf():
    d = rkdl.parse("a {\r\n    b\r\n}")
    d["a"].children = None
    assert str(d) == "a"
    d.children.append(N("x"))
    assert str(d) == "a\r\nx\r\n"


def test_popping_the_last_child_of_the_last_crlf_block_keeps_crlf():
    d = rkdl.parse("a {\r\n    b\r\n}")
    d["a"].children.pop()
    assert str(d) == "a {}"
    d["a"].children.append(N("x"))
    assert str(d) == "a {\r\n    x\r\n}"


def test_a_crlf_document_gets_its_node_back_in_crlf():
    a = rkdl.parse("p\r\n")
    b = rkdl.parse("q\n")
    b.children.append(a.children.pop())
    assert str(a) == ""
    assert str(b) == "q\np\n"
    a.children.append(b.children.pop(0))
    assert str(a) == "q\r\n"


def test_a_detached_node_keeps_the_style_of_its_document():
    popped = rkdl.parse("a {\r\n}\r\n").children.pop()
    popped.children.append(N("y"))
    assert str(popped) == "a {\r\n    y\r\n}"
    copied = rkdl.parse("a {}\r\n")["a"].copy()
    copied.children.append(N("y"))
    assert str(copied) == "a {\r\n    y\r\n}"


@pytest.mark.parametrize("eol", ["\r", "\x85", "\x0b", "\x0c", "\u2028", "\u2029"])
def test_every_kdl_line_ending_is_a_style(eol):
    d = rkdl.parse(f"a{eol}b")
    d.children.append(N("x"))
    d["b"].children = []
    d["b"].children.append(N("y"))
    assert str(d) == f"a{eol}b {{{eol}    y{eol}}}{eol}x{eol}"
    d = rkdl.parse(f"a{eol}")
    d.children.pop()
    d.children.append(N("x"))
    assert str(d) == f"x{eol}"


@pytest.mark.parametrize("eol", ["\r", "\u2028", "\x0c"])
def test_nested_blocks_with_other_line_endings(eol):
    d = rkdl.parse(f"a {{{eol}    b{eol}}}{eol}")
    d["a"].children.append(rkdl.parse("x {\n    y\n}").children.pop())
    assert str(d) == f"a {{{eol}    b{eol}    x {{{eol}        y{eol}    }}{eol}}}{eol}"


# Moved subtrees take the destination's indentation


def test_a_tab_indented_subtree_moves_into_spaces():
    src = rkdl.parse('x {\n\t// about grand\n\tgrand {\n\t\tdeep """\n\t\t\tstr\n\t\t\t"""\n\t}\n}\n')
    d = rkdl.parse("top {\n    a\n}\n")
    d["top"].children.append(src.children.pop())
    # A string's own lines stay as they were.
    assert str(d) == (
        'top {\n    a\n    x {\n        // about grand\n        grand {\n            deep """\n\t\t\tstr\n\t\t\t"""\n'
        "        }\n    }\n}\n"
    )
    assert rkdl.parse(str(d))["top"]["x"]["grand"]["deep"].args[0] == "str"


def test_a_two_space_subtree_moves_into_tabs():
    src = rkdl.parse("x {\n  grand {\n    deep\n  }\n}\n")
    d = rkdl.parse("a {\n\tb\n}\n")
    d["a"].children.append(src.children.pop())
    assert str(d) == "a {\n\tb\n\tx {\n\t\tgrand {\n\t\t\tdeep\n\t\t}\n\t}\n}\n"


# Smaller layout items


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("\ta {\n\t\tb\n\t}\n", "\ta {\n\t\tx\n\t}\n"),
        ("  a {\n    b\n  }\n", "  a {\n    x\n  }\n"),
        ("a {\n  b {\n    c\n  }\n}\n", "a {\n  x\n}\n"),
        ("a {\n\n    b\n\n}\n", "a {\n\n    x\n\n}\n"),
        ("a {\n    b\n  }\n", "a {\n    x\n  }\n"),
    ],
)
def test_replacing_the_only_child_keeps_its_place(text, expected):
    d = rkdl.parse(text)
    d["a"].children[0] = N("x")
    assert str(d) == expected


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("\ta {\n\t\tb\n\t}\nc {}\n", "c {\n\tx\n}\n"),
        ("  a {\n    b\n  }\nc {}\n", "c {\n  x\n}\n"),
    ],
)
def test_indent_unit_is_relative_to_the_parent(text, expected):
    d = rkdl.parse(text)
    d["c"].children.append(N("x"))
    assert str(d).endswith(expected)


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a; b; c\n", "c; b; a\n"),
        ("a {\n    b\n\n    c\n}\n", "a {\n    c\n\n    b\n}\n"),
        ("a {\n    // c\n    b\n    d\n}\n", "a {\n    d\n    // c\n    b\n}\n"),
        ("a // about a\nb\n", "b\na // about a\n"),
    ],
)
def test_reverse_keeps_the_layout_by_position(text, expected):
    d = rkdl.parse(text)
    view = d["a"].children if text.startswith("a {") else d.children
    view.reverse()
    assert str(d) == expected
    view.reverse()
    assert str(d) == text


def test_reverse_where_a_comment_would_lose_its_line_end():
    text = "x; y\nz // c\n"
    d = rkdl.parse(text)
    d.children.reverse()
    assert [n.name for n in d.children] == ["z", "y", "x"]
    reads_back(str(d))


def unedited_reads_back(doc):
    try:
        return str(rkdl.parse(str(doc))) == str(doc)
    except rkdl.ParseError:
        return False


def corpus_blocks():
    for path in spec_inputs():
        text = read_case(path)
        if text is None or not is_valid_case(path):
            continue
        for crlf in (False, True):
            text = text.replace("\n", "\r\n") if crlf else text
            doc = rkdl.parse(text)
            # Files that don't read back as themselves unedited hit known kdl-rs issues; skip them.
            if not unedited_reads_back(doc):
                continue
            for where, node in [((), doc), *walk(doc)]:
                if node.children is not None and len(node.children) > 1:
                    yield path.name, crlf, where


@pytest.mark.parametrize(("name", "crlf", "where"), list(corpus_blocks()))
def test_reversing_twice_gives_back_the_text(name, crlf, where):
    path = next(p for p in spec_inputs() if p.name == name)
    text = read_case(path)
    doc = rkdl.parse(text.replace("\n", "\r\n") if crlf else text)
    before = str(doc)
    view = doc.children
    for i in where:
        view = view[i].children
    names = [n.name for n in view]
    view.reverse()
    once = str(doc)
    reads_back(once)
    assert [n.name for n in view] == names[::-1]
    view.reverse()
    assert str(doc) == before


# A block written on one line stays on one line while what goes in fits on one line. Otherwise it spreads over lines.


@pytest.mark.parametrize(
    ("source", "moved"),
    [
        # A multi-line string argument. Strings keep their own lines.
        ('x """\n  hi\n  """\n', 'x """\n  hi\n  """'),
        # A slashdashed block after the entries, from a block at the depth it lands at.
        ("p {\n    x 1 /-{\n        y\n    }\n}\n", "x 1 /-{\n        y\n    }"),
    ],
)
def test_a_node_written_over_lines_spreads_a_one_line_block(source, moved):
    src = rkdl.parse(source)
    x = (src["p"] if source.startswith("p") else src).children.pop()
    d = rkdl.parse("a { b }\n")
    d["a"].children.append(x)
    assert str(d) == f"a {{\n    b\n    {moved}\n}}\n"
    reads_back(str(d))


def test_a_node_ending_in_a_semicolon_fits_a_one_line_block():
    x = rkdl.parse("x 1;").children.pop()
    d = rkdl.parse("a { b }\n")
    d["a"].children.append(x)
    assert str(d) == "a { b; x 1 }\n"


def test_a_node_ending_in_a_continuation_spreads_a_one_line_block():
    # Nothing can follow its `\` on the line, so it can't go before the `}`. Where the closing `;` goes isn't pinned:
    # task 63 indents it, and then this can check the exact text.
    x = rkdl.parse("n 1 \\").children.pop()
    d = rkdl.parse("a { b }\n")
    d["a"].children.append(x)
    out = str(d)
    assert out.startswith("a {\n    b\n    n 1 \\\n")
    assert out.endswith("\n}\n")
    assert [n.name for n in rkdl.parse(out)["a"].children] == ["b", "n"]


def test_a_block_with_a_multi_line_string_is_not_one_line():
    d = rkdl.parse('a { b """\n  hi\n  """ }\n')
    d["a"].children.append(N("x"))
    assert str(d) == 'a { b """\n  hi\n  """\n    x\n}\n'


@pytest.mark.parametrize(
    ("index", "expected"),
    [
        (0, "a {\n    x {\n        y\n    }\n    c\n}\n"),
        (1, "a {\n    b\n    x {\n        y\n    }\n}\n"),
    ],
)
def test_a_replacement_that_does_not_fit_spreads_a_one_line_block(index, expected):
    d = rkdl.parse("a { b; c }\n")
    d["a"].children[index] = N("x", children=[N("y")])
    assert str(d) == expected


# Reordering by assignment: nodes from the replaced range may go back in, moved, as with a list. Each place keeps its
# whitespace, as with reverse().


def test_sorting_a_one_line_block_keeps_it_on_one_line():
    d = rkdl.parse("a { z; b; m }\n")
    a = d["a"]
    z = a["z"]
    a.children = sorted(a.children, key=lambda n: n.name)
    assert str(d) == "a { b; m; z }\n"
    # The nodes moved: the handles still point at them.
    assert a.children[2] is z
    assert z.parent == a


def test_sorting_keeps_each_place_s_whitespace_and_each_node_s_comments():
    d = rkdl.parse("z 1\n// about b\nb 2 // end b\n\n    m 3\n")
    d.children = sorted(d.children, key=lambda n: n.name)
    assert str(d) == "// about b\nb 2 // end b\nm 3\n\n    z 1\n"
    reads_back(str(d))


def test_sorting_where_a_comment_would_lose_its_line_end():
    # b would take a's place, `a;`, which c follows on the same line. A `//` comment runs to the end of its line, so
    # there it would comment out c: b has to go in another way.
    d = rkdl.parse("b // about b\na; c\n")
    b = d["b"]
    d.children[:] = sorted(d.children, key=lambda n: n.name)
    assert [n.name for n in d.children] == ["a", "b", "c"]
    assert d.children[1] is b
    assert "b // about b\n" in str(d)
    assert [n.name for n in rkdl.parse(str(d)).children] == ["a", "b", "c"]
    reads_back(str(d))


def test_sorting_nested_blocks():
    d = rkdl.parse("pkg {\n    z {\n        x\n    }\n    b {}\n}\n")
    pkg = d["pkg"]
    pkg.children = sorted(pkg.children, key=lambda n: n.name)
    assert str(d) == "pkg {\n    b {}\n    z {\n        x\n    }\n}\n"


@pytest.mark.parametrize(
    "assign",
    [
        lambda c: c.__setitem__(slice(None), list(c)),
        lambda c: c.__setitem__(0, c[0]),
        lambda c: c.__setitem__(slice(1, None), list(c)[1:]),
        lambda c: c.__setitem__(slice(None, None, -1), list(c)[::-1]),
    ],
)
def test_assigning_nodes_back_to_their_own_places_changes_nothing(assign):
    text = "a 1 // one\n/* two */ b {\n    c\n}\nd; e\n"
    d = rkdl.parse(text)
    assign(d.children)
    assert str(d) == text


def test_assignment_can_mix_moved_new_and_dropped_nodes():
    d = rkdl.parse("a\nb\nc\n")
    a, b, c = d.children
    d.children[0:2] = [rkdl.Node("new"), b]
    assert str(d) == "new\nb\nc\n"
    assert a.parent is None and str(a) == "a"
    d.children[::2] = [c, d["new"]]
    assert str(d) == "c\nb\nnew\n"
    d.children = [b]
    assert str(d) == "b\n"
    assert c.parent is None


def test_nodes_from_outside_the_replaced_range_still_raise():
    d = rkdl.parse("p {\n    q\n}\nr\ns\n")
    other = rkdl.parse("x\n")
    with pytest.raises(ValueError, match="already in a document"):
        d.children[0] = d.children[1]
    with pytest.raises(ValueError, match="already in a document"):
        d["p"].children = [d["p"]["q"], d["r"]]
    with pytest.raises(ValueError, match="already in a document"):
        d.children[1:] = [d["s"], other["x"]]
    with pytest.raises(ValueError, match="already in a document"):
        d["p"].children[:] = [d["p"]]
    # All or nothing.
    assert str(d) == "p {\n    q\n}\nr\ns\n"


# Comments above a node are its own up to the last blank line. Above that, they stay where they are.

HEADED = "// header\n\n// about a\na\n\nb\n"


def test_the_header_stays_when_the_first_node_is_removed():
    for remove in (
        lambda d: d.children.pop(0),
        lambda d: d.children.remove(d["a"]),
        lambda d: d.children.__delitem__(0),
        lambda d: d.children.__delitem__(slice(0, 1)),
        lambda d: d["a"].detach(),
    ):
        d = rkdl.parse(HEADED)
        remove(d)
        assert str(d) == "// header\n\nb\n"


def test_the_header_stays_when_every_node_goes():
    d = rkdl.parse(HEADED)
    d.children.clear()
    assert str(d) == "// header\n"
    d.children.append(N("x"))
    assert str(d) == "// header\nx\n"
    d = rkdl.parse(HEADED)
    d.children = [N("x")]
    assert str(d) == "// header\n\nx\n"


def test_comments_right_above_go_with_the_node():
    d = rkdl.parse(HEADED)
    a = d.children.pop(0)
    other = rkdl.parse("z\n")
    other.children.append(a)
    assert str(other) == "z\n// about a\na\n"


def test_moving_the_first_node_leaves_the_header():
    d = rkdl.parse(HEADED)
    d.children.append(d["a"].detach())
    assert str(d) == "// header\n\nb\n// about a\na\n"
    d = rkdl.parse(HEADED)
    other = rkdl.parse("z\n")
    other.children.insert(0, d.children.pop(0))
    assert str(d) == "// header\n\nb\n"
    assert str(other) == "// about a\na\nz\n"


def test_a_comment_a_blank_line_separates_stays():
    d = rkdl.parse("a\n\n// about the rest\n\nb\nc\n")
    d.children.remove(d["b"])
    assert str(d) == "a\n\n// about the rest\n\nc\n"
    d.children.remove(d["c"])
    assert str(d) == "a\n\n// about the rest\n"


def test_removing_between_two_blank_lines_leaves_one():
    d = rkdl.parse("a\n\nb\n\nc\n")
    d.children.remove(d["b"])
    assert str(d) == "a\n\nc\n"
    # At the end, no blank line is left over.
    d.children.remove(d["c"])
    assert str(d) == "a\n"


def test_removing_the_last_node_keeps_the_blank_line_above_a_closing_comment():
    d = rkdl.parse("a\n\nb\n// end\n")
    d.children.remove(d["b"])
    assert str(d) == "a\n\n// end\n"


def test_insert_before_the_first_goes_under_the_header():
    d = rkdl.parse(HEADED)
    d.children.insert(0, N("x"))
    assert str(d) == "// header\n\nx\n// about a\na\n\nb\n"
    # Without a blank line, the comment is the first node's, and the new node goes above it.
    d = rkdl.parse("// about a\na\n")
    d.children.insert(0, N("x"))
    assert str(d) == "x\n// about a\na\n"


def test_replacing_the_first_node_keeps_the_header():
    d = rkdl.parse(HEADED)
    d.children[0] = N("x")
    assert str(d) == "// header\n\nx\n\nb\n"


def test_sort_and_reverse_keep_the_header_at_the_top():
    d = rkdl.parse("// header\n\n// about c\nc\nb\na\n")
    d.children[:] = sorted(d.children, key=lambda n: n.name)
    assert str(d) == "// header\n\na\nb\n// about c\nc\n"
    d.children.reverse()
    assert str(d) == "// header\n\n// about c\nc\nb\na\n"


def test_a_copy_leaves_the_gap():
    d = rkdl.parse(HEADED)
    other = rkdl.parse("z\n")
    other.children.append(d["a"].copy())
    assert str(other) == "z\n// about a\na\n"
    assert str(d) == HEADED


def test_gaps_in_crlf_files():
    d = rkdl.parse(HEADED.replace("\n", "\r\n"))
    d.children.pop(0)
    assert str(d) == "// header\r\n\r\nb\r\n"
    d.children.insert(0, N("x"))
    assert str(d) == "// header\r\n\r\nx\r\nb\r\n"


def test_gaps_in_nested_blocks():
    d = rkdl.parse("p {\n    // about the block\n\n    // about a\n    a\n\n    b\n}\n")
    d["p"].children.pop(0)
    assert str(d) == "p {\n    // about the block\n\n    b\n}\n"
    d["p"].children.insert(0, N("x"))
    assert str(d) == "p {\n    // about the block\n\n    x\n    b\n}\n"
    d["p"].children.clear()
    assert str(d) == "p {\n    // about the block\n}\n"


def test_slashdashed_nodes_count_as_comments():
    d = rkdl.parse("/- old 1\n\n/- older 2\na\nb\n")
    a = d.children.pop(0)
    assert str(d) == "/- old 1\n\nb\n"
    other = rkdl.parse("")
    other.children.append(a)
    assert str(other) == "/- older 2\na\n"


# A comment is one piece, blank lines inside it included. The whole of it is in the gap or the node's own.


def _parent(d):
    """What holds the block the case is about: p, in the nested cases."""
    return d["p"] if d.children[0].name == "p" else d


def _block(d):
    return _parent(d).children


def _removed_a(d):
    _block(d).remove(_parent(d)["a"])
    return str(d)


def _reversed(d):
    _block(d).reverse()
    return str(d)


def _x_inserted_first(d):
    _block(d).insert(0, N("x"))
    return str(d)


def _a_copied(d):
    # Into a block at the same depth, so the comment's own lines keep their indent either way.
    nested = d.children[0].name == "p"
    other = rkdl.parse("q {\n    z\n}\n" if nested else "z\n")
    (other["q"].children if nested else other.children).append(_parent(d)["a"].copy())
    return str(other)


OWN = "/* n\n\n m */\na\nb\n"
OWN_NESTED = "p {\n    /* n\n\n     m */\n    a\n    b\n}\n"
HEADED_OWN = "// h\n\n/* n\n\n m */\na\nb\n"
HEADED_OWN_NESTED = "p {\n    // h\n\n    /* n\n\n     m */\n    a\n    b\n}\n"


@pytest.mark.parametrize(
    ("text", "edit", "expected"),
    [
        # No blank line between pieces: the whole comment is a's own. It goes with a, it stays above a when a node
        # goes in front of a, and it comes with a's copy.
        (OWN, _removed_a, "b\n"),
        (OWN, _x_inserted_first, "x\n/* n\n\n m */\na\nb\n"),
        (OWN, _a_copied, "z\n/* n\n\n m */\na\n"),
        (OWN, _reversed, "b\n/* n\n\n m */\na\n"),
        (OWN_NESTED, _removed_a, "p {\n    b\n}\n"),
        (OWN_NESTED, _x_inserted_first, "p {\n    x\n    /* n\n\n     m */\n    a\n    b\n}\n"),
        (OWN_NESTED, _a_copied, "q {\n    z\n    /* n\n\n     m */\n    a\n}\n"),
        (OWN_NESTED, _reversed, "p {\n    b\n    /* n\n\n     m */\n    a\n}\n"),
        # A blank line between pieces as well: what's above it is the gap and stays.
        (HEADED_OWN, _removed_a, "// h\n\nb\n"),
        (HEADED_OWN, _x_inserted_first, "// h\n\nx\n/* n\n\n m */\na\nb\n"),
        (HEADED_OWN, _a_copied, "z\n/* n\n\n m */\na\n"),
        (HEADED_OWN, _reversed, "// h\n\nb\n/* n\n\n m */\na\n"),
        (HEADED_OWN_NESTED, _removed_a, "p {\n    // h\n\n    b\n}\n"),
        (HEADED_OWN_NESTED, _x_inserted_first, "p {\n    // h\n\n    x\n    /* n\n\n     m */\n    a\n    b\n}\n"),
        (HEADED_OWN_NESTED, _a_copied, "q {\n    z\n    /* n\n\n     m */\n    a\n}\n"),
        (HEADED_OWN_NESTED, _reversed, "p {\n    // h\n\n    b\n    /* n\n\n     m */\n    a\n}\n"),
        # Comments nest.
        ("/* x /* y\n\n */ z */\na\nb\n", _removed_a, "b\n"),
        # Each place keeps its whitespace and line end, and u moves with its whole comment.
        ("h { i }\n/* x\n\n y */\nu /* c */ 2", _reversed, "/* x\n\n y */\nu /* c */ 2\nh { i }"),
        (
            "p {\n    h { i }\n    /* x\n\n     y */\n    u /* c */ 2\n}\n",
            _reversed,
            "p {\n    /* x\n\n     y */\n    u /* c */ 2\n    h { i }\n}\n",
        ),
    ],
)
def test_a_comment_with_a_blank_line_inside_is_one_piece(text, edit, expected):
    out = edit(rkdl.parse(text))
    assert out == expected
    reads_back(out)


@pytest.mark.parametrize(
    "text",
    [
        "/-old {\n\n}\na\nb\n",
        "/-\n\nold\na\nb\n",
        "/-old /-\n\n  1\na\nb\n",
        '/-old """\n\n  """\na\nb\n',
        '/-old #"""\n\n  """#\na\nb\n',
        '/-old {\n    k "}"\n\n}\na\nb\n',
        '/-old {\n    k #"}"#\n\n}\na\nb\n',
        "/-old {\n    // }\n\n}\na\nb\n",
        "p {\n    /-old {\n\n    }\n    a\n    b\n}\n",
    ],
)
def test_a_slashdashed_node_with_a_blank_line_inside_is_one_piece(text):
    out = _removed_a(rkdl.parse(text))
    assert out == ("p {\n    b\n}\n" if text.startswith("p") else "b\n")
    reads_back(out)


# Every line break belongs to the thing before it, after a `;` too.


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a; b\n\nc\n", "a;\n\nc\n"),
        ("a; b;\n\nc\n", "a;\n\nc\n"),
        ("p {\n    a; b\n\n    c\n}\n", "p {\n    a;\n\n    c\n}\n"),
        ("a; b\n\n// about c\nc\n", "a;\n\n// about c\nc\n"),
    ],
)
def test_removing_the_second_of_a_line_keeps_the_blank_line_after_it(text, expected):
    d = rkdl.parse(text)
    _block(d).pop(1)
    assert str(d) == expected
    reads_back(expected)


@pytest.mark.parametrize(
    ("text", "index", "expected"),
    [
        ("p {\n    b\n    c;\n}\n", -1, "p {\n    b\n}\n"),
        ("a\nc;\n", -1, "a\n"),
        ("a;\nb;\n", 0, "b;\n"),
        ("a;\nb;\nc;\n", 1, "a;\nc;\n"),
        ("p {\n    a;\n    b\n}\n", 0, "p {\n    b\n}\n"),
        # The comments on c's line are c's own and go with it.
        ("p {\n    b\n    c; // x\n}\n", -1, "p {\n    b\n}\n"),
        ("b\nc; /* x */ // y\nd\n", 1, "b\nd\n"),
        # One that runs onto the next line is d's.
        ("b\nc; /* x\n*/ d\n", 1, "b\n/* x\n*/ d\n"),
    ],
)
def test_a_node_ending_in_a_semicolon_takes_its_line_ending(text, index, expected):
    d = rkdl.parse(text)
    _block(d).pop(index)
    assert str(d) == expected
    reads_back(expected)


@pytest.mark.parametrize(
    ("target", "move", "expected"),
    [
        ("z\n", lambda d, c: d.children.append(c), "z\nc; // x\n"),
        ("z\n", lambda d, c: d.children.insert(0, c), "c; // x\nz\n"),
        ("z\ny\n", lambda d, c: d.children.__setitem__(0, c), "c; // x\ny\n"),
        ("q {\n    z\n}\n", lambda d, c: d["q"].children.append(c), "q {\n    z\n    c; // x\n}\n"),
    ],
)
def test_a_node_ending_in_a_semicolon_moves_with_the_comment_on_its_line(target, move, expected):
    d = rkdl.parse("p {\n    b\n    c; // x\n}\n")
    other = rkdl.parse(target)
    move(other, d["p"].children.pop())
    assert str(other) == expected
    # The comment is where kdl-rs puts it when it reads the text, so formatting keeps it.
    other.fmt()
    assert "// x" in str(other)
    reads_back(str(other))


def test_removing_the_node_after_a_comment_at_the_end_of_a_line_keeps_it():
    d = rkdl.parse("a; // x\nb\n")
    d.children.pop(1)
    assert str(d) == "a; // x\n"
    d = rkdl.parse("p {\n    a; // x\n    b\n    c\n}\n")
    d["p"].children.pop(1)
    assert str(d) == "p {\n    a; // x\n    c\n}\n"


@pytest.mark.parametrize(
    ("text", "index", "expected"),
    [
        ("a; // x\nb\n", 1, "a; // x\nx\nb\n"),
        ("a; /* x */\n", 1, "a; /* x */\nx\n"),
        ("p {\n    a; // x\n    b\n}\n", 1, "p {\n    a; // x\n    x\n    b\n}\n"),
        # The line after `{` works the same way.
        ("p { // x\n    b\n}\n", 0, "p { // x\n    x\n    b\n}\n"),
    ],
)
def test_a_new_node_goes_below_the_comment_on_the_line_before(text, index, expected):
    d = rkdl.parse(text)
    _block(d).insert(index, N("x"))
    assert str(d) == expected
    reads_back(expected)
    d.fmt()
    assert "// x" in str(d) or "/* x */" in str(d)
    reads_back(str(d))


def test_a_moved_node_s_multi_line_string_keeps_its_lines():
    # Inside strings, the indentation is content: it doesn't move with the node.
    x = rkdl.parse('x """\n  hi\n  """\n').children.pop()
    d = rkdl.parse("a {\n    b\n}\n")
    d["a"].children.append(x)
    assert str(d) == 'a {\n    b\n    x """\n  hi\n  """\n}\n'


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="task 63: lines inside a moved node's own text (continuations, slashdashed blocks) keep their old indent",
)
@pytest.mark.parametrize(
    ("source", "expected"),
    [
        ("x 1 /-{\n    y\n}\n", "a {\n    b\n    x 1 /-{\n        y\n    }\n}\n"),
        ("x \\\n    1\n", "a {\n    b\n    x \\\n        1\n}\n"),
    ],
)
def test_a_moved_node_s_own_lines_take_the_new_indent(source, expected):
    x = rkdl.parse(source).children.pop()
    d = rkdl.parse("a {\n    b\n}\n")
    d["a"].children.append(x)
    assert str(d) == expected


@pytest.mark.xfail(
    strict=True,
    raises=AssertionError,
    reason="task 63: spreading a one-line block leaves `}` after the comment before it, off the parent's indent",
)
def test_spreading_a_one_line_block_puts_its_closing_comment_on_a_line():
    d = rkdl.parse("a { b; /* c */ }\n")
    d["a"].children.append(N("x", children=[N("y")]))
    assert str(d) == "a {\n    b\n    x {\n        y\n    }\n    /* c */\n}\n"
