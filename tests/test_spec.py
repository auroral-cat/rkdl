"""The KDL spec's test cases (kdl-org/kdl's tests/test_cases), run through rkdl's Python API."""

import re

import pytest
import rkdl
from conftest import SPEC_EXPECTED, is_valid_case, read_case, skip_module_without_spec, spec_inputs, walk

skip_module_without_spec()

INVALID_ACCEPTED = {
    "multiline_raw_string_single_line_err_fail.kdl": 'kdl-rs reads #"""one line"""# as a raw string',
    "unicode_delete_fail.kdl": "kdl-rs accepts U+007F in an identifier",
}

VALID = [p for p in spec_inputs() if is_valid_case(p)]
INVALID = [p for p in spec_inputs() if not is_valid_case(p)]

WRONG_EXPECTED = {
    # SPEC.md's newline table lists U+000B, so `node<VT>arg` is two nodes. The case was fixed upstream after 2.0.0.
    "vertical_tab_whitespace.kdl": "the 2.0.0 suite expects U+000B to be whitespace, but the spec makes it a newline",
}


def cases(paths, fails):
    return [
        pytest.param(
            p, id=p.name, marks=[pytest.mark.xfail(strict=True, reason=fails[p.name])] if p.name in fails else []
        )
        for p in paths
    ]


def canonical(doc):
    """The document printed the way the spec's tests/README.md describes the expected_kdl files."""
    # No comments, no blank lines, no escaped newlines, 4-space indents.
    doc.fmt(rkdl.Formatter(no_comments=True))
    for _, node in walk(doc):
        # Arguments in order, then properties in alphabetical order, keeping only the rightmost of duplicates.
        # Writing the values back gives them fresh spellings: strings plain, quoted only where they must be, with
        # escapes; integers in decimal; type annotations quoted only where they must be.
        args = [rkdl.Value(e.value, e.type) for e in node.entries if e.name is None]
        props = {e.name: rkdl.Value(e.value, e.type) for e in node.entries if e.name is not None}
        node.props = {}
        node.args = args
        node.props = dict(sorted(props.items()))
        node.name = node.name
        node.type = node.type
        # Children only if there are some.
        if node.children is not None and len(node.children) == 0:
            node.children = None
    # The expected files end lines with `\n`. fmt() gives every line ending outside a value the document's style.
    return re.sub(r"\r\n|[\r\x0b\x0c\x85\u2028\u2029]", "\n", str(doc))


# A number token: at the start of a line or after a space, `=` or a type annotation, up to a space, `;`, a brace or
# the end of the line. Quoted strings are matched first so nothing inside them counts.
TOKEN = re.compile(
    r'"(?:[^"\\]|\\.)*"'
    r"|(?:(?<=[\s=)])|^)(?P<number>#-?inf|#nan|[-+]?[0-9][0-9_]*(?:\.[0-9][0-9_]*)?(?:[eE][-+]?[0-9][0-9_]*)?)(?=[\s;{}]|$)",
    re.MULTILINE,
)


def floats_alike(text):
    """Every float written the same way, Python's repr.

    The expected files write floats like `1.0E-10`, and rkdl prints `1e-10`. The spec's README says parsers don't
    have to represent numbers any particular way, so only the value is compared, as the Python float rkdl reads.
    `1.23E+1000` is too large for one: it's infinity both in the expected file and in rkdl's `#inf`.
    """

    def one(match):
        number = match["number"]
        if number is None or not re.search(r"[.eE#]", number):
            return match[0]
        return repr(float(number.removeprefix("#").replace("_", "")))

    return TOKEN.sub(one, text)


def test_floats_alike():
    assert floats_alike("n 1.0E-10 1e-10 x=1_0.5e+1_0 (t)2.0 3 0x1 a1.0\n") == (
        "n 1e-10 1e-10 x=105000000000.0 (t)2.0 3 0x1 a1.0\n"
    )
    assert floats_alike('n "1.0E-10" 1.5;m 2E1{\n') == 'n "1.0E-10" 1.5;m 20.0{\n'
    assert floats_alike("n 1.23E+1000 #inf #-inf #nan\n") == "n inf inf -inf nan\n"


@pytest.mark.parametrize("path", cases(VALID, {}))
def test_round_trip(path):
    text = read_case(path)
    assert str(rkdl.parse(text)) == text


@pytest.mark.parametrize("path", cases(VALID, WRONG_EXPECTED))
def test_canonical(path):
    doc = rkdl.parse(read_case(path))
    assert floats_alike(canonical(doc)) == floats_alike(read_case(SPEC_EXPECTED / path.name))


@pytest.mark.parametrize("path", cases(INVALID, INVALID_ACCEPTED))
def test_invalid(path):
    text = read_case(path)
    if text is None:
        pytest.skip("not UTF-8: rkdl only takes str, so the file is rejected before it gets there")
    with pytest.raises(rkdl.ParseError):
        rkdl.parse(text)


# A `;` ends a slashdashed node like any other: in the spec's grammar, `node := base-node node-terminator`, a
# `base-node` may start with `slashdash`, and `;` is a `node-terminator`.


@pytest.mark.xfail(strict=True, reason="kdl-rs rejects a `;` straight after a slashdashed node")
@pytest.mark.parametrize("text", ["/- foo bar;", "/-n 1;"])
def test_semicolon_after_a_slashdashed_node(text):
    doc = rkdl.parse(text)
    assert len(doc) == 0
    assert str(doc) == text
