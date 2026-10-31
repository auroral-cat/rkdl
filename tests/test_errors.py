import io
import os
import pickle

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


def test_report_is_a_summary_then_each_diagnostic():
    # No `Error: ` line before each diagnostic, as miette puts before related reports.
    err = parse_error('name #"raw"#"quoted key"=1')
    parts = str(err).split("\n\n")
    assert parts[0] == "1:6: Failed to parse KDL document"
    assert [part.splitlines()[0] for part in parts[1:]] == [f"  × {d.message}" for d in err.diagnostics]
    assert not any(line.startswith("Error") for line in str(err).splitlines())


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


def test_rkdl_refusals_are_reported_like_kdl_rs_errors():
    # The prescan's refusals go through the same report as kdl-rs's errors. A long block comment is safe to try in
    # process: kdl-rs would only overflow on one many times longer.
    err = parse_error("a\n/*" + "*" * 20_000 + "*/")
    assert (err.line, err.column, err.offset) == (2, 1, 2)
    [diagnostic] = err.diagnostics
    assert diagnostic.message == "Block comment too long"
    report = str(err)
    assert "this comment" in report
    assert "recurses once per piece" in report


def test_a_long_document_after_an_error_reports_the_first_error():
    # After an error kdl-rs's parser might misread the rest, so rkdl reports the first error itself rather than
    # letting kdl-rs try a long document. A short one still gets kdl-rs's report.
    rest = "\nb {}" * 10_000
    err = parse_error('a "\\q"' + rest)
    assert (err.line, err.column) == (1, 4)
    assert err.diagnostics[0].message == "Invalid escape in a string"
    assert "rkdl stopped at the first error" in str(err)
    assert "rkdl stopped" not in str(parse_error('a "\\q"' + rest[:100]))


def test_parse_error_pickles_with_its_diagnostics():
    # So it can come back from a ProcessPoolExecutor worker.
    err = parse_error("a {\nb }}\n")
    again = pickle.loads(pickle.dumps(err))
    assert type(again) is rkdl.ParseError
    assert str(again) == str(err)
    assert (again.line, again.column, again.offset) == (err.line, err.column, err.offset)
    assert again.diagnostics == err.diagnostics
    assert [d.message for d in again.diagnostics] == [d.message for d in err.diagnostics]


BAD = "a 1\nb {\n"


def load_error(source, load=rkdl.load):
    with pytest.raises(rkdl.ParseError) as info:
        load(source)
    return info.value


LOADERS = pytest.mark.parametrize(
    "load", [rkdl.load, rkdl.Parser(types=[rkdl.Type("t", str, str, str)]).load], ids=["rkdl", "Parser"]
)


@pytest.mark.parametrize(
    "open_it",
    [
        lambda path: path,
        lambda path: str(path),
        lambda path: open(path, "rb"),  # noqa: SIM115
        lambda path: open(path, encoding="utf-8"),  # noqa: SIM115
    ],
    ids=["Path", "str", "binary file", "text file"],
)
@LOADERS
def test_a_parse_error_from_load_names_the_file(tmp_path, open_it, load):
    path = tmp_path / "config.kdl"
    path.write_text(BAD)
    source = open_it(path)
    try:
        err = load_error(source, load)
    finally:
        if hasattr(source, "close"):
            source.close()
    assert err.path == str(path)
    lines = str(err).splitlines()
    assert lines[0] == f"{path}:2:3: Failed to parse KDL document"
    assert f"╭─[{path}:2:3]" in str(err)
    assert (err.line, err.column) == (2, 3)
    again = pickle.loads(pickle.dumps(err))
    assert again.path == err.path
    assert str(again) == str(err)


@LOADERS
def test_a_parse_error_from_text_has_no_path(load):
    err = parse_error(BAD)
    assert err.path is None
    assert str(err).splitlines()[0] == "2:3: Failed to parse KDL document"
    assert "╭─[2:3]" in str(err)
    # A file object without a path, like one in memory.
    assert load_error(io.StringIO(BAD), load).path is None
    assert load_error(io.BytesIO(BAD.encode()), load).path is None


def stdin_with(text):
    # A pipe read the way Python reads sys.stdin: a FileIO named "<stdin>", buffered and decoded.
    read, write = os.pipe()
    os.write(write, text.encode())
    os.close(write)
    raw = io.FileIO(read, "r")
    raw.name = "<stdin>"
    return io.TextIOWrapper(io.BufferedReader(raw), encoding="utf-8")


@LOADERS
def test_stdin_is_not_a_path(load):
    with stdin_with(BAD) as stdin:
        err = load_error(stdin, load)
    assert err.path is None
    assert str(err).startswith("2:3: ")
    with stdin_with("a 1\n") as stdin:
        assert load(stdin).path is None


def test_diagnostics_compare_by_value():
    first = parse_error("a {").diagnostics[0]
    same = rkdl.Diagnostic(first.message, first.label, first.help, first.offset, first.length, first.line, first.column)
    assert same == first
    assert hash(same) == hash(first)
    assert pickle.loads(pickle.dumps(first)) == first


def test_diagnostic_repr_shows_where_and_what():
    # A diagnostic a program builds for its own check, as a ParseError's would read.
    diagnostic = rkdl.Diagnostic("unknown setting", "not a setting", None, 14, 7, 2, 5)
    assert repr(diagnostic) == '<Diagnostic 2:5 "unknown setting">'


def lines(eol, last):
    return eol.join(["a", "", f"  b 1; {last}", "c"])


# Each case is a file with a stray `}` and its valid twin with a node `x` in the same spot.
LINE_COUNTING = {
    **{
        name: (lines(eol, "}"), lines(eol, "x"), (3, 8))
        for name, eol in [
            ("LF", "\n"),
            ("CRLF", "\r\n"),
            ("CR", "\r"),
            ("VT", "\x0b"),
            ("FF", "\x0c"),
            ("NEL", "\x85"),
            ("LS", "\u2028"),
            ("PS", "\u2029"),
        ]
    },
    # The byte order mark isn't a column, and é is one column of two bytes.
    "BOM": ("\ufeffb é; }\n", "\ufeffb é; x\n", (1, 6)),
    "tab": ("a\n\tb 1; }\n", "a\n\tb 1; x\n", (2, 7)),
}


@pytest.mark.parametrize(("bad", "good", "at"), LINE_COUNTING.values(), ids=LINE_COUNTING.keys())
def test_parse_errors_count_lines_as_node_line_does(tmp_path, bad, good, at):
    twin = rkdl.parse(good)["x"]
    assert (twin.line, twin.column) == at
    line, column = at

    err = parse_error(bad)
    assert (err.line, err.column) == at
    assert (err.diagnostics[0].line, err.diagnostics[0].column) == at
    assert str(err).splitlines()[0] == f"{line}:{column}: Failed to parse KDL document"

    path = tmp_path / "config.kdl"
    path.write_bytes(bad.encode())
    with pytest.raises(rkdl.ParseError) as info:
        rkdl.load(path)
    loaded = info.value
    assert (loaded.line, loaded.column) == at
    assert str(loaded).splitlines()[0] == f"{path}:{line}:{column}: Failed to parse KDL document"
    # miette's snippet header agrees, though the snippet under it only breaks lines at LF and CRLF.
    assert f"╭─[{path}:{line}:{column}]" in str(loaded)


# KDL v1 spellings: the diagnostic's help says the file looks like v1 and gives the v2 form.

V1 = {
    "bare true": ("node true", "#true"),
    "bare false": ("node 1 false", "#false"),
    "bare null as a property": ("node key=null", "#null"),
    "bare null after an annotation": ("node (t)null", "#null"),
    "raw string": ('node r"C:\\path"', '#"C:\\path"#'),
    "raw string with hashes": ('node r#"say "hi""#', '#"say "hi""#'),
    "escaped slash": ('node "a\\/b"', '"a/b"'),
}


@pytest.mark.parametrize("name", V1)
def test_a_v1_spelling_gets_a_help_with_the_v2_form(name):
    text, v2 = V1[name]
    first = parse_error(text).diagnostics[0]
    assert first.help.splitlines()[0] == f"this looks like KDL v1: in KDL v2 it's {v2}"
    rkdl.parse(f"node {v2}")


@pytest.mark.parametrize("name", V1)
def test_the_v1_help_leaves_the_rest_of_the_diagnostic_alone(name):
    text, _ = V1[name]
    first = parse_error(text).diagnostics[0]
    assert first.message in ("Expected identifier string", "Expected quoted string")
    assert first.label in ("not identifier string", "not quoted string")


def test_the_v1_help_replaces_kdl_rs_help():
    first = parse_error('node r"raw"').diagnostics[0]
    assert first.help == 'this looks like KDL v1: in KDL v2 it\'s #"raw"#'


def test_kdl_rs_help_stays_where_there_is_no_v1_help():
    first = parse_error('node "multi\nline"').diagnostics[0]
    assert first.help.startswith("You can make a string multi-line")


def test_the_v1_help_is_in_the_report():
    err = parse_error("node {\n    enabled true\n}\n")
    assert str(err).splitlines()[0] == "2:13: Failed to parse KDL document"
    assert "  help: this looks like KDL v1: in KDL v2 it's #true" in str(err).splitlines()


def test_a_long_or_multiline_raw_string_gets_the_rule_instead_of_the_v2_form():
    long = parse_error('node r"' + "x" * 50 + '"').diagnostics[0]
    assert (
        long.help.splitlines()[0] == 'this looks like KDL v1: in KDL v2 raw strings are written #"..."#, without the r'
    )
    multiline = parse_error('node r"one\ntwo"').diagnostics[0]
    assert multiline.help.splitlines()[0] == long.help.splitlines()[0]


def test_an_unfinished_string_gets_the_rule_instead_of_the_v2_form():
    # A file cut off inside a string, which has no v2 form to show; once right after a backslash.
    for text in ['out "build\\/', 'out "build\\/\\']:
        first = parse_error(text).diagnostics[0]
        assert first.help.splitlines()[0] == "this looks like KDL v1: in KDL v2 / isn't escaped: write \\/ as /"


@pytest.mark.parametrize(
    "text",
    [
        "true 1",  # a node name: v1 didn't allow it either
        "term true-color#true",  # an identifier starting with true-, with a missing space after it
        "retries 3) true",  # after a stray ): kdl-rs reads true as a node name, which #true isn't either
        "node true=1",  # a property key
        "node (true)1",  # a type annotation
        "node { null; }",
        'node "a\\q"',  # a bad escape, not \/
        "node 0b2",
    ],
)
def test_other_errors_get_no_v1_help(text):
    for diagnostic in parse_error(text).diagnostics:
        assert "KDL v1" not in (diagnostic.help or "")


def test_v2_files_with_the_same_words_parse():
    doc = rkdl.parse('"true" #true r="a\\\\b" "true"=#null null-ish "a/b" #"r"x"#\n')
    assert doc["true"].args == [True, "null-ish", "a/b", 'r"x']
    assert doc["true"].props == {"r": "a\\b", "true": None}
