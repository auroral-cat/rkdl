"""The official KDL test suite (vendored in kdl-rs), run through rkdl's Python API."""

import os
from pathlib import Path

import pytest

import rkdl

CASES = Path(os.environ.get("RKDL_SPEC_CASES", "/home/aurora/code/rust/rkdl/refs/kdl-rs/tests/test_cases"))
if not (CASES / "input").is_dir():
    pytest.skip(f"spec test cases not found at {CASES}", allow_module_level=True)

TYPE_SPACE = "kdl-rs drops whitespace and comments in and after a node's type annotation"

ROUND_TRIP_FAILS = {
    "comment_after_node_type.kdl": TYPE_SPACE,
    "comment_in_node_type.kdl": TYPE_SPACE,
    "space_after_node_type.kdl": TYPE_SPACE,
    "space_in_node_type.kdl": TYPE_SPACE,
    "space_after_node.kdl": "kdl-rs drops everything after a closing } up to the newline, newline included",
}

CANONICAL_FAILS = {
    "prop_ambiguous_number.kdl": "kdl-rs prints the strings +.7 and -.7 bare, which don't parse back",
}

INVALID_ACCEPTED = {
    "hex.kdl": "valid KDL (fits i128) with no expected file in kdl-rs's copy of the suite",
    "multiline_raw_string_single_line_err_fail.kdl": 'kdl-rs reads #"""one line"""# as a raw string',
    "unicode_delete_fail.kdl": "kdl-rs accepts U+007F in an identifier",
}


def read(path):
    """The file's text with CRLF normalised, or None if it isn't UTF-8."""
    try:
        text = path.read_bytes().decode("utf-8")
    except UnicodeDecodeError:
        return None
    return text.replace("\r\n", "\n")


INPUTS = sorted(p.name for p in (CASES / "input").glob("*.kdl"))
VALID = sorted(n for n in INPUTS if (CASES / "expected_kdl" / n).exists())
SKIPPED = sorted(n for n in INPUTS if (CASES / "expected_kdl" / f"_{n}").exists())
INVALID = sorted(n for n in INPUTS if n not in VALID and n not in SKIPPED)


def cases(names, fails):
    return [
        pytest.param(n, id=n, marks=pytest.mark.xfail(strict=True, reason=fails[n])) if n in fails else n
        for n in names
    ]


def nodes(container):
    for node in container.children or ():
        yield node
        yield from nodes(node)


def has_duplicate_props(doc):
    return any(len(node.props) != sum(1 for e in node.entries if e.name is not None) for node in nodes(doc))


def canonical(doc):
    """kdl-rs's stringify_to_expected, through the Python API."""
    doc.fmt(rkdl.Formatter(no_comments=True))
    for node in nodes(doc):
        for entry in node.entries:
            if isinstance(entry.value, str):
                entry.value = entry.value
        node.name = node.name
        # Property names have no setter, but fmt() has already reset their spelling. Duplicate properties can't be
        # removed (del removes the last one, we need to keep it), so the caller skips those cases.
        if node.children is not None and len(node.children) == 0:
            node.children = None
    return str(doc)


@pytest.mark.parametrize("name", cases(VALID + SKIPPED, ROUND_TRIP_FAILS))
def test_round_trip(name):
    text = read(CASES / "input" / name)
    assert str(rkdl.parse(text)) == text


@pytest.mark.parametrize("name", cases(VALID, CANONICAL_FAILS))
def test_canonical(name):
    doc = rkdl.parse(read(CASES / "input" / name))
    if has_duplicate_props(doc):
        pytest.skip("rkdl can't remove a duplicate property other than the last")
    expected = read(CASES / "expected_kdl" / name)
    assert canonical(doc) == expected


@pytest.mark.parametrize("name", cases(INVALID, INVALID_ACCEPTED))
def test_invalid(name):
    text = read(CASES / "input" / name)
    if text is None:
        # rkdl only takes str, so a file that isn't UTF-8 is rejected before it gets there.
        return
    with pytest.raises(rkdl.ParseError):
        rkdl.parse(text)
