import rkdl

MESSY = """\
// header
node1   1    2 {
   // inner
      child   key=0x1F90  "x"
  }
   node2   #true
"""


def formatted(text, formatter=None):
    doc = rkdl.parse(text)
    doc.fmt(formatter)
    return str(doc)


def test_default_indent():
    assert formatted(MESSY) == """\
// header
node1 1 2 {
    // inner
    child key=8080 x
}
node2 #true
"""


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
    assert formatted(MESSY, rkdl.Formatter(entry_autoformat_keep=True)) == """\
// header
node1   1    2 {
    // inner
    child   key=0x1F90  "x"
}
node2   #true
"""


def test_parser_formatter_matches_fmt():
    parser = rkdl.Parser(formatter=rkdl.Formatter())
    assert str(parser.parse(MESSY)) == formatted(MESSY)


def test_defaults_and_repr():
    f = rkdl.Formatter()
    assert (f.indent, f.indent_level, f.no_comments, f.entry_autoformat_keep) == ("    ", 0, False, False)
    assert repr(rkdl.Formatter(indent="\t", no_comments=True)) == (
        "Formatter(indent='\\t', indent_level=0, no_comments=True, entry_autoformat_keep=False)"
    )
