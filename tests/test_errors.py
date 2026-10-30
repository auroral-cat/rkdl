import pytest
import rkdl
from conftest import parse_error


def test_parse_error_is_value_error():
    assert issubclass(rkdl.ParseError, ValueError)
    assert isinstance(parse_error("a {"), ValueError)


def test_position_counts_code_points():
    # The stray brace is at byte 11 but code point 9.
    err = parse_error("é é {\nb }}\n")
    assert (err.line, err.column, err.offset) == (2, 4, 9)
    first = err.diagnostics[0]
    assert (first.line, first.column, first.offset) == (2, 4, 9)


def test_span_over_multibyte_text():
    # The bad number "1日本x" is 8 bytes but 4 code points, after a 2-byte "é".
    err = parse_error("é 1日本x")
    first = err.diagnostics[0]
    assert (first.line, first.column, first.offset, first.length) == (1, 3, 2, 4)


def test_several_diagnostics():
    err = parse_error('name #"raw"#"quoted key"=1')
    assert len(err.diagnostics) == 2
    report = str(err)
    for diag in err.diagnostics:
        assert diag.message in report
    assert (err.line, err.column, err.offset) == (1, 6, 5)


def test_report_has_snippet_and_no_ansi():
    err = parse_error('\tnode "ünïcødé" {\n\t\tchild }}\n')
    report = str(err)
    assert "\x1b[" not in report
    assert "\x1b]" not in report
    assert "child }}" in report
    assert err.diagnostics[0].message in report


@pytest.mark.xfail(
    strict=True,
    reason="kdl-rs: an integer outside i128 fails as 'Expected end of document' and 'Found invalid node name'",
)
@pytest.mark.parametrize(
    "number", ["170141183460469231731687303715884105728", "-170141183460469231731687303715884105729"]
)
def test_integer_outside_i128_is_reported_as_one(number):
    report = str(parse_error(f"n {number}"))
    assert "end of document" not in report
    assert "node name" not in report


@pytest.mark.xfail(strict=True, reason="kdl-rs accepts U+007F inside a quoted string")
def test_delete_inside_a_quoted_string_is_rejected():
    parse_error('n "a\x7fb"')
