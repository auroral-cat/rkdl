"""kdl-rs's own tests, ported to rkdl's Python API.

Skipped, because they test Rust-only API:
- document.rs: span_test. Spans of single nodes and entries aren't exposed.
- document.rs: autoformat. Marked #[ignore] upstream; kdl-rs itself doesn't pass it.
- document.rs: v1_v2_conversions, entry.rs: v1_to_v2_format, formatting.rs: the three ensure_v1/v2 tests. KDL v1.
- entry.rs: new. Checks KdlEntry struct fields, which a parsed or built Python entry doesn't expose beyond
  name/value/type (covered by display below).
- identifier.rs: formatting's set_repr part. Identifier reprs can't be set from Python.
- identifier.rs: parsing's "   space   " case. Only standalone KdlIdentifier parsing rejects surrounding space.
- error.rs: kdl_error, kdl_diagnostic. They build KdlError and KdlDiagnostic by hand and test their Rust traits.
- v2_parser.rs: test_binary's "123" case and take() leftovers. They call the binary sub-parser directly.
- Format structs (KdlNodeFormat, KdlEntryFormat leading/trailing): checked through round-tripping instead.
"""

import pytest
import rkdl
from conftest import EXAMPLES, MISSING_KDL_RS, formatted, have_kdl_rs, parse_error


def diagnostics(text):
    return [(d.message, d.label, d.help, d.offset, d.length) for d in parse_error(text).diagnostics]


def only_arg(text):
    node = rkdl.parse(f"n {text}").children[0]
    assert len(node.args) == 1
    return node.args[0]


# document.rs


def test_document_canonical_clear_fmt():
    # document.rs: canonical_clear_fmt. clear_format_recursive isn't exposed; autoformatting without comments is the
    # closest equivalent.
    left = """
// There is a node here
first_node /*with cool comments, too */ param=1.03e2 /-"commented" "argument" {
    // With nested nodes too
    nested 1 2 3
    nested_2 "hi" "world" // this one is cool
}
second_node param=153 { nested one=1 two=2; }"""
    right = """
first_node param=103.0       "argument" {
        // Different indentation, because
        // Why not
        nested 1 2 3
        nested_2 "hi" /* actually, "hello" */ "world"
}
// There is a node here
second_node /* This time, the comment is here */ param=153 {
        nested one=1 two=2
}"""
    assert str(rkdl.parse(left)) != str(rkdl.parse(right))
    no_comments = rkdl.Formatter(no_comments=True)
    assert formatted(left, no_comments) == formatted(right, no_comments)


def test_document_basic_parsing():
    # document.rs: basic_parsing
    src = """
            // Hello, world!
            node 1
            node two
            node item="three";
            node {
                nested 1 2 3
                nested_2 hi "world"
            }
            (type)node ("type")what?
            +false #true
            null_id null_prop=#null
                    foo indented
            // normal comment?
            /- comment
            /* block comment */
            inline /*comment*/ here
            another /-comment there


            after some whitespace
            trailing /* multiline */
            trailing // single line
            """
    assert str(rkdl.parse(src)) == src


PARSING_SRC = """
// This is the first node
foo 1 2 three #null #true bar="baz" {
    - 1
    - 2
    - three
    (mytype)something ("name")else\r
}

null_id null_prop=#null
true_id true_prop=#null
+false #true

         bar "indented" // trailing whitespace after this\t
/*
Some random comment
 */

a; b; c;
/-commented "node"

another /*foo*/ "node" /-1 /*bar*/ #null;
final;"""


def test_document_parsing():
    # document.rs: parsing
    doc = rkdl.parse(PARSING_SRC)
    foo = doc["foo"]
    assert foo.args[0] == 1
    assert [n.args[0] for n in foo.get_all("-")] == [1, 2, "three"]
    assert foo.args[2] == "three"
    assert foo.props["bar"] == "baz"
    assert foo["something"].args[0] == "else"
    assert doc["another"].args[0] == "node"
    assert doc["null_id"].props["null_prop"] is None
    assert doc["true_id"].props["true_prop"] is None
    assert doc["+false"].args[0] is True
    assert str(doc) == PARSING_SRC


def test_document_parsing_node_display():
    # document.rs: parsing. str(node) drops the leading whitespace and comments and trims the end, like str(entry).
    doc = rkdl.parse(PARSING_SRC)
    assert str(doc["bar"]) == 'bar "indented" // trailing whitespace after this'
    assert str(doc["a"]) == "a;"
    assert str(doc["b"]) == "b;"


def test_document_parsing_append():
    # document.rs: parsing. kdl-rs keeps the hand-parsed entry's 0xDEADbeef; rkdl writes values in canonical form
    # on purpose, and has no way to build an entry from KDL text. kdl-rs also prints `final;new`; rkdl starts the new
    # node on its own line.
    doc = rkdl.parse(PARSING_SRC)
    doc.children.append(rkdl.Node("new", props={"blah": 0xDEADBEEF}))
    assert str(doc) == PARSING_SRC + "\nnew blah=3735928559\n"


def test_document_construction():
    # document.rs: construction. kdl-rs pushes each entry at the end, so its args land after `prop=value`. rkdl puts
    # a new arg after the last arg, and with no args yet that's before the props: args come first, as in most KDL.
    doc = rkdl.parse("")
    doc.children.append(rkdl.Node("foo"))
    bar = rkdl.Node("bar")
    bar.props["prop"] = "value"
    bar.args.append(1)
    bar.args.append(2)
    bar.args.append(False)
    bar.args.append(None)
    bar.children = []
    bar.children.append(rkdl.Node("barchild"))
    doc.children.append(bar)
    doc.children.append(rkdl.Node("baz"))
    doc.fmt()
    assert str(doc) == "foo\nbar 1 2 #false #null prop=value {\n    barchild\n}\nbaz\n"


def test_document_simple_autoformat():
    # document.rs: simple_autoformat
    assert formatted("a { b { c { }; }; }") == "a {\n    b {\n        c {\n\n        }\n    }\n}\n"


def test_document_simple_autoformat_two_spaces():
    # document.rs: simple_autoformat_two_spaces
    assert formatted("a { b { c { }; }; }", rkdl.Formatter(indent="  ")) == "a {\n  b {\n    c {\n\n    }\n  }\n}\n"


def test_document_simple_autoformat_single_tabs():
    # document.rs: simple_autoformat_single_tabs
    assert formatted("a { b { c { }; }; }", rkdl.Formatter(indent="\t")) == "a {\n\tb {\n\t\tc {\n\n\t\t}\n\t}\n}\n"


def test_document_simple_autoformat_no_comments():
    # document.rs: simple_autoformat_no_comments
    src = "// a comment\na {\n// another comment\n b { c { // another comment\n }; }; }"
    assert formatted(src, rkdl.Formatter(no_comments=True)) == "a {\n    b {\n        c {\n\n        }\n    }\n}\n"


@pytest.mark.parametrize(
    "name",
    [
        "kdl-schema.kdl",
        "Cargo.kdl",
        "ci.kdl",
        "nuget.kdl",
        "website.kdl",
        "zellij.kdl",
        "zellij-unquoted-bindings.kdl",
    ],
)
def test_document_parse_examples(name):
    # document.rs: parse_examples
    if not have_kdl_rs():
        pytest.skip(MISSING_KDL_RS)
    text = (EXAMPLES / name).read_text(encoding="utf-8")
    assert str(rkdl.parse(text)) == text


# node.rs


def test_node_canonical_clear_fmt():
    # node.rs: canonical_clear_fmt. See test_document_canonical_clear_fmt.
    left = """node /-"commented" param_name=103.000 {
            // This is a nested node
            nested 1 2 3
        }"""
    right = "node param_name=103.0 { nested 1 2 3; }"
    assert str(rkdl.parse(left)) != str(rkdl.parse(right))
    no_comments = rkdl.Formatter(no_comments=True)
    assert formatted(left, no_comments) == formatted(right, no_comments)


def test_node_parsing():
    # node.rs: parsing
    src = '\n\t  ("ty")"node" 0xDEADbeef;\n'
    doc = rkdl.parse(src)
    node = doc.children[0]
    assert node.type == "ty"
    assert node.name == "node"
    assert node.args[0] == 0xDEADBEEF
    assert str(node.args.entry(0)) == "0xDEADbeef"
    assert str(doc) == src

    doc = rkdl.parse('node test {\n    link "blah" anything=self\n}\n')
    node = doc["node"]
    assert node.args[0] == "test"
    assert len(node.children) == 1


def test_node_indexing():
    # node.rs: indexing
    node = rkdl.Node("foo")
    node.args.append("bar")
    node.props["foo"] = 1
    assert node.args[0] == "bar"
    assert node.props["foo"] == 1
    node.args[0] = False
    node.props["foo"] = None
    assert node.args[0] is False
    assert node.props["foo"] is None
    # kdl-rs pushes two x props by hand; from Python a duplicate key only comes from parsing.
    assert rkdl.parse("n x=1 x=2").children[0].props["x"] == 2


def test_node_insertion():
    # node.rs: insertion
    node = rkdl.Node("foo")
    node.args.append("pos0")
    node.props["keyword"] = 6.0
    node.args.append("pos1")
    assert len(node.entries) == 3
    node.args.insert(0, "inserted0")
    node.args.insert(2, "inserted1")
    assert len(node.entries) == 5
    assert list(node.args) == ["inserted0", "pos0", "inserted1", "pos1"]


def test_node_removal():
    # node.rs: removal
    node = rkdl.Node("foo")
    node.args.append("pos0")
    node.props["keyword"] = 6.0
    node.args.append("pos1")
    assert len(node.entries) == 3
    del node.args[1]
    assert len(node.entries) == 2
    assert "keyword" in node.props
    assert node.props.pop("not an existing keyword", None) is None
    assert len(node.entries) == 2
    del node.props["keyword"]
    assert len(node.entries) == 1
    del node.args[0]
    assert len(node.entries) == 0


def test_node_removal_of_parsed_prop():
    # node.rs: removal. kdl-rs's remove("keyword") misses a parsed key because it compares spellings; rkdl's del
    # finds it on purpose.
    doc = rkdl.parse("foo pos0 keyword=6.0 pos1\n")
    del doc["foo"].props["keyword"]
    assert str(doc) == "foo pos0 pos1\n"


def test_node_remove_panic():
    # node.rs: remove_panic. kdl-rs panics; rkdl raises IndexError instead.
    node = rkdl.Node("foo")
    node.args.append("pos0")
    node.props["keyword"] = 6.0
    del node.args[0]
    assert len(node.entries) == 1
    with pytest.raises(IndexError):
        del node.args[0]


# entry.rs


def test_entry_reset_value_repr():
    # entry.rs: reset_value_repr. clear_format isn't exposed; writing a value back resets its spelling.
    left = rkdl.parse("n   name=1.03e2")
    right = rkdl.parse("n   name=103.0")
    assert str(left) != str(right)
    for doc in (left, right):
        node = doc.children[0]
        node.props["name"] = node.props["name"]
    assert str(left) == str(right) == "n   name=103.0"


def test_entry_parsing():
    # entry.rs: parsing
    node = rkdl.parse("n foo").children[0]
    entry = node.entries[0]
    assert (entry.name, entry.value, entry.type) == (None, "foo", None)
    assert str(entry) == "foo"

    node = rkdl.parse("n foo=bar").children[0]
    entry = node.entries[0]
    assert (entry.name, entry.value, entry.type) == ("foo", "bar", None)
    assert str(entry) == "foo=bar"

    src = 'n \\\n ("m\\"eh")0xDEADbeef\t\\\n'
    doc = rkdl.parse(src)
    entry = doc.children[0].entries[0]
    assert (entry.name, entry.value, entry.type) == (None, 0xDEADBEEF, 'm"eh')
    assert str(doc) == src

    src = 'n \\\n "foo"=("m\\"eh")0xDEADbeef\t\\\n'
    doc = rkdl.parse(src)
    entry = doc.children[0].entries[0]
    assert (entry.name, entry.value, entry.type) == ("foo", 0xDEADBEEF, 'm"eh')
    assert str(doc) == src


def test_entry_display():
    # entry.rs: display
    node = rkdl.Node("n", 42, props={"name": 42})
    assert str(node.args.entry(0)) == "42"
    assert str(node.props.entry("name")) == "name=42"


# identifier.rs


def test_identifier_parsing():
    # identifier.rs: parsing, with identifiers in node-name position.
    assert rkdl.parse("foo").children[0].name == "foo"
    assert rkdl.parse('"foo\\"bar"').children[0].name == 'foo"bar'
    parse_error("123")
    parse_error('"x')


def test_identifier_formatting():
    # identifier.rs: formatting
    doc = rkdl.parse("")
    doc.children.append(rkdl.Node("foo"))
    doc.children.append(rkdl.Node('foo"bar'))
    assert str(doc) == 'foo\n"foo\\"bar"\n'


# value.rs


def test_value_plain_ident():
    # value.rs: plain_ident_test
    node = rkdl.Node("n", "foo123,bar", "foo123~!@$%^&*.:'|?+<>,")
    assert [str(e) for e in node.entries] == ["foo123,bar", "foo123~!@$%^&*.:'|?+<>,"]


def test_value_formatting():
    # value.rs: formatting
    node = rkdl.Node("n", "foo\n", 1234567890, 1234567890.12345, True, None)
    assert [str(e) for e in node.entries] == ['"foo\\n"', "1234567890", "1234567890.12345", "#true", "#null"]


# fmt.rs


def test_fmt_builder():
    # fmt.rs: builder. Formatter's keyword arguments stand in for FormatConfig's builder.
    f = rkdl.Formatter(indent_level=12, indent=" \t", no_comments=True)
    assert (f.indent_level, f.indent, f.no_comments, f.entry_autoformat_keep) == (12, " \t", True, False)


# v2_parser.rs


def test_parser_node():
    # v2_parser.rs: test_node
    node = rkdl.parse("foo").children[0]
    assert (node.name, node.type, list(node.entries), node.children) == ("foo", None, [], None)
    doc = rkdl.parse("foo bar")
    node = doc.children[0]
    assert (node.name, list(node.args), node.children) == ("foo", ["bar"], None)
    assert str(doc) == "foo bar"


@pytest.mark.parametrize(
    ("text", "name", "value"),
    [
        ("foo=bar", "foo", "bar"),
        ("foo", None, "foo"),
        ("/-foo bar", None, "bar"),
        ("/- foo=1 bar = 2", "bar", 2),
        ("/- \nfoo = 1 bar = 2", "bar", 2),
    ],
)
def test_parser_entry(text, name, value):
    # v2_parser.rs: entry_test
    doc = rkdl.parse(f"n {text}")
    entries = list(doc.children[0].entries)
    assert [(e.name, e.value) for e in entries] == [(name, value)]
    assert str(doc) == f"n {text}"


@pytest.mark.parametrize(
    ("text", "value"),
    [
        # identifier_string
        ("foo", "foo"),
        (",", ","),
        # single_line_quoted_string
        ('"foo"', "foo"),
        ('"foo\\u{0a}"', "foo\n"),
        ('"\\u{10FFFF}"', "\U0010ffff"),
        # multiline_quoted_string
        ('"""\nfoo\nbar\nbaz\n"""', "foo\nbar\nbaz"),
        ('"""\n  foo\n    bar\n  baz\n  """', "foo\n  bar\nbaz"),
        ('"""\nfoo\r\nbar\nbaz\n"""', "foo\nbar\nbaz"),
        ('"""\n  foo\n    bar\n   baz\n  """', "foo\n  bar\n baz"),
        ('"""\n  \\     foo\n    \\  bar\n   \\ baz\n  """', "foo\n  bar\n baz"),
        ('"""\n\n    string\t\n    """', "\nstring\t"),
        ('"""\n \t \\\n          \n \t """', ""),
        ('"""\n\\"""\n"""', '"""'),
        # raw_string
        ('#"foo"#', "foo"),
        # multiline_raw_string
        ('#"""\nfoo\nbar\nbaz\n"""#', "foo\nbar\nbaz"),
        ('#"""\nfoo\r\nbar\nbaz\n"""#', "foo\nbar\nbaz"),
        ('##"""\n  foo\n    bar\n  baz\n  """##', "foo\n  bar\nbaz"),
        ('#"""\n  foo\n    \\nbar\n   baz\n  """#', "foo\n  \\nbar\n baz"),
        # ident
        ("+.", "+."),
    ],
)
def test_parser_strings(text, value):
    # v2_parser.rs: string_tests
    assert only_arg(text) == value


@pytest.mark.parametrize(
    "text",
    [
        '"""\nfoo\n  bar\n  baz\n  """',
        '#"""#',
        '#"""\nfoo\n  bar\n  baz\n  """#',
        '#"\nfoo\nbar\nbaz\n"#',
        '"\nfoo\nbar\nbaz\n"',
    ],
)
def test_parser_bad_strings(text):
    # v2_parser.rs: string_tests
    parse_error(f"n {text}")


def test_parser_escline():
    # v2_parser.rs: escline_test
    assert len(rkdl.parse("foo bar\\\n   baz").children[0].entries) == 2


@pytest.mark.parametrize(
    "comment", ["/* foo */", "/**/", "/*\nfoo\n*/", "/*\nfoo*/", "/*foo\n*/", "/* foo\n*/", "/* /*bar*/ foo\n*/"]
)
def test_parser_multi_line_comment(comment):
    # v2_parser.rs: multi_line_comment_test
    text = f"n {comment} 1"
    doc = rkdl.parse(text)
    assert list(doc["n"].args) == [1]
    assert str(doc) == text


# kdl-rs's test runs its recovering parser and only checks for Ok; the public parse reports an error.
SLASHDASH_SEMICOLON = pytest.mark.xfail(
    strict=True,
    raises=rkdl.ParseError,
    reason="kdl-rs behaviour: a slashdashed top-level node followed by ';' fails with 'Found invalid node name'",
)


@pytest.mark.parametrize(
    ("text", "names"),
    [
        ("/- foo bar", []),
        pytest.param("/- foo bar;", [], marks=SLASHDASH_SEMICOLON),
        pytest.param("/-n 1;", [], marks=SLASHDASH_SEMICOLON),
        ("/- foo\nbar baz", ["bar"]),
        ("n /-commented tada", ["n"]),
        # before_node_children_test, around_children_test
        ("foo /- { }", ["foo"]),
        ("foo /- { bar }", ["foo"]),
        ("/- foo bar\nnode /-1 2 { x }", ["node"]),
        ("/- foo bar\nnode 2 /-3 { x }", ["node"]),
        ("/- foo bar\nnode /-1 2 /-3 { x }", ["node"]),
    ],
)
def test_parser_slashdash(text, names):
    # v2_parser.rs: slashdash_tests
    doc = rkdl.parse(text)
    assert [n.name for n in doc.children] == names
    assert str(doc) == text


@pytest.mark.parametrize(
    ("text", "value"),
    [
        # float_test
        ("12_34.56", 1234.56),
        ("1234_.56", 1234.56),
        ("2.5", 2.5),
        # decimal_test
        ("12_34", 1234),
        ("1234_", 1234),
        # test_hex
        ("0xdead_beef123", 0xDEADBEEF123),
        ("0xDeAd_BeEf123", 0xDEADBEEF123),
        ("0xdeadbeef123_", 0xDEADBEEF123),
        # test_octal
        ("0o12_34", 0o1234),
        ("0o1234_", 0o1234),
        # test_binary
        ("0b10_01", 0b1001),
        ("0b1001_", 0b1001),
    ],
)
def test_parser_numbers(text, value):
    # v2_parser.rs: float_test, decimal_test, test_hex, test_octal, test_binary
    result = only_arg(text)
    assert result == value
    assert type(result) is type(value)


@pytest.mark.parametrize(
    "text", ["1234a.56", "1234a", "0xABCDEF0123456789abcdef0123456789", "0x_deadbeef123", "0xbeefg1", "0o89", "0b12389"]
)
def test_parser_bad_numbers(text):
    # v2_parser.rs: float_test, decimal_test, test_hex, test_octal, test_binary
    parse_error(f"n {text}")


@pytest.mark.parametrize("text", ["_1234.56", "_1234", "0o_12_34", "0b_10_01"])
def test_parser_underscore_prefixed_numbers(text):
    # v2_parser.rs: float_test, decimal_test, test_octal, test_binary. The number sub-parsers reject these; in a
    # whole document a leading underscore makes a bare identifier, and 0o_/0b_ fail as numbers.
    if text.startswith("_"):
        assert only_arg(text) == text
    else:
        parse_error(f"n {text}")


TERMINATOR_HELP = (
    "A valid value was partially parsed, but was not followed by a value terminator. Did you want a space here?"
)
NAME_HELP = (
    "This can be any string type, including a quoted, raw, or multiline string, as well as a plain identifier string."
)
CHILD_HELP = "Did you forget to add the node name itself? Or perhaps terminated the node before its child block?"


def test_failure_bad_node_name():
    # v2_parser.rs: failure_tests::bad_node_name_test
    assert diagnostics("foo { bar; { baz; }; }") == [
        ("Found child block instead of node name", "not node name", CHILD_HELP, 11, 1),
    ]
    assert diagnostics("no/de 1 {\n    1 2 foo\n    bad#\n}") == [
        ("Expected identifier string", "not identifier string", TERMINATOR_HELP, 0, 5),
        ("Found invalid node name", "not node name", NAME_HELP, 0, 5),
        ("Found invalid node name", "not node name", NAME_HELP, 14, 1),
        ("Expected identifier string", "not identifier string", TERMINATOR_HELP, 26, 4),
        ("Found invalid node name", "not node name", NAME_HELP, 26, 4),
    ]


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("node 1asdf 2", ("Expected integer", "not integer", TERMINATOR_HELP, 5, 5)),
        ("node 0x1asdf 2", ("Expected hexadecimal number", "not hexadecimal number", TERMINATOR_HELP, 5, 7)),
        ("node 0o1asdf 2", ("Expected octal number", "not octal number", TERMINATOR_HELP, 5, 7)),
        ("node 0b1asdf 2", ("Expected binary number", "not binary number", TERMINATOR_HELP, 5, 7)),
        ("node 1.0asdf 2", ("Expected float", "not float", TERMINATOR_HELP, 5, 7)),
        ("node 1.asdf 2", ("Non-digit character found after the '.' of a float", "not float", None, 5, 6)),
        (
            "node 1.0easdf 2",
            (
                "Non-digit character found in the exponent part of a float",
                "not float",
                "Floats with exponent parts should look like '2.0e123', or '43.3E-4'.",
                5,
                8,
            ),
        ),
    ],
)
def test_failure_bad_entry_number(text, expected):
    # v2_parser.rs: failure_tests::bad_entry_number_test
    assert diagnostics(text) == [expected]


def test_failure_bad_string():
    # v2_parser.rs: failure_tests::bad_string_test
    assert diagnostics('node " 1') == [("Expected quoted string", "not quoted string", None, 5, 3)]
    assert diagnostics('node "foo"1') == [("Expected quoted string", "not quoted string", TERMINATOR_HELP, 5, 6)]
    assert diagnostics('node "\nlet\'s do multiline!"') == [
        (
            "Unexpected newline in single-line quoted string",
            "not quoted string",
            (
                'You can make a string multi-line by wrapping it in \'"""\', with a newline immediately after the '
                "opening quotes."
            ),
            5,
            1,
        ),
        ("Expected identifier string", "not identifier string", TERMINATOR_HELP, 16, 11),
    ]


def test_failure_bad_child():
    # v2_parser.rs: failure_tests::bad_child_test
    assert diagnostics("node {") == [("No closing '}' for child block", "not closed", None, 5, 1)]
    assert diagnostics("node {}}") == [("Expected end of document", "not EOF", None, 7, 1)]
    assert diagnostics("node }{") == [
        ("Expected end of document", "not EOF", None, 5, 1),
        ("Found child block instead of node name", "not node name", CHILD_HELP, 6, 1),
        ("No closing '}' for child block", "not closed", None, 6, 1),
    ]
    assert diagnostics("node {\n") == [
        ("No closing '}' for child block", "not closed", None, 5, 1),
        ("Closing '}' was not found after nodes", "not closed", None, 6, 1),
    ]
    assert diagnostics("node {\nnode2{{}}") == [
        ("Found child block instead of node name", "not node name", CHILD_HELP, 13, 1),
        ("No closing '}' for child block", "not closed", None, 5, 1),
        ("Closing '}' was not found after nodes", "not closed", None, 6, 10),
    ]


def test_failure_position_is_first_diagnostic():
    # v2_parser.rs: failure_tests. ParseError's own position is the first diagnostic's; its message is a rendered
    # report rather than kdl-rs's "Failed to parse KDL document", on purpose.
    err = parse_error("node 1asdf 2")
    assert (err.line, err.column, err.offset) == (1, 6, 5)
    assert "Expected integer" in str(err)


# tests/formatting.rs


def test_formatting_build_and_format():
    # formatting.rs: build_and_format
    c = rkdl.Node("c")
    c.children = []
    b = rkdl.Node("b")
    b.children = []
    b.children.append(c)
    a = rkdl.Node("a")
    a.children = []
    a.children.append(b)
    doc = rkdl.parse("")
    doc.children.append(a)
    doc.fmt()
    assert str(doc) == "a {\n    b {\n        c {\n\n        }\n    }\n}\n"


def test_formatting_fresh_nested_nodes_with_custom_indent():
    # formatting.rs: format_fresh_nested_nodes_with_custom_indent
    child = rkdl.Node("child")
    child.children = []
    child.children.append(rkdl.Node("innermost"))
    parent = rkdl.Node("parent")
    parent.children = []
    parent.children.append(child)
    doc = rkdl.parse("")
    doc.children.append(parent)
    doc.fmt(rkdl.Formatter(indent="  "))
    assert str(doc) == "parent {\n  child {\n    innermost\n  }\n}\n"


@pytest.mark.xfail(
    strict=True, reason="kdl-rs behaviour: kdl 6.7.1 drops the spaces inside and after the type annotation"
)
def test_formatting_node_ty():
    # formatting.rs: format_node_ty
    src = "( the-type ) the-node-name"
    assert str(rkdl.parse(src)) == src
