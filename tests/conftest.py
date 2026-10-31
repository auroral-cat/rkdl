"""Paths into the KDL spec's test cases, and helpers more than one test file uses."""

import os
import threading
import time
from pathlib import Path

import pytest
import rkdl

# The spec's test cases come from kdl-org/kdl, checked out at tests/kdl-spec as a submodule. RKDL_SPEC points the
# tests at another checkout of that repo instead.
SPEC = Path(os.environ.get("RKDL_SPEC", Path(__file__).resolve().parent / "kdl-spec"))
SPEC_INPUT = SPEC / "tests" / "test_cases" / "input"
SPEC_EXPECTED = SPEC / "tests" / "test_cases" / "expected_kdl"

MISSING_SPEC = (
    f"KDL spec test cases not found under {SPEC}: run `git submodule update --init`, "
    "or point RKDL_SPEC at a kdl-org/kdl checkout"
)


def have_spec():
    return SPEC_INPUT.is_dir() and SPEC_EXPECTED.is_dir() and any(SPEC_INPUT.glob("*.kdl"))


def pytest_report_header():
    return f"KDL spec: {SPEC}" + ("" if have_spec() else " (MISSING, spec tests will skip)")


def pytest_terminal_summary(terminalreporter):
    # Repeated at the end, where it can't scroll away: a run without the spec cases skips most of the suite.
    if not have_spec():
        terminalreporter.write_sep("=", "spec tests skipped", yellow=True, bold=True)
        terminalreporter.write_line(MISSING_SPEC, yellow=True)


def skip_module_without_spec():
    if not have_spec():
        pytest.skip(MISSING_SPEC, allow_module_level=True)


def spec_inputs():
    return sorted(SPEC_INPUT.glob("*.kdl")) if have_spec() else []


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


# Long enough for any test on a slow machine, short enough that a deadlock fails instead of hanging.
THREAD_TIMEOUT = 60


def run_threads(*targets):
    """Runs each target in its own thread, all starting together, and returns what each returned.

    A thread that's still running after the timeout fails the test, and so does any exception a thread raised.
    """
    barrier = threading.Barrier(len(targets))
    results = [None] * len(targets)
    errors = []

    def wrap(i, target):
        def go():
            try:
                barrier.wait()
                results[i] = target()
            except BaseException as e:  # noqa: BLE001
                errors.append(e)

        return go

    # Daemon threads, so a deadlocked one can't keep the test process alive either.
    threads = [threading.Thread(target=wrap(i, t), daemon=True) for i, t in enumerate(targets)]
    for t in threads:
        t.start()
    deadline = time.monotonic() + THREAD_TIMEOUT
    for t in threads:
        t.join(max(0, deadline - time.monotonic()))
    stuck = sum(t.is_alive() for t in threads)
    if stuck:
        pytest.fail(f"{stuck} of {len(threads)} threads still running after {THREAD_TIMEOUT}s: deadlock?")
    if errors:
        raise errors[0]
    return results
