import pytest
import rkdl
from conftest import formatted

MESSY = """\
// header
node1   1    2 {
   // inner
      child   key=0x1F90  "x"
  }
   node2   #true
"""


def test_default_indent():
    assert (
        formatted(MESSY)
        == """\
// header
node1 1 2 {
    // inner
    child key=8080 x
}
node2 #true
"""
    )


def test_fmt_without_argument_uses_defaults():
    doc = rkdl.parse(MESSY)
    doc.fmt()
    assert str(doc) == formatted(MESSY, rkdl.Formatter())


def test_tab_indent():
    assert formatted("a {\n  b {\n c\n }\n}\n", rkdl.Formatter(indent="\t")) == "a {\n\tb {\n\t\tc\n\t}\n}\n"


def test_no_comments():
    assert formatted(MESSY, rkdl.Formatter(no_comments=True)) == "node1 1 2 {\n    child key=8080 x\n}\nnode2 #true\n"


def test_entry_autoformat_keep():
    # Keeping entry formatting keeps the whitespace before each entry too; only node indentation changes.
    assert (
        formatted(MESSY, rkdl.Formatter(entry_autoformat_keep=True))
        == """\
// header
node1   1    2 {
    // inner
    child   key=0x1F90  "x"
}
node2   #true
"""
    )


def test_parser_formatter_matches_fmt():
    parser = rkdl.Parser(formatter=rkdl.Formatter())
    assert str(parser.parse(MESSY)) == formatted(MESSY)


# Strings kdl-rs's own spelling gets wrong: `+.7` bare, U+0000 and the rest unescaped.
AWKWARD = '("+.7")"-.7"   "+.7"=("\\u{0}")"\\u{2028}" "\\u{feff}"=("\\u{7f}")1 {\n "-.5"  "\\u{85}"\n  }\n'
AWKWARD_FORMATTED = '("+.7")"-.7" "+.7"=("\\u{0}")"\\u{2028}" "\\u{feff}"=("\\u{7f}")1 {\n    "-.5" "\\u{85}"\n}\n'


@pytest.mark.parametrize("formatter", [None, rkdl.Formatter(no_comments=True)])
def test_fmt_keeps_strings_quoted_that_need_it(formatter):
    assert formatted(AWKWARD, formatter) == AWKWARD_FORMATTED


def test_fmt_keeping_entry_format_keeps_strings_quoted():
    text = formatted(AWKWARD, rkdl.Formatter(entry_autoformat_keep=True))
    assert text == '("+.7")"-.7"   "+.7"=("\\u{0}")"\\u{2028}" "\\u{feff}"=("\\u{7f}")1 {\n    "-.5"  "\\u{85}"\n}\n'


def test_fmt_keeps_quoting_on_written_strings():
    doc = rkdl.parse("n 1\n")
    node = doc["n"]
    node.args[0] = "\x00"
    node.props["+.7"] = "\u2028"
    node.type = "-.7"
    doc.fmt()
    assert str(doc) == '("-.7")n "\\u{0}" "+.7"="\\u{2028}"\n'


def test_parser_formatter_keeps_strings_quoted():
    parser = rkdl.Parser(formatter=rkdl.Formatter())
    assert str(parser.parse(AWKWARD)) == AWKWARD_FORMATTED


def test_fmt_leaves_other_spellings_alone():
    assert formatted('"a" #"b"# "c d" 0x10 k="v"\n') == 'a b "c d" 16 k=v\n'


@pytest.mark.parametrize("comment", ["/* end */", "; // end", "; /* end */"])
def test_fmt_keeps_comments_at_the_end_of_a_line(comment):
    text = f"a 1 {comment}\nb\n"
    assert "end" in formatted(text)
    assert "end" not in formatted(text, rkdl.Formatter(no_comments=True))


@pytest.mark.xfail(
    strict=True,
    reason="kdl-rs: KdlNode::autoformat_config replaces any terminator that isn't a newline with one, and a `//` "
    "comment ending a node is its terminator",
)
@pytest.mark.parametrize("text", ["a 1 // end\nb\n", "p {\n    a 1 // end\n}\n"])
def test_fmt_keeps_a_line_comment_ending_a_node(text):
    assert "// end" in formatted(text)


@pytest.mark.xfail(strict=True, reason="kdl-rs: KdlEntry::autoformat resets the entry format, comments included")
def test_fmt_keeps_comments_between_entries():
    assert formatted("a 1 /* mid */ 2\n") == "a 1 /* mid */ 2\n"


@pytest.mark.xfail(
    strict=True,
    reason="kdl-rs: autoformat_trailing ends a node's trailing comment or `/-` with a newline, and the terminator "
    "adds another",
)
@pytest.mark.parametrize("text", ["a 1 /* end */\n", "a /-b\n", "a /-{}\n"])
def test_fmt_leaves_no_blank_line_after_a_trailing_comment(text):
    assert formatted(text) == text


def test_defaults_and_repr():
    f = rkdl.Formatter()
    assert (f.indent, f.no_comments, f.entry_autoformat_keep) == ("    ", False, False)
    assert repr(rkdl.Formatter(indent="\t", no_comments=True)) == (
        "Formatter(indent='\\t', no_comments=True, entry_autoformat_keep=False)"
    )


@pytest.mark.parametrize("indent", ["", " ", "\t", "  \t", "　", " "])
def test_indent_can_be_any_kdl_space(indent):
    assert formatted("a {\nb\n}\n", rkdl.Formatter(indent=indent)) == f"a {{\n{indent}b\n}}\n"


# Each of these would change what the document says: a new node, children commented out, or text that doesn't parse.
@pytest.mark.parametrize("indent", ["x", "  // ", "x}", "\n", "\r\n  ", ";", "\\"])
def test_indent_must_be_kdl_space(indent):
    with pytest.raises(ValueError, match="indent must be KDL whitespace"):
        rkdl.Formatter(indent=indent)


def test_indent_level_is_not_a_parameter():
    with pytest.raises(TypeError, match="unexpected keyword argument 'indent_level'"):
        rkdl.Formatter(indent_level=2)


@pytest.mark.parametrize("eol", ["\r\n", "\r", "\u2028"])
def test_fmt_keeps_the_line_ending_style(eol):
    text = "a 1 { b; c }\n// d\ne\n".replace("\n", eol)
    doc = rkdl.parse(text)
    doc.fmt()
    assert str(doc) == "a 1 {\n    b\n    c\n}\n// d\ne\n".replace("\n", eol)
    parsed = rkdl.Parser(formatter=rkdl.Formatter()).parse(text)
    assert str(parsed) == str(doc)
