import collections.abc
import gc
import weakref

import pytest
import rkdl

TEXT = """\
// header
a 1
b {
    c
    // about d
    d
}
e {}
"""


def doc():
    return rkdl.parse(TEXT)


# Identity and the cache


def test_lookup_returns_the_same_object():
    d = doc()
    assert d["b"] is d["b"]
    assert d["b"]["c"] is d.children[1].children[0]
    assert d["a"] == d.children[0]
    assert d["a"] != d["b"]
    assert hash(d["a"]) == hash(d.children[0])


def test_handles_are_not_kept_alive():
    d = doc()
    ref = weakref.ref(d["a"])
    gc.collect()
    assert ref() is None
    assert d["a"] is d["a"]
    assert d["a"].name == "a"


def test_identity_after_detaching():
    d = doc()
    c = d["b"]["c"]
    popped = d["b"].children.pop(0)
    assert popped is c
    assert c.name == "c"
    assert "c" not in [n.name for n in d["b"].children]


def test_nodes_in_a_moved_subtree_keep_their_handles():
    d = doc()
    b = d["b"]
    c, dd = b["c"], b["d"]
    d.children.remove(b)
    assert b["c"] is c
    other = rkdl.parse("x\n")
    other.children.append(b)
    assert other["b"] is b
    assert other["b"]["d"] is dd


def test_repr():
    d = rkdl.parse('server\n"my node"\n')
    assert repr(d["server"]) == "<Node server>"
    assert repr(d["my node"]) == '<Node "my node">'


def test_children_is_a_mutable_sequence():
    assert isinstance(doc().children, collections.abc.MutableSequence)


# Constructor and attributes


def test_constructor():
    d = rkdl.parse("")
    d.children.append(rkdl.Node("server", "web", 2, type="svc", port=8080, props={"quoted key": True}))
    assert str(d) == '(svc)server web 2 "quoted key"=#true port=8080\n'


def test_name_and_type():
    d = rkdl.parse('(old)"q" 1\n')
    n = d["q"]
    assert n.type == "old"
    n.name = "renamed"
    n.type = "new"
    # A quoted name stays quoted.
    assert str(d) == '(new)"renamed" 1\n'
    n.type = None
    n.name = "two words"
    assert n.type is None
    assert str(d) == '"two words" 1\n'


@pytest.mark.parametrize(
    "text",
    [
        "(dev)  dep 1\n",
        "(dev)/*hey*/dep 1\n",
        "( the-type ) the-node-name\n",
        '(\t"a type" /* c */ )\t"a name"\n',
        "a { (t)  b }\n",
        "a {\n    ( t ) \\\n    b\n}\n",
        "(u8)  n (u8)  7\n",
    ],
)
def test_text_around_a_node_annotation_survives(text):
    assert str(rkdl.parse(text)) == text


def test_text_around_a_node_annotation_through_edits():
    doc = rkdl.parse('( old ) /* c */ "q" 1\n')
    node = doc["q"]
    # A new annotation or name keeps the text around the annotation, and quoting keeps its style.
    node.type = "new"
    node.name = "renamed"
    assert str(doc) == '( new ) /* c */ "renamed" 1\n'
    assert node.type == "new"
    assert node.name == "renamed"
    copied = node.copy()
    assert str(copied) == '( new ) /* c */ "renamed" 1'
    # Removing the annotation takes the text inside it and between it and the name.
    node.type = None
    assert str(doc) == '"renamed" 1\n'
    node.type = "back"
    assert str(doc) == '(back)"renamed" 1\n'
    # The copy is untouched.
    assert copied.type == "new"
    assert str(copied) == '( new ) /* c */ "renamed" 1'


def test_text_around_a_node_annotation_moves_with_the_node():
    doc = rkdl.parse("a {\r\n    ( t ) \\\r\n    b 1\r\n}\r\n")
    node = doc["a"].children.pop(0)
    other = rkdl.parse("z\n")
    other.children.append(node)
    # Line endings follow the new document. Like a continuation between entries, the next line keeps its indent.
    assert str(other) == "z\n( t ) \\\n    b 1\n"
    assert other["b"].type == "t"


def test_fmt_drops_the_text_around_a_node_annotation():
    doc = rkdl.parse("( t ) /* c */ n 1\n")
    doc.fmt()
    assert str(doc) == "(t)n 1\n"
    assert str(rkdl.parse(str(doc))) == str(doc)


def test_constructor_rejects_unconvertible_values():
    with pytest.raises(TypeError):
        rkdl.Node("n", object())


# Inserting


@pytest.mark.parametrize(
    ("index", "expected"),
    [
        (0, "b {\n    new\n    c\n    // about d\n    d\n}\n"),
        (1, "b {\n    c\n    new\n    // about d\n    d\n}\n"),
        (2, "b {\n    c\n    // about d\n    d\n    new\n}\n"),
        (-1, "b {\n    c\n    new\n    // about d\n    d\n}\n"),
    ],
)
def test_insert_positions(index, expected):
    d = doc()
    d["b"].children.insert(index, rkdl.Node("new"))
    assert str(d) == "// header\na 1\n" + expected + "e {}\n"


def test_append_to_empty_block():
    d = doc()
    d["e"].children.append(rkdl.Node("new"))
    assert str(d) == "// header\na 1\nb {\n    c\n    // about d\n    d\n}\ne {\n    new\n}\n"


def test_insert_top_level():
    d = doc()
    d.children.insert(0, rkdl.Node("first"))
    d.children.insert(2, rkdl.Node("middle"))
    d.children.append(rkdl.Node("last"))
    assert str(d) == "first\n// header\na 1\nmiddle\nb {\n    c\n    // about d\n    d\n}\ne {}\nlast\n"


def test_insert_into_empty_document():
    d = rkdl.parse("")
    d.children.append(rkdl.Node("a"))
    assert str(d) == "a\n"


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("// hi", "// hi\nnew\n"),
        ("/* x */", "/* x */\nnew\n"),
        ("// hi\n", "// hi\nnew\n"),
        ("/* x */\n", "/* x */\nnew\n"),
    ],
)
def test_insert_after_a_final_comment_starts_a_new_line(text, expected):
    d = rkdl.parse(text)
    d.children.append(rkdl.Node("new"))
    assert str(d) == expected
    assert [n.name for n in rkdl.parse(str(d)).children] == ["new"]


def test_nested_three_deep():
    d = rkdl.parse("top {\n    mid {\n        low {\n        }\n    }\n}\n")
    d["top"]["mid"]["low"].children.append(rkdl.Node("leaf", 1))
    assert str(d) == "top {\n    mid {\n        low {\n            leaf 1\n        }\n    }\n}\n"


def test_built_subtree_lines_up_where_it_lands():
    outer = rkdl.Node("outer")
    outer.children = []
    inner = rkdl.Node("inner")
    inner.children = []
    inner.children.append(rkdl.Node("leaf"))
    outer.children.append(inner)
    d = rkdl.parse("top {\n    mid {\n    }\n}\n")
    d["top"]["mid"].children.append(outer)
    assert str(d) == (
        "top {\n    mid {\n        outer {\n            inner {\n                leaf\n            }\n        }\n    }\n}\n"
    )


def test_indent_follows_the_document():
    d = rkdl.parse("a {\n\tb {\n\t}\n}\nc {}\n")
    d["a"]["b"].children.append(rkdl.Node("x"))
    d["c"].children.append(rkdl.Node("y"))
    assert str(d) == "a {\n\tb {\n\t\tx\n\t}\n}\nc {\n\ty\n}\n"


def test_insert_after_a_last_node_without_newline():
    d = rkdl.parse("a\nb")
    d.children.append(rkdl.Node("c"))
    assert str(d) == "a\nb\nc\n"


def test_new_block_goes_after_trailing_slashdashed_entries():
    d = rkdl.parse("node /- key=value\n")
    d["node"].children = []
    d["node"].children.append(rkdl.Node("added", 1))
    assert str(d) == "node /- key=value {\n    added 1\n}\n"
    d = rkdl.parse("node {a} /- {b}\nx\n")
    d["node"].children.append(rkdl.Node("added", 1))
    assert str(d).endswith("} /- {b}\nx\n")


@pytest.mark.parametrize(
    ("text", "parent", "expected"),
    [
        ("node1;", None, "node1;\nadded 1\n"),
        ("node1 {\n      node2;\n}", "node1", "node1 {\n      node2;\n      added 1\n}"),
    ],
)
def test_insert_after_semicolon_starts_a_new_line(text, parent, expected):
    d = rkdl.parse(text)
    (d[parent] if parent else d).children.append(rkdl.Node("added", 1))
    assert str(d) == expected


def test_insert_after_trailing_line_continuation():
    d = rkdl.parse("node \\")
    d.children.append(rkdl.Node("added", 1))
    assert str(d) == "node \\\n;\nadded 1\n"
    assert [n.name for n in rkdl.parse(str(d)).children] == ["node", "added"]


CRLF = "a {\r\n    b\r\n    c\r\n}\r\nd {}\r\n"


@pytest.mark.parametrize(
    ("index", "expected"),
    [
        (0, "a {\r\n    x\r\n    b\r\n    c\r\n}\r\n"),
        (1, "a {\r\n    b\r\n    x\r\n    c\r\n}\r\n"),
        (2, "a {\r\n    b\r\n    c\r\n    x\r\n}\r\n"),
    ],
)
def test_insert_keeps_crlf_line_endings(index, expected):
    d = rkdl.parse(CRLF)
    d["a"].children.insert(index, rkdl.Node("x"))
    assert str(d) == expected + "d {}\r\n"


def test_append_into_empty_block_uses_crlf():
    d = rkdl.parse(CRLF)
    d["d"].children.append(rkdl.Node("x"))
    d.children.append(rkdl.Node("y"))
    assert str(d) == "a {\r\n    b\r\n    c\r\n}\r\nd {\r\n    x\r\n}\r\ny\r\n"


def test_moved_subtree_takes_the_line_endings_of_its_new_place():
    built = rkdl.Node("built")
    built.children = []
    built.children.append(rkdl.Node("inner"))
    d = rkdl.parse(CRLF)
    d["d"].children.append(built)
    assert str(d).endswith("d {\r\n    built {\r\n        inner\r\n    }\r\n}\r\n")


@pytest.mark.parametrize(
    ("index", "expected"),
    [
        (0, "a { x; b; c }\n"),
        (1, "a { b; x; c }\n"),
        (2, "a { b; c; x }\n"),
    ],
)
def test_insert_into_single_line_block(index, expected):
    d = rkdl.parse("a { b; c }\n")
    d["a"].children.insert(index, rkdl.Node("x"))
    assert str(d) == expected


def test_single_line_block_keeps_its_spacing():
    d = rkdl.parse("a {b}\n")
    d["a"].children.append(rkdl.Node("x", 1))
    assert str(d) == "a {b;x 1}\n"


def test_node_that_needs_lines_spreads_a_single_line_block():
    node = rkdl.parse("// about x\nx\n").children.pop()
    d = rkdl.parse("top {\n    a { b; c }\n}\n")
    d["top"]["a"].children.insert(1, node)
    assert str(d) == "top {\n    a {\n        b\n        // about x\n        x\n        c\n    }\n}\n"


@pytest.mark.parametrize("text", ["a { b }\n", "a { b; }\n"])
def test_replace_only_child_keeps_one_line_block(text):
    d = rkdl.parse(text)
    d["a"].children[0] = rkdl.Node("c")
    assert str(d) == text.replace("b", "c")


def test_moved_empty_multi_line_block_lines_up():
    node = rkdl.parse("q {\n}\n").children.pop()
    d = rkdl.parse("a {\n    b\n}\n")
    d["a"].children.append(node)
    assert str(d) == "a {\n    b\n    q {\n    }\n}\n"


@pytest.mark.parametrize("how", ["pop", "copy"])
def test_last_node_taken_from_a_one_line_block_has_no_trailing_space(how):
    d = rkdl.parse("a { b; c }\n")
    node = d["a"].children.pop() if how == "pop" else d["a"]["c"].copy()
    d.children.append(node)
    assert str(d).endswith("}\nc\n")


def test_block_on_a_node_with_trailing_space_reads_back():
    d = rkdl.parse("c \nd\n")
    d["c"].children = []
    # Whitespace right after `}` doesn't read back (space_after_node.kdl), so the space goes before `{`.
    assert str(d) == "c {}\nd\n"
    assert str(rkdl.parse(str(d))) == str(d)


def test_moved_line_continuation_takes_crlf():
    node = rkdl.parse("n \\\n    1\n").children.pop()
    d = rkdl.parse("a\r\n")
    d.children.append(node)
    assert str(d) == "a\r\nn \\\r\n    1\r\n"


def test_crlf_survives_emptying_the_document():
    d = rkdl.parse("a {\r\n    b\r\n}\r\n")
    node = d.children.pop()
    assert str(d) == ""
    d.children.append(node)
    assert str(d) == "a {\r\n    b\r\n}\r\n"


def test_block_at_the_end_of_a_one_line_block():
    d = rkdl.parse("a { b }\n")
    d["a"]["b"].children = []
    # The space before the parent's `}` can't follow b's `}` (space_after_node.kdl), so a `;` goes in between.
    assert str(d) == "a { b {}; }\n"
    assert str(rkdl.parse(str(d))) == str(d)


def test_moved_node_keeps_its_comment_and_reindents():
    d = doc()
    node = d["b"].children.pop(1)
    d.children.append(node)
    assert str(d) == "// header\na 1\nb {\n    c\n}\ne {}\n// about d\nd\n"
    node = d.children.pop()
    d["e"].children.append(node)
    assert str(d) == "// header\na 1\nb {\n    c\n}\ne {\n    // about d\n    d\n}\n"


def test_insert_attached_node_raises():
    d = doc()
    with pytest.raises(ValueError, match="already in a document"):
        d["e"].children.append(d["a"])
    other = rkdl.parse("x\n")
    with pytest.raises(ValueError):
        other.children.append(d["a"])
    assert str(d) == TEXT


def test_copy_then_insert():
    d = doc()
    copy = d["b"].copy()
    assert copy is not d["b"]
    assert copy != d["b"]
    assert copy["c"] != d["b"]["c"]
    d["e"].children.append(copy)
    assert str(d) == (
        "// header\na 1\nb {\n    c\n    // about d\n    d\n}\n"
        "e {\n    b {\n        c\n        // about d\n        d\n    }\n}\n"
    )


def test_insert_into_itself_raises():
    n = rkdl.Node("n")
    n.children = []
    with pytest.raises(ValueError, match="inside itself"):
        n.children.append(n)
    child = rkdl.Node("child")
    n.children.append(child)
    child.children = []
    with pytest.raises(ValueError):
        child.children.append(n)


def test_setitem_replaces():
    d = doc()
    d["b"].children[0] = rkdl.Node("new")
    assert str(d) == "// header\na 1\nb {\n    new\n    // about d\n    d\n}\ne {}\n"


def test_setitem_failures_keep_the_old_child():
    d = doc()
    c = d["b"]["c"]
    with pytest.raises(ValueError, match="already in a document"):
        d["b"].children[0] = d["a"]
    with pytest.raises(IndexError):
        d["b"].children[5] = rkdl.Node("new")
    assert d["b"].children[0] is c
    assert str(d) == TEXT


def test_setitem_detaches_the_old_child():
    d = doc()
    c = d["b"]["c"]
    d["b"].children[-2] = rkdl.Node("new")
    assert c not in d["b"].children
    other = rkdl.parse("")
    other.children.append(c)
    assert str(other) == "c\n"


# Removing


def test_pop_detaches_every_reference():
    d = doc()
    c = d["b"]["c"]
    same = d["b"].children[0]
    popped = d["b"].children.pop(0)
    assert popped is c is same
    # Detached, so it can go into another document.
    other = rkdl.parse("")
    other.children.append(same)
    assert str(other) == "c\n"
    assert other["c"] is c


def test_pop_first_child_keeps_newline_after_brace():
    d = doc()
    d["b"].children.pop(0)
    assert str(d) == "// header\na 1\nb {\n    // about d\n    d\n}\ne {}\n"


def test_removing_takes_leading_comment():
    d = doc()
    d["b"].children.remove(d["b"]["d"])
    assert str(d) == "// header\na 1\nb {\n    c\n}\ne {}\n"
    d.children.pop(0)
    assert str(d) == "b {\n    c\n}\ne {}\n"


def test_removing_last_child_empties_block():
    d = doc()
    del d["b"].children[:]
    assert str(d) == "// header\na 1\nb {}\ne {}\n"


def test_remove_keeps_crlf_line_endings():
    d = rkdl.parse(CRLF)
    d["a"].children.pop(0)
    assert str(d) == "a {\r\n    c\r\n}\r\nd {}\r\n"
    d["a"].children.pop()
    assert str(d) == "a {}\r\nd {}\r\n"


@pytest.mark.parametrize(
    ("index", "expected"),
    [
        (0, "a { c }\n"),
        # The `;` before stays, so the text before the removed node is untouched.
        (1, "a { b; }\n"),
    ],
)
def test_remove_from_single_line_block(index, expected):
    d = rkdl.parse("a { b; c }\n")
    d["a"].children.pop(index)
    assert str(d) == expected
    d["a"].children.pop()
    assert str(d) == "a {}\n"


def test_del_and_errors():
    d = doc()
    del d.children[1]
    assert str(d) == "// header\na 1\ne {}\n"
    with pytest.raises(IndexError):
        d.children.pop(5)
    with pytest.raises(IndexError):
        d.children[-3]
    with pytest.raises(ValueError):
        d.children.remove(rkdl.Node("stray"))
    with pytest.raises(ValueError):
        d.children.index(rkdl.Node("stray"))


def test_view_follows_node_and_block():
    d = doc()
    view = d["b"].children
    d.children.remove(d["b"])
    assert [n.name for n in view] == ["c", "d"]
    e_view = d["e"].children
    d["e"].children = None
    with pytest.raises(ValueError, match="no children block"):
        len(e_view)


# children = None / []


def test_children_none_detaches():
    d = doc()
    b = d["b"]
    c = b["c"]
    b.children = None
    assert b.children is None
    assert str(d) == "// header\na 1\nb\ne {}\n"
    d.children.append(c)
    assert str(d).endswith("e {}\nc\n")


def test_children_empty_list():
    d = doc()
    d["a"].children = []
    assert str(d) == "// header\na 1 {}\nb {\n    c\n    // about d\n    d\n}\ne {}\n"
    d["b"].children = []
    assert str(d) == "// header\na 1 {}\nb {}\ne {}\n"
    d["a"].children.append(rkdl.Node("x"))
    assert str(d) == "// header\na 1 {\n    x\n}\nb {}\ne {}\n"


# Only the text up to `{}` is checked: what fmt() leaves after it hits two kdl-rs bugs, the blank line after a
# trailing comment (test_fmt_leaves_no_blank_line_after_a_trailing_comment) and the dropped `// end`
# (test_fmt_keeps_a_line_comment_ending_a_node).
@pytest.mark.parametrize(
    ("text", "start"),
    [
        # fmt() ends the comment with a newline of its own, which mustn't land before `{`.
        ("a 1 /* end */\n", "a 1 /* end */ {}"),
        ("x {\n  a 1 /* end */\n}\n", "x {\n    a 1 /* end */ {}"),
        ("a /-b \\\n  /-c\n", "a /-b \\\n  /-c {}"),
        ("a /-{}\n", "a /-{} {}"),
        ("a 1 // end\n", "a 1 {}"),
    ],
)
def test_children_empty_list_after_fmt(text, start):
    d = rkdl.parse(text)
    d.fmt()
    (d["x"]["a"] if "x" in text else d["a"]).children = []
    assert str(d).startswith(start)
    assert str(rkdl.parse(str(d))) == str(d)


def test_children_assignment_errors():
    d = doc()
    before = str(d)
    with pytest.raises(TypeError):
        d["a"].children = 5
    with pytest.raises(TypeError):
        d["a"].children = [rkdl.Node("x"), 1]
    with pytest.raises(TypeError):
        d.children = None
    assert str(d) == before


def test_children_iadd():
    d = rkdl.parse("a { b; }\n")
    node = d["a"]
    node.children += [rkdl.Node("c")]
    d.children += [rkdl.Node("d")]
    assert str(d) == "a { b; c; }\nd\n"


def test_children_assigning_another_view_errors():
    d = rkdl.parse("a { b; }\ne { f; }\n")
    a = d["a"]
    e = d["e"]
    other = rkdl.parse("x\n")
    # Another view holds attached nodes, which have to be detached or copied first.
    with pytest.raises(ValueError, match="already in a document"):
        a.children = e.children
    with pytest.raises(ValueError, match="already in a document"):
        d.children = other.children
    with pytest.raises(ValueError, match="already in a document"):
        d.children = a.children
    assert str(d) == "a { b; }\ne { f; }\n"


# Lookups


def test_lookups():
    d = rkdl.parse("a 1\n(t)a 2\nb\n(u)a 3\n")
    assert d["a"] is d.children[0]
    assert d[("t", "a")] is d.children[1]
    assert d[(None, "a")] is d.children[0]
    assert d.get_all("a") == [d.children[0], d.children[1], d.children[3]]
    assert d.get_all(("u", "a")) == [d.children[3]]
    assert d.get("missing") is None
    assert d.get("missing", 5) == 5
    assert d.get_all("missing") == []
    with pytest.raises(KeyError):
        d["missing"]
    with pytest.raises(KeyError):
        d[("x", "a")]
    with pytest.raises(TypeError):
        d[0]


def test_node_lookups():
    d = doc()
    assert d["b"]["d"].name == "d"
    assert d["b"].get_all("c") == [d["b"].children[0]]
    assert d["a"].get("c") is None
    assert d["a"].get_all("c") == []
    with pytest.raises(KeyError):
        d["a"]["c"]


# Iteration


def test_editing_during_iteration():
    d = rkdl.parse("a\nb\nc\nd\n")
    seen = []
    for node in d.children:
        seen.append(node.name)
        if node.name in ("a", "c"):
            d.children.remove(node)
    assert seen == ["a", "b", "c", "d"]
    assert str(d) == "b\nd\n"


def test_slices():
    d = rkdl.parse("a\nb\nc\nd\n")
    assert [n.name for n in d.children[1:3]] == ["b", "c"]
    assert [n.name for n in d.children[::-2]] == ["d", "b"]
    del d.children[::2]
    assert str(d) == "b\nd\n"
    assert d["d"] in d.children
    assert d.children.index(d["d"]) == 1


def test_count():
    d = doc()
    assert d.children.count(d["a"]) == 1
    assert d.children.count(d["b"]["c"]) == 0
    assert d.children.count(rkdl.Node("a")) == 0
    assert d.children.count("a") == 0


def test_reversed():
    d = doc()
    assert list(reversed(d["b"].children)) == [d["b"]["d"], d["b"]["c"]]
    assert list(reversed(d["e"].children)) == []


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a\nb\nc", "c\nb\na"),
        ("n {\n    // about a\n    a\n    b\n    c\n}\n", "n {\n    c\n    b\n    // about a\n    a\n}\n"),
        ("n { a; b; c }\n", "n { c; b; a }\n"),
        ("n {\r\n  a\r\n  b\r\n}\r\n", "n {\r\n  b\r\n  a\r\n}\r\n"),
        ("n {}\n", "n {}\n"),
    ],
)
def test_reverse(text, expected):
    d = rkdl.parse(text)
    view = d.children if text.startswith("a") else d["n"].children
    before = list(view)
    view.reverse()
    assert str(d) == expected
    assert list(view) == before[::-1]


def test_iadd_and_extend():
    d = doc()
    view = same = d["e"].children
    view += [rkdl.Node("x"), rkdl.Node("y")]
    assert view is same
    assert [n.name for n in d["e"].children] == ["x", "y"]
    d["e"].children.extend(n for n in [rkdl.Node("z")])
    assert str(d).endswith("e {\n    x\n    y\n    z\n}\n")
    # The argument is read in full first, and its nodes are already attached.
    with pytest.raises(ValueError, match="already in a document"):
        view.extend(view)
    assert [n.name for n in view] == ["x", "y", "z"]


# Untouched text stays identical


BIG = """\
// config
server  "web"   port=0x1F90 {
    /* listen */ listen "0.0.0.0"
    routes {
        // first
        get "/"   handler=index
        post "/x" ; put "/y"
    }
}

/-disabled 1
client #"raw"# {
    retries 3
}
"""


def test_edits_leave_the_rest_byte_identical():
    d = rkdl.parse(BIG)
    assert str(d) == BIG
    routes = d["server"]["routes"]
    routes.children.append(rkdl.Node("delete", "/z"))
    out = str(d)
    head = BIG[: BIG.index('        post "/x"')]
    tail = BIG[BIG.index("    }\n}\n\n/-disabled") :]
    assert out.startswith(head)
    assert out.endswith(tail)

    d = rkdl.parse(BIG)
    d["client"].children.insert(0, rkdl.Node("timeout", 5))
    out = str(d)
    assert out.startswith(BIG[: BIG.index("    retries")])
    assert out.endswith(BIG[BIG.index("    retries") :])
    assert 'client #"raw"# {\n    timeout 5\n    retries 3\n}\n' in out

    d = rkdl.parse(BIG)
    d["server"].name = "srv"
    assert str(d) == BIG.replace("server  ", "srv  ", 1)

    d = rkdl.parse(BIG)
    d["server"]["routes"].children.pop(0)
    out = str(d)
    assert out.startswith(BIG[: BIG.index("        // first")])
    assert out.endswith(BIG[BIG.index('        post "/x"') :])


def test_repr_quotes_names_that_would_not_parse_back():
    assert repr(rkdl.Node("+.7")) == '<Node "+.7">'
    assert repr(rkdl.Node("a\x7f")) == '<Node "a\\u{7f}">'
