"""`line` and `column` on nodes and entries, and `doc.path`."""

import copy
import io

import pytest
import rkdl


def at(thing):
    return (thing.line, thing.column)


TEXT = """\
// header
server "web" port=8080 {
    (svc)listen  (u16)80 host=(ip)"0.0.0.0"
    "quoted name" #true
}
last
"""


# Where things are


def test_nodes_point_at_the_annotation_or_the_name():
    doc = rkdl.parse(TEXT)
    server = doc["server"]
    assert at(server) == (2, 1)
    assert at(server["listen"]) == (3, 5)
    assert at(server["quoted name"]) == (4, 5)
    assert at(doc["last"]) == (6, 1)


def test_entries_point_at_their_first_character():
    doc = rkdl.parse(TEXT)
    assert [at(e) for e in doc["server"].entries] == [(2, 8), (2, 14)]
    # An argument's annotation, then a property's key even when its value has one.
    assert [at(e) for e in doc["server"]["listen"].entries] == [(3, 18), (3, 26)]
    assert at(doc["server"]["quoted name"].args.entry(0)) == (4, 19)


def test_annotation_with_spaces_inside():
    doc = rkdl.parse("a; ( t )b ( u ) 1")
    assert at(doc["b"]) == (1, 4)
    assert at(doc["b"].args.entry(0)) == (1, 11)


def test_after_slashdashes():
    text = "/- gone 1 {\n    x\n}\nkept /- 1 2 /- k=3 v=4 /- {\n    y\n} {\n    /- z\n    child\n}\n"
    doc = rkdl.parse(text)
    kept = doc["kept"]
    assert at(kept) == (4, 1)
    assert [at(e) for e in kept.entries] == [(4, 11), (4, 20)]
    assert at(kept["child"]) == (8, 5)


def test_after_multi_line_strings():
    text = 'a """\n    one\n    two\n    """ b=1\nc #"""\n  x\n  """# 2\n'
    doc = rkdl.parse(text)
    assert [at(e) for e in doc["a"].entries] == [(1, 3), (4, 9)]
    assert at(doc["c"]) == (5, 1)
    assert [at(e) for e in doc["c"].entries] == [(5, 3), (7, 8)]


@pytest.mark.parametrize("eol", ["\n", "\r\n", "\r", " ", "\x85"])
def test_every_kdl_line_ending(eol):
    doc = rkdl.parse(f"a{eol}{eol}  b 1{eol}c")
    assert at(doc["a"]) == (1, 1)
    assert at(doc["b"]) == (3, 3)
    assert at(doc["b"].args.entry(0)) == (3, 5)
    assert at(doc["c"]) == (4, 1)


def test_columns_count_characters():
    # A tab is one character, and so are é and 日.
    doc = rkdl.parse('\ta "é日" x\n日本 y=1\n')
    assert at(doc["a"]) == (1, 2)
    assert [at(e) for e in doc["a"].entries] == [(1, 4), (1, 9)]
    assert at(doc["日本"]) == (2, 1)
    assert at(doc["日本"].props.entry("y")) == (2, 4)


def test_byte_order_mark_is_not_a_column():
    doc = rkdl.parse("﻿a 1\n")
    assert at(doc["a"]) == (1, 1)
    assert at(doc["a"].args.entry(0)) == (1, 3)


def test_after_text_following_a_closing_brace():
    # kdl-rs drops what follows a block's `}`, and rkdl puts it back. The positions after it are still right.
    text = "a {} // c\r\nb { c {} } ; d 1\np {\n    q {} /- { }  \n    s\n}\n"
    doc = rkdl.parse(text)
    assert str(doc) == text
    assert at(doc["b"]) == (2, 1)
    assert at(doc["b"]["c"]) == (2, 5)
    assert at(doc["d"]) == (2, 14)
    assert at(doc["d"].args.entry(0)) == (2, 16)
    assert at(doc["p"]["s"]) == (5, 5)


def test_positions_are_from_before_formatting():
    doc = rkdl.Parser(formatter=rkdl.Formatter()).parse("a   1 {  b  }\n")
    assert str(doc) != "a   1 {  b  }\n"
    assert at(doc["a"].args.entry(0)) == (1, 5)
    assert at(doc["a"]["b"]) == (1, 10)


# Edits don't move them


def test_edits_leave_positions_alone():
    doc = rkdl.parse("a 1 k=2\nb\n")
    a = doc["a"]
    doc.children.insert(0, rkdl.Node("first"))
    a.name = "renamed"
    a.type = "t"
    a.args.insert(0, 0)
    assert at(a) == (1, 1)
    assert at(a.args.entry(1)) == (1, 3)
    assert at(doc["b"]) == (2, 1)


def test_writing_a_value_keeps_the_entry_position():
    doc = rkdl.parse("a 1 k=2\n")
    a = doc["a"]
    a.args[0] = "one"
    a.props["k"] = 3.5
    a.props.entry("k").value = "x"
    assert at(a.args.entry(0)) == (1, 3)
    assert at(a.props.entry("k")) == (1, 5)


def test_a_renamed_property_keeps_its_position():
    doc = rkdl.parse("a k=2\n")
    entry = doc["a"].props.entry("k")
    entry.name = "j"
    assert at(entry) == (1, 3)


# What has none


def test_built_nodes_have_none():
    node = rkdl.Node("n", 1, k=2)
    assert at(node) == (None, None)
    assert [at(e) for e in node.entries] == [(None, None)] * 2
    assert at(rkdl.parse("").node("n")) == (None, None)


def test_added_entries_have_none():
    doc = rkdl.parse("a 1\n")
    a = doc["a"]
    a.args.append(2)
    a.props["k"] = 3
    assert [at(e) for e in a.entries] == [(1, 3), (None, None), (None, None)]


def test_a_node_added_to_a_parsed_document_has_none():
    doc = rkdl.parse("a\n")
    doc.children.append(rkdl.Node("b", 1))
    assert at(doc["b"]) == (None, None)
    assert at(doc["b"].args.entry(0)) == (None, None)


@pytest.mark.parametrize("make", [lambda n: n.copy(), copy.copy, copy.deepcopy])
def test_node_copies_have_none(make):
    doc = rkdl.parse("a 1 {\n    b\n}\n")
    copied = make(doc["a"])
    assert at(copied) == (None, None)
    assert at(copied.args.entry(0)) == (None, None)
    assert at(copied["b"]) == (None, None)


def test_document_copies_have_none():
    doc = rkdl.parse("a 1\n")
    copied = doc.copy()
    assert at(copied["a"]) == (None, None)
    assert at(copied["a"].args.entry(0)) == (None, None)
    assert at(doc["a"]) == (1, 1)


def test_a_document_built_empty_has_none():
    doc = rkdl.Document()
    doc.children.append(rkdl.Node("a"))
    assert at(doc["a"]) == (None, None)


def test_a_removed_entry_raises():
    doc = rkdl.parse("a 1\n")
    entry = doc["a"].args.entry(0)
    del doc["a"].args[0]
    with pytest.raises(RuntimeError, match="removed"):
        _ = entry.line


# Moves


def test_moving_within_the_document_keeps_positions():
    doc = rkdl.parse("a 1\np {\n    q x=1 {\n        r\n    }\n}\n")
    a, q = doc["a"], doc["p"]["q"]
    doc.children.append(doc.children.pop(0))
    doc["a"].children = [doc["p"].children.pop(0)]
    doc.children.reverse()
    doc.children[:] = [doc.children[1], doc.children[0]]
    doc.children[0:1] = [doc.children[0], rkdl.Node("new")]
    assert at(a) == (1, 1)
    assert at(a.args.entry(0)) == (1, 3)
    assert at(q) == (3, 5)
    assert at(q.props.entry("x")) == (3, 7)
    assert at(q["r"]) == (4, 9)


def test_a_detached_node_keeps_positions():
    doc = rkdl.parse("a\np 1 {\n    q\n}\n")
    p = doc["p"].detach()
    assert at(p) == (2, 1)
    assert at(p.args.entry(0)) == (2, 3)
    assert at(p["q"]) == (3, 5)
    removed = doc.children.pop()
    assert at(removed) == (1, 1)


def test_moving_back_into_the_same_document_keeps_positions():
    doc = rkdl.parse("a\np 1 {\n    q\n}\n")
    p = doc.children.pop(1)
    doc["a"].children = [p]
    assert at(p) == (2, 1)
    assert at(p["q"]) == (3, 5)
    assert at(p.args.entry(0)) == (2, 3)


def test_moving_into_another_document_drops_positions():
    doc = rkdl.parse("a\np 1 {\n    q\n}\n")
    other = rkdl.parse("x\n")
    p = doc.children.pop(1)
    q = p["q"]
    other.children.append(p)
    assert at(p) == (None, None)
    assert at(p.args.entry(0)) == (None, None)
    assert at(q) == (None, None)
    assert at(other["x"]) == (1, 1)
    # They belonged to the first document's file, and coming back doesn't bring them back.
    doc.children.append(other.children.pop())
    assert at(p) == (None, None)


def test_moving_into_a_built_node_drops_positions():
    doc = rkdl.parse("a\np\n")
    holder = rkdl.Node("holder", children=[doc.children.pop()])
    assert at(holder["p"]) == (None, None)


def test_moving_into_a_detached_node_of_the_same_document_keeps_positions():
    doc = rkdl.parse("a {\n}\np\n")
    a, p = doc.children.pop(0), doc.children.pop(0)
    a.children.append(p)
    assert at(p) == (3, 1)
    doc.children.append(a)
    assert at(a["p"]) == (3, 1)


def test_two_parses_of_the_same_text_are_different_files():
    one, two = rkdl.parse("a\nb\n"), rkdl.parse("a\nb\n")
    b = one.children.pop()
    two.children.insert(0, b)
    assert at(b) == (None, None)


# doc.path


def test_parsed_text_has_no_path():
    assert rkdl.parse("a").path is None
    assert rkdl.Parser().parse("a").path is None
    assert rkdl.Document().path is None


def test_load_from_a_path_records_it(tmp_path):
    file = tmp_path / "config.kdl"
    file.write_text("a\n  b 1\n")
    doc = rkdl.load(file)
    assert doc.path == str(file)
    b = doc["b"]
    assert f"{doc.path}:{b.line}:{b.column}" == f"{file}:2:3"
    assert doc.copy().path is None


def test_load_from_a_named_file_records_its_name(tmp_path):
    file = tmp_path / "config.kdl"
    file.write_text("a\n")
    with open(file, "rb") as f:
        assert rkdl.load(f).path == str(file)
    assert rkdl.load(io.StringIO("a\n")).path is None
