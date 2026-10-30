"""The official KDL test suite (vendored in kdl-rs), run through rkdl's Python API."""

import re

import pytest
import rkdl
from conftest import (
    SPEC_EXPECTED,
    has_duplicate_props,
    is_valid_case,
    read_case,
    skip_module_without_kdl_rs,
    spec_inputs,
    walk,
)

skip_module_without_kdl_rs()

TYPE_SPACE = "kdl-rs drops whitespace and comments in and after a node's type annotation"

ROUND_TRIP_FAILS = {
    "comment_after_node_type.kdl": TYPE_SPACE,
    "comment_in_node_type.kdl": TYPE_SPACE,
    "space_after_node_type.kdl": TYPE_SPACE,
    "space_in_node_type.kdl": TYPE_SPACE,
}

INVALID_ACCEPTED = {
    "hex.kdl": "valid KDL (fits i128) with no expected file in kdl-rs's copy of the suite",
    "multiline_raw_string_single_line_err_fail.kdl": 'kdl-rs reads #"""one line"""# as a raw string',
    "unicode_delete_fail.kdl": "kdl-rs accepts U+007F in an identifier",
}

VALID = [p for p in spec_inputs() if is_valid_case(p)]
INVALID = [p for p in spec_inputs() if not is_valid_case(p)]


def cases(paths, fails):
    return [
        pytest.param(
            p, id=p.name, marks=[pytest.mark.xfail(strict=True, reason=fails[p.name])] if p.name in fails else []
        )
        for p in paths
    ]


def canonical(doc):
    """kdl-rs's stringify_to_expected, through the Python API."""
    doc.fmt(rkdl.Formatter(no_comments=True))
    for _, node in walk(doc):
        for entry in node.entries:
            if isinstance(entry.value, str):
                entry.value = entry.value
        node.name = node.name
        # Property names have no setter, but fmt() has already reset their spelling. Duplicate properties can't be
        # removed (del removes the last one, we need to keep it), so the caller skips those cases.
        if node.children is not None and len(node.children) == 0:
            node.children = None
    # The expected files use `\n`, and fmt() keeps the line ending style of the document, which all its line endings
    # outside values now have.
    return re.sub(r"\r\n|[\r\x0b\x0c\x85\u2028\u2029]", "\n", str(doc))


@pytest.mark.parametrize("path", cases(VALID, ROUND_TRIP_FAILS))
def test_round_trip(path):
    text = read_case(path)
    assert str(rkdl.parse(text)) == text


@pytest.mark.parametrize("path", cases(VALID, {}))
def test_canonical(path):
    doc = rkdl.parse(read_case(path))
    if has_duplicate_props(doc):
        pytest.skip("rkdl can't remove a duplicate property other than the last")
    assert canonical(doc) == read_case(SPEC_EXPECTED / path.name)


@pytest.mark.parametrize("path", cases(INVALID, INVALID_ACCEPTED))
def test_invalid(path):
    text = read_case(path)
    if text is None:
        pytest.skip("not UTF-8: rkdl only takes str, so the file is rejected before it gets there")
    with pytest.raises(rkdl.ParseError):
        rkdl.parse(text)
