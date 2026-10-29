import gc
import weakref

import pytest

import rkdl


def typed_parser(box):
    """A parser whose `t` type's decode refers to `box`, so whatever goes into `box` is reachable from the Type."""
    return rkdl.Parser(types=[rkdl.Type("t", int, decode=lambda v: (box, v)[1], encode=int)])


def home_of(node):
    """A detached node's private document, which Python never gets as a value. The node reports it to the cycle
    collector, which is how it can be found."""
    (home,) = [o for o in gc.get_referents(node) if isinstance(o, rkdl.Document)]
    return home


def test_cycle_through_a_type_callback():
    box = []
    doc = typed_parser(box).parse("a (t)1\n")
    box.append(doc)
    assert doc["a"].args[0] == 1
    doc_ref = weakref.ref(doc)
    del doc, box
    gc.collect()
    assert doc_ref() is None


def test_cycle_through_a_node_handle():
    box = []
    doc = typed_parser(box).parse("a (t)1\n")
    box.append(doc["a"])
    doc_ref = weakref.ref(doc)
    node_ref = weakref.ref(box[0])
    del doc, box
    gc.collect()
    assert doc_ref() is None
    assert node_ref() is None


class Marker:
    pass


def test_cycle_through_a_parser():
    # A Parser can't be weakly referenced, so a marker in the cycle stands in for it.
    box = [Marker()]
    box.append(typed_parser(box))
    marker_ref = weakref.ref(box[0])
    del box
    gc.collect()
    assert marker_ref() is None


def test_document_outlives_a_parser_whose_type_refers_to_it():
    # Not a leak: the parser's Type really does keep the document reachable.
    box = []
    parser = typed_parser(box)
    box.append(parser.parse("a (t)1\n"))
    doc_ref = weakref.ref(box[0])
    gc.collect()
    assert doc_ref() is not None
    del parser, box
    gc.collect()
    assert doc_ref() is None


@pytest.mark.xfail(
    strict=True,
    reason="handles a move repointed share one document pointer, so none of them reports it to the collector",
)
def test_cycle_through_a_moved_node():
    box = []
    doc = typed_parser(box).parse("a\n")
    node = rkdl.Node("b")
    node.children = []
    node.children.append(rkdl.Node("c"))
    child = node["c"]
    doc.children.append(node)
    box.extend([node, child])
    doc_ref = weakref.ref(doc)
    del doc, node, child, box
    gc.collect()
    assert doc_ref() is None


@pytest.mark.xfail(strict=True, reason="Entry and the views have no __traverse__ yet")
@pytest.mark.parametrize("handle", ["entry", "args", "props", "entries", "children"])
def test_cycle_through_an_entry_or_a_view(handle):
    box = []
    doc = typed_parser(box).parse("a (t)1 { b; }\n")
    node = doc["a"]
    box.append(node.args.entry(0) if handle == "entry" else getattr(node, handle))
    doc_ref = weakref.ref(doc)
    del doc, node, box
    gc.collect()
    assert doc_ref() is None


@pytest.mark.parametrize("doc_goes_first", [True, False])
def test_document_is_freed_with_its_last_handle(doc_goes_first):
    doc = rkdl.parse("a 1 x=2 { b; }\n")
    node = doc["a"]
    handles = [node, node["b"], node.args, node.props, node.entries, node.children, node.args.entry(0), doc.children]
    doc_ref = weakref.ref(doc)
    if doc_goes_first:
        del doc
        assert doc_ref() is not None
        del node
        handles.clear()
    else:
        del node
        handles.clear()
        assert doc_ref() is not None
        del doc
    gc.collect()
    assert doc_ref() is None


def test_detached_nodes_private_document_is_freed():
    node = rkdl.Node("a", 1, x=2)
    home_ref = weakref.ref(home_of(node))
    node_ref = weakref.ref(node)
    del node
    gc.collect()
    assert node_ref() is None
    assert home_ref() is None


def test_popped_nodes_private_document_is_freed():
    doc = rkdl.parse("a { b; }\nc\n")
    node = doc.children.pop(0)
    child = node["b"]
    home_ref = weakref.ref(home_of(node))
    del doc, node
    gc.collect()
    assert home_ref() is not None
    del child
    gc.collect()
    assert home_ref() is None


def test_moved_node_keeps_neither_document_alive():
    source = rkdl.parse("a 1\n")
    target = rkdl.parse("b\n")
    node = source["a"]
    target.children.append(source.children.pop(0))
    source_ref = weakref.ref(source)
    target_ref = weakref.ref(target)
    del source
    gc.collect()
    assert source_ref() is None
    del target
    gc.collect()
    assert target_ref() is not None
    del node
    gc.collect()
    assert target_ref() is None


def test_reference_counting_alone_frees_documents_and_handles():
    gc.disable()
    try:
        doc = rkdl.parse("a 1 x=2 { b; }\nc\n")
        other = rkdl.parse("d\n")
        node = doc["a"]
        handles = [node, node["b"], node.args, node.props, node.entries, node.children, node.args.entry(0)]
        other.children.append(doc.children.pop())
        detached = rkdl.Node("e", 1)
        refs = [weakref.ref(o) for o in [doc, other, detached, home_of(detached), *handles[:2], handles[-1]]]
        del doc, other, node, handles, detached
        assert [r() for r in refs] == [None] * len(refs)
    finally:
        gc.enable()
