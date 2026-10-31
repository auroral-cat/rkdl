"""Edits whose Python callbacks change the document under them, and inserting nodes from inside a detached node."""

import math

import pytest
import rkdl

KEPT_CHANGING = "the document kept changing during this edit"


class Box:
    def __init__(self, value):
        self.value = value

    def __repr__(self):
        return f"Box({self.value})"


def box_parser(decode=None, encode=None):
    """A parser whose `box` type wraps values in a Box, with hooks that run inside decode and encode."""

    def dec(value):
        if decode:
            decode()
        return Box(value)

    def enc(box):
        if encode:
            encode()
        return box.value

    return rkdl.Parser(types=[rkdl.Type("box", Box, decode=dec, encode=enc)])


# Callbacks that change the node every time they run. Each edit plans with Python, then applies the plan if the node
# didn't change meanwhile, so these used to plan again forever.


def test_pop_whose_decode_always_appends():
    holder = {}
    doc = box_parser(decode=lambda: holder["n"].args.append(1)).parse("n (box)5")
    holder["n"] = doc["n"]
    with pytest.raises(RuntimeError, match=KEPT_CHANGING):
        doc["n"].args.pop(0)


def test_set_whose_encode_always_appends():
    holder = {}
    doc = box_parser(encode=lambda: holder["n"].args.append(1)).parse("n 0")
    holder["n"] = doc["n"]
    with pytest.raises(RuntimeError, match=KEPT_CHANGING):
        doc["n"].args[0] = Box(5)


def test_entry_value_whose_encode_always_changes_the_type():
    holder = {"calls": 0}

    # A new annotation every time, so however many times an attempt encodes, the type has changed by the end of it.
    def retype():
        holder["calls"] += 1
        holder["entry"].type = f"t{holder['calls']}"

    doc = box_parser(encode=retype).parse("n (box)0")
    holder["entry"] = doc["n"].args.entry(0)
    with pytest.raises(RuntimeError, match=KEPT_CHANGING):
        holder["entry"].value = Box(5)


def test_remove_whose_eq_always_edits():
    doc = rkdl.parse("n 1 2 3")
    node = doc["n"]

    class Bad:
        def __eq__(self, other):
            node.props["k"] = 1
            node.args.append(0)
            return True

    with pytest.raises(RuntimeError, match=KEPT_CHANGING):
        node.args.remove(Bad())


def test_del_children_slice_whose_index_always_appends():
    doc = rkdl.parse("a\nb")

    class Index:
        def __index__(self):
            doc.children.append(rkdl.Node("z"))
            return 0

    with pytest.raises(RuntimeError, match=KEPT_CHANGING):
        del doc.children[Index() : 1]


def test_del_args_slice_whose_index_always_appends():
    node = rkdl.parse("a 1 2")["a"]

    class Index:
        def __index__(self):
            node.args.append(0)
            return 0

    with pytest.raises(RuntimeError, match=KEPT_CHANGING):
        del node.args[Index() : 1]


def test_children_slice_whose_index_always_pops():
    doc = rkdl.parse("a\nb")

    class Index:
        def __index__(self):
            doc.children.append(rkdl.Node("z"))
            doc.children.pop(0)
            return 0

    with pytest.raises(RuntimeError, match=KEPT_CHANGING):
        doc.children[Index() : 1]


def test_one_change_during_an_edit_just_retries():
    holder = {"armed": True}

    def once():
        if holder["armed"]:
            holder["armed"] = False
            holder["n"].args.append(1)

    doc = box_parser(decode=once).parse("n (box)5")
    holder["n"] = doc["n"]
    assert doc["n"].args.pop(0).value == 5
    assert str(doc) == "n 1"


# Plans that return or match a value check it again when they apply, so a value written meanwhile isn't lost.


def test_pop_returns_the_value_it_removes():
    holder = {"armed": True}

    def overwrite():
        if holder["armed"]:
            holder["armed"] = False
            holder["n"].args[0] = Box(99)  # same annotation, so the entries keep their shape

    doc = box_parser(decode=overwrite).parse("n (box)1 2")
    holder["n"] = doc["n"]
    assert doc["n"].args.pop(0).value == 99
    assert str(doc) == "n 2"


def test_props_pop_returns_the_value_it_removes():
    holder = {"armed": True}

    def overwrite():
        if holder["armed"]:
            holder["armed"] = False
            holder["n"].props["k"] = Box(99)

    doc = box_parser(decode=overwrite).parse("n k=(box)1 j=2")
    holder["n"] = doc["n"]
    assert doc["n"].props.pop("k").value == 99
    assert str(doc) == "n j=2"


def test_popitem_returns_the_value_it_removes():
    holder = {"armed": True}

    def overwrite():
        if holder["armed"]:
            holder["armed"] = False
            holder["n"].props["k"] = Box(99)

    doc = box_parser(decode=overwrite).parse("n j=2 k=(box)1")
    holder["n"] = doc["n"]
    key, value = doc["n"].props.popitem()
    assert (key, value.value) == ("k", 99)
    assert str(doc) == "n j=2"


def test_remove_matches_the_value_it_removes():
    node = rkdl.parse("n 1 2 3")["n"]

    class FirstTimeChanges:
        armed = True

        def __eq__(self, other):
            if FirstTimeChanges.armed:
                FirstTimeChanges.armed = False
                node.args[0] = 7  # the match it's about to report no longer holds
                return True
            return other == 2

    node.args.remove(FirstTimeChanges())
    assert list(node.args) == [7, 3]


def test_pop_of_a_nan_doesnt_retry_forever():
    # NaN isn't equal to itself, so the apply-time check compares values by their bits instead.
    node = rkdl.parse("n #nan 1")["n"]
    assert math.isnan(node.args.pop(0))
    node.props["x"] = math.nan
    assert math.isnan(node.props.pop("x"))
    assert str(node) == "n 1"


# A node inside a detached node is in a document, like any other node, even though that document is private.


def test_inserting_a_child_of_a_detached_node():
    detached = rkdl.parse("a { b }").children.pop()
    doc = rkdl.parse("x")
    with pytest.raises(ValueError, match="already in a document"):
        doc.children.append(detached["b"])
    with pytest.raises(ValueError, match="already in a document"):
        doc.children[0] = detached["b"]
    assert str(doc) == "x"
    assert str(detached) == "a { b }"


def test_inserting_a_grandchild_into_its_detached_ancestor():
    detached = rkdl.parse("a { b { c } }").children.pop()
    before = str(detached)
    with pytest.raises(ValueError, match="already in a document"):
        detached.children.append(detached["b"]["c"])
    with pytest.raises(ValueError, match="already in a document"):
        detached.children.append(detached["b"])
    assert str(detached) == before


def test_inserting_a_detached_node_into_its_own_child():
    detached = rkdl.parse("a { b {} }").children.pop()
    with pytest.raises(ValueError, match="inside itself"):
        detached["b"].children.append(detached)
    with pytest.raises(ValueError, match="inside itself"):
        detached.children.append(detached)


def test_inserting_a_detached_node_together_with_its_child():
    section = rkdl.Node("section", children=[rkdl.Node("key", 1)])
    doc = rkdl.parse("x\n")
    # The child is in the section's document, so it can't go in beside it, in either order. Neither goes in.
    with pytest.raises(ValueError, match="already in a document"):
        doc.children.extend([section, section["key"]])
    with pytest.raises(ValueError, match="already in a document"):
        doc.children.extend([section["key"], section])
    assert str(doc) == "x\n"
    assert section.parent is None
    assert section["key"].parent is section
