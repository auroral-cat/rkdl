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
    assert str(d) == "(new)renamed 1\n"
    n.type = None
    n.name = "two words"
    assert n.type is None
    assert str(d) == '"two words" 1\n'


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


def test_children_assignment_errors():
    d = doc()
    with pytest.raises(TypeError):
        d["a"].children = [rkdl.Node("x")]
    with pytest.raises(TypeError):
        d.children = []


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
    assert "client #\"raw\"# {\n    timeout 5\n    retries 3\n}\n" in out

    d = rkdl.parse(BIG)
    d["server"].name = "srv"
    assert str(d) == BIG.replace("server  ", "srv  ", 1)

    d = rkdl.parse(BIG)
    d["server"]["routes"].children.pop(0)
    out = str(d)
    assert out.startswith(BIG[: BIG.index("        // first")])
    assert out.endswith(BIG[BIG.index('        post "/x"') :])
