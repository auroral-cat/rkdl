import io
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
