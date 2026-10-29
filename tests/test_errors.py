import pytest

import rkdl


def parse_error(text):
    with pytest.raises(rkdl.ParseError) as info:
        rkdl.parse(text)
    return info.value


def test_parse_error_is_value_error():
    assert issubclass(rkdl.ParseError, ValueError)
    assert isinstance(parse_error("a {"), ValueError)


def test_position_counts_code_points():
    # The stray brace is at byte 11 but code point 9.
    err = parse_error("é é {\nb }}\n")
    assert (err.line, err.column, err.offset) == (2, 4, 9)
    first = err.diagnostics[0]
    assert (first.line, first.column, first.offset) == (2, 4, 9)


def test_several_diagnostics():
    err = parse_error('name #"raw"#"quoted key"=1')
    assert len(err.diagnostics) == 2
    report = str(err)
    for diag in err.diagnostics:
        assert diag.message in report
    assert (err.line, err.column, err.offset) == (1, 6, 5)


def test_report_has_snippet_and_no_ansi():
    err = parse_error("\tnode \"ünïcødé\" {\n\t\tchild }}\n")
    report = str(err)
    assert "\x1b[" not in report
    assert "\x1b]" not in report
    assert "child }}" in report
    assert err.diagnostics[0].message in report
