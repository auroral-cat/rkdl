import gc
import subprocess
import sys
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


def test_cycle_through_nodes_moved_together():
    # Every handle in the subtree moves at once, and each of them must report its document on its own. The second
    # document's Type holds them all, and they end up there after moving away and back.
    box = []
    parser = typed_parser(box)
    source = parser.parse("x\n")
    target = parser.parse("y\n")
    node = rkdl.Node("a", 1, k=2)
    node.children = []
    for name in "bcd":
        node.children.append(rkdl.Node(name, 3))
    handles = [node, *node.children, node.args.entry(0), node.props.entry("k"), node.children[0].args.entry(0)]
    target.children.append(node)
    source.children.append(target.children.pop())
    target.children.append(source.children.pop())
    box.extend(handles)
    target_ref = weakref.ref(target)
    handle_refs = [weakref.ref(h) for h in handles]
    del parser, source, target, node, handles, box
    gc.collect()
    assert target_ref() is None
    assert [r() for r in handle_refs] == [None] * len(handle_refs)


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


# A handle in a garbage cycle. The collector clears the cache's weakref to it before finalizers run, and the cache
# used to treat the slot as taken until the handle was freed, so looking the node up from a finalizer spun forever.
# Each case runs in a subprocess, so a hang fails the test instead of stopping the run. The old handle, which only a
# finalizer can still reach, isn't the one lookups return any more.

FINALIZER_CASES = {
    "finalizer_looks_up_the_node": (
        """
class Wrapper:
    def __init__(self, node):
        self.node = node
        self.me = self
    def __del__(self):
        print(doc["a"], doc["a"] is doc["a"])
doc = rkdl.parse("a 1\\n")
Wrapper(doc["a"])
gc.collect()
""",
        "a 1 True",
    ),
    "finalizer_in_a_cycle_through_a_type": (
        """
class Spy:
    def __del__(self):
        print(self.doc["a"], self.doc["a"] is self.box[1])
def make():
    box = []
    parser = rkdl.Parser(types=[rkdl.Type("t", int, decode=lambda v: (box, v)[1], encode=int)])
    doc = parser.parse("a (t)1\\n")
    spy = Spy()
    spy.doc = doc
    spy.box = box
    box.extend([doc, doc["a"], spy])
make()
gc.collect()
""",
        "a (t)1 False",
    ),
    "resurrected_handle": (
        """
kept = []
class Wrapper:
    def __init__(self, node):
        self.node = node
        self.me = self
    def __del__(self):
        kept.append(self)
doc = rkdl.parse("a 1\\nb 2\\n")
Wrapper(doc["a"])
gc.collect()
print(kept[0].node, doc["a"], doc["a"] is kept[0].node, len(list(doc.children)))
""",
        "a 1 a 1 False 2",
    ),
    "resurrected_cycle_through_a_type": (
        """
saved = []
class Spy:
    def __del__(self):
        saved.append(self.box)
def make():
    box = []
    parser = rkdl.Parser(types=[rkdl.Type("t", int, decode=lambda v: (box, v)[1], encode=int)])
    doc = parser.parse("a (t)1\\n")
    spy = Spy()
    spy.box = box
    box.extend([doc, doc["a"], spy])
make()
gc.collect()
doc, old, _ = saved[0]
print(old.name, old.args[0], doc["a"] is old, doc["a"] == old)
""",
        "a 1 False True",
    ),
}


@pytest.mark.parametrize("case", FINALIZER_CASES)
def test_finalizer_in_a_garbage_cycle_gets_a_fresh_handle(case):
    code, output = FINALIZER_CASES[case]
    command = [sys.executable, "-c", "import gc\nimport rkdl\n" + code]
    result = subprocess.run(command, check=False, capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == output


def test_a_weakref_callback_on_a_handle_can_look_its_node_up():
    # The handle's home is freed before its weakrefs are cleared, and CPython runs the newer callback first, so this
    # lookup finds the node's slot dead and puts a new handle in it. The cache's own callback for the old handle runs
    # after, and must leave the new one's slot alone.
    doc = rkdl.parse("server 8080\n")
    looked_up = []
    ref = weakref.ref(doc["server"], lambda _: looked_up.append(doc["server"]))
    assert ref() is None
    (again,) = looked_up
    assert again.args[0] == 8080
    assert doc["server"] is again
    assert doc.children[0] is again


def weakref_count():
    return sum(isinstance(o, weakref.ReferenceType) for o in gc.get_objects())


def test_cache_drops_the_slots_of_dead_handles():
    # The cache used to sweep dead slots only when it doubled in size, keeping up to twice the peak.
    doc = rkdl.parse("".join(f"n{i} {i}\n" for i in range(2000)))
    gc.collect()
    before = weakref_count()
    nodes = list(doc.children)
    entries = [n.args.entry(0) for n in nodes]
    assert weakref_count() >= before + 4000
    del nodes, entries
    gc.collect()
    assert weakref_count() <= before + 10


def test_removed_entry_lets_go_of_its_document():
    doc = rkdl.parse("a 1 k=2\n")
    arg = doc["a"].args.entry(0)
    prop = doc["a"].props.entry("k")
    doc["a"].args.pop(0)
    del doc["a"].props["k"]
    doc_ref = weakref.ref(doc)
    del doc
    gc.collect()
    assert doc_ref() is None
    for entry in (arg, prop):
        with pytest.raises(RuntimeError, match="entry was removed"):
            entry.value  # noqa: B018
        assert not [o for o in gc.get_referents(entry) if isinstance(o, rkdl.Document)]
