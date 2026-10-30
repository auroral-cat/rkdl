"""Paths into the kdl-rs checkout, and helpers more than one test file uses."""

import os
from pathlib import Path

import pytest
import rkdl

# Everything the tests read from kdl-rs comes from this one checkout: the KDL spec suite and the examples.
KDL_RS = Path(os.environ.get("RKDL_KDL_RS", Path(__file__).resolve().parent.parent / "refs" / "kdl-rs"))
SPEC_INPUT = KDL_RS / "tests" / "test_cases" / "input"
SPEC_EXPECTED = KDL_RS / "tests" / "test_cases" / "expected_kdl"
EXAMPLES = KDL_RS / "examples"

MISSING_KDL_RS = f"kdl-rs checkout not found at {KDL_RS}: clone it there or point RKDL_KDL_RS at one"


def have_kdl_rs():
    return SPEC_INPUT.is_dir() and SPEC_EXPECTED.is_dir() and EXAMPLES.is_dir()


def pytest_report_header():
    return f"kdl-rs: {KDL_RS}" + ("" if have_kdl_rs() else " (MISSING, spec tests will skip)")


def skip_module_without_kdl_rs():
    if not have_kdl_rs():
        pytest.skip(MISSING_KDL_RS, allow_module_level=True)


def spec_inputs():
    return sorted(SPEC_INPUT.glob("*.kdl")) if have_kdl_rs() else []


def is_valid_case(path):
    """Whether a spec input is valid KDL, which the suite says by giving it an expected output."""
    return (SPEC_EXPECTED / path.name).exists()


def read_case(path):
    """A spec file's text with CRLF normalised, or None if it isn't UTF-8."""
    try:
        text = path.read_bytes().decode("utf-8")
    except UnicodeDecodeError:
        return None
    return text.replace("\r\n", "\n")


def parse_error(text):
    with pytest.raises(rkdl.ParseError) as info:
        rkdl.parse(text)
    return info.value


def formatted(text, formatter=None):
    doc = rkdl.parse(text)
    doc.fmt(formatter)
    return str(doc)


def walk(container, path=()):
    """Every node under a document or node, depth first, with its path of child indexes."""
    for i, node in enumerate(container.children or ()):
        yield (*path, i), node
        yield from walk(node, (*path, i))


def has_duplicate_props(container):
    return any(len(node.props) != sum(1 for e in node.entries if e.name is not None) for _, node in walk(container))
