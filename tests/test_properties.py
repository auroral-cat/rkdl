"""Property tests: random sequences of edits over two documents, checked against a plain-Python model.

The model is a tree of `MNode`s holding `MEntry`s. Model objects stand for API objects one to one, so the handle pool
can say which node or entry each handle should still refer to.
"""

import gc
import math
import os
import re

import pytest
import rkdl
from conftest import MISSING_KDL_RS, has_duplicate_props, have_kdl_rs, read_case, spec_inputs, walk
from hypothesis import note, settings
from hypothesis import strategies as st
from hypothesis.stateful import RuleBasedStateMachine, initialize, invariant, precondition, rule
from rkdl._rkdl import _debug_reserved_types

settings.register_profile("default", max_examples=120, stateful_step_count=40, deadline=None)
settings.register_profile("long", max_examples=1000, stateful_step_count=60, deadline=None)
PROFILE = settings.get_profile(os.environ.get("HYPOTHESIS_PROFILE", "default"))

# Type annotations rkdl converts. Entry annotations the tests set avoid these, so values stay plain.
RESERVED_TYPES = {name for name, _ in _debug_reserved_types()}

# The model doesn't follow annotations it didn't set (reserved ones from the spec corpus, after a value write).
ANY_TYPE = "<any>"


# The model


class MEntry:
    def __init__(self, value, type=None):
        self.value = value
        self.type = type
        self.alive = True

    def copy(self):
        return MEntry(self.value, self.type)

    def write(self, value):
        """A value write keeps an annotation the value fits, which the model knows only for unreserved ones."""
        self.value = value
        if self.type is not None and (self.type in RESERVED_TYPES or self.type == ANY_TYPE):
            self.type = ANY_TYPE


class MNode:
    def __init__(self, name, type=None, args=(), props=(), children=None):
        self.name = name
        self.type = type
        self.args = list(args)
        self.props = dict(props)
        self.children = children
        # Layout the model knows for sure, for generated text and new nodes. False means "don't know".
        self.flat = False  # the node's text, comments above it included, is one line
        self.one_line = False  # its children block is written on one line, like `a { b; c }`
        self.foreign = False  # from the spec corpus, whose text the model knows nothing about

    def copy(self):
        m = MNode(
            self.name,
            self.type,
            [e.copy() for e in self.args],
            {k: e.copy() for k, e in self.props.items()},
            None if self.children is None else [c.copy() for c in self.children],
        )
        m.flat, m.one_line, m.foreign = self.flat, self.one_line, self.foreign
        return m

    def walk(self):
        yield self
        for child in self.children or ():
            yield from child.walk()

    def __repr__(self):
        args = [e.value for e in self.args]
        props = {k: e.value for k, e in self.props.items()}
        kids = None if self.children is None else len(self.children)
        return f"MNode({self.name!r}, type={self.type!r}, args={args!r}, props={props!r}, children={kids})"


def same_value(actual, expected, loose=False):
    if isinstance(actual, float) and isinstance(expected, float) and math.isnan(actual) and math.isnan(expected):
        return True
    if loose:
        return actual == expected
    return type(actual) is type(expected) and actual == expected


def read_model(node):
    """Builds a model node from what the API reports."""
    args, props = [], {}
    for e in node.entries:
        if e.name is None:
            args.append(MEntry(e.value, e.type))
        else:
            props[e.name] = MEntry(e.value, e.type)
    children = None if node.children is None else [read_model(c) for c in node.children]
    return MNode(node.name, node.type, args, props, children)


def check_entry(entry, expected, where):
    loose = expected.type == ANY_TYPE
    if not loose:
        assert entry.type == expected.type, where
    assert same_value(entry.value, expected.value, loose), (where, entry.value, expected.value)


def check_node(node, expected, where="node"):
    """Asserts that an API node matches a model node, recursively."""
    where = f"{where}/{expected.name!r}"
    assert node.name == expected.name, where
    assert node.type == expected.type, where
    entries = list(node.entries)
    args = [e for e in entries if e.name is None]
    props = [e for e in entries if e.name is not None]
    assert len(args) == len(expected.args), (where, [e.value for e in args], expected)
    for i, (e, m) in enumerate(zip(args, expected.args)):
        check_entry(e, m, f"{where} arg {i}")
    assert [e.name for e in props] == list(expected.props), (where, [e.name for e in props], expected)
    for e in props:
        check_entry(e, expected.props[e.name], f"{where} prop {e.name!r}")
    values = list(node.args)
    assert all(same_value(a, m.value, True) for a, m in zip(values, expected.args)), where
    assert list(node.props) == list(expected.props), where
    check_children(node.children, expected.children, where)


def check_children(children, expected, where):
    if expected is None:
        assert children is None, where
        return
    assert children is not None, where
    nodes = list(children)
    assert len(nodes) == len(expected), (where, [n.name for n in nodes], [m.name for m in expected])
    for node, m in zip(nodes, expected):
        check_node(node, m, where)


# Values


I128 = 2**127
# Code points a quoted string in generated text has to escape, since KDL doesn't allow them literally.
UNPRINTABLE = set(map(chr, [*range(0x08), *range(0x0E, 0x20), 0x0B, 0x7F, 0x85, 0x2028, 0x2029]))
UNPRINTABLE |= set(map(chr, [0x200E, 0x200F, *range(0x202A, 0x202F), *range(0x2066, 0x206A), 0xFEFF]))

AWKWARD_STRINGS = [
    "", " ", "  ", "\t", "\n", "\r\n", '"', "\\", '\\"', "'", "é", "日本語", "\u00a0", "\U0001f600",
    "+.7", "-.7", ".7", "+", "-", "--", "#", "##", "true", "false", "null", "inf", "-inf", "nan", "#true",
    "1", "-1", "0x10", "1e3", "a b", "a=b", "/-", "//", "/*", "*/", "{", "}", ";", "(", ")", "=", '#"x"#', "\\u{41}",
]  # fmt: skip


strings = st.one_of(
    st.sampled_from(AWKWARD_STRINGS),
    st.text(st.characters(exclude_categories=["Cs"]), max_size=6),
)
# i128::MIN itself is left out: kdl-rs can't parse it back (notes/upstream.md).
ints = st.one_of(
    st.sampled_from([0, 1, -1, I128 - 1, -I128 + 1, 2**63, -(2**63) - 1, 2**64]),
    st.integers(-I128 + 1, I128 - 1),
)
floats = st.one_of(st.sampled_from([math.inf, -math.inf, math.nan, 0.0, -0.0, 1e300, 5e-324]), st.floats())
values = st.one_of(strings, ints, floats, st.booleans(), st.none())
names = strings
entry_types = st.one_of(st.none(), strings.filter(lambda s: s not in RESERVED_TYPES))


# Starting documents: generated KDL text with varied formatting


BARE = re.compile(r"[A-Za-z_][A-Za-z0-9_-]*")
KEYWORDS = {"true", "false", "null", "inf", "nan"}
ESCAPES = {"\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t", "\b": "\\b", "\f": "\\f"}


def quote(s):
    out = []
    for c in s:
        if c in ESCAPES:
            out.append(ESCAPES[c])
        elif c in UNPRINTABLE:
            out.append(f"\\u{{{ord(c):x}}}")
        else:
            out.append(c)
    return '"' + "".join(out) + '"'


def render_string(draw, s):
    style = draw(st.sampled_from(["quoted", "raw", "bare"]))
    if style == "bare" and BARE.fullmatch(s) and s not in KEYWORDS:
        return s
    # `#"""#` would open a multi-line raw string.
    raw_ok = '"#' not in s and not s.startswith('"') and not any(c in UNPRINTABLE or c in "\r\n\f\b" for c in s)
    if style == "raw" and raw_ok:
        return '#"' + s + '"#'
    return quote(s)


def render_value(draw, v):
    if v is None:
        return "#null"
    if v is True:
        return "#true"
    if v is False:
        return "#false"
    if isinstance(v, int):
        if v >= 0 and draw(st.booleans()):
            return hex(v)
        return str(v)
    if isinstance(v, float):
        if math.isnan(v):
            return "#nan"
        if math.isinf(v):
            return "#inf" if v > 0 else "#-inf"
        return repr(v)
    return render_string(draw, v)


def render_entry(draw, key, entry):
    ty = "" if entry.type is None else "(" + render_string(draw, entry.type) + ")"
    text = ty + render_value(draw, entry.value)
    return text if key is None else render_string(draw, key) + "=" + text


def render_head(draw, node):
    """A node's type, name and entries, and a skipped block if it has a block. All on one line."""
    parts = []
    if node.type is not None:
        # No whitespace inside `(type)`: kdl-rs 6.7.1 drops it (notes/upstream.md).
        parts.append("(" + render_string(draw, node.type) + ")" + render_string(draw, node.name))
    else:
        parts.append(render_string(draw, node.name))
    # Properties go between arguments, keeping their own order.
    entries = [(None, e) for e in node.args]
    lowest = 0
    for key, e in node.props.items():
        at = draw(st.integers(lowest, len(entries)))
        entries.insert(at, (key, e))
        lowest = at + 1
    for key, e in entries:
        if draw(st.integers(0, 6)) == 0:
            parts.append("/-" + draw(st.sampled_from(["0", "x=1", '"s"'])))
        if draw(st.integers(0, 8)) == 0:
            parts.append("/* c */")
        parts.append(render_entry(draw, key, e))
    line = draw(st.sampled_from([" ", "  ", "\t"])).join(parts)
    if node.children is not None and draw(st.integers(0, 6)) == 0:
        line += " /-{ skipped }"
    return line


def render_block_on_one_line(draw, nodes):
    """A children block written on one line, like ` { b; c }`. Every node in it is flat."""
    out = []
    for node in nodes:
        node.flat = True
        node.one_line = node.children is not None
        line = render_head(draw, node)
        if node.children:
            line += render_block_on_one_line(draw, node.children)
        elif node.children is not None:
            line += draw(st.sampled_from([" {}", "{}"]))
        out.append(line)
    return " { " + "; ".join(out) + draw(st.sampled_from(["", ";"])) + " }"


def render_nodes(draw, nodes, indent, level):
    """Nodes one per line, setting each model node's `flat` and `one_line` to match the text."""
    pad = indent * level
    out = []
    end = "\n"
    for node in nodes:
        # kdl-rs gives a comment after `;` to the next node, as part of its leading.
        above = "//" in end
        if draw(st.integers(0, 5)) == 0:
            out.append("\n")
            above = True
        if draw(st.integers(0, 4)) == 0:
            out.append(pad + "// a comment\n")
            above = True
        if draw(st.integers(0, 6)) == 0:
            # No `;` after a slashdashed node: kdl-rs rejects it (notes/upstream.md).
            out.append(pad + "/- gone 1 {\n" + pad + indent + "x\n" + pad + "}\n")
            above = True
        line = pad + render_head(draw, node)
        if node.children and draw(st.integers(0, 3)) == 0:
            line += render_block_on_one_line(draw, node.children)
        elif node.children:
            line += " {\n" + render_nodes(draw, node.children, indent, level + 1) + pad + "}"
        elif node.children is not None:
            line += draw(st.sampled_from([" {}", "{}", " {\n" + pad + "}"]))
        # Nothing but a terminator right after `}`: kdl-rs drops whitespace (notes/upstream.md) and comments
        # (test_comment_after_a_block_survives) there. A node without a block may get one later, so the same goes for
        # a line comment after it. Spaces and a block comment are fine: the new block goes after them.
        if node.children is None:
            end = draw(st.sampled_from(["\n", "\n", ";\n", " \n", " /* end */\n", "; // end\n"]))
        else:
            end = draw(st.sampled_from(["\n", "\n", ";\n", "; // end\n"]))
        node.one_line = node.children is not None and "\n" not in line
        node.flat = "\n" not in line and not above and end in ("\n", ";\n", " \n")
        out.append(line + end)
    return "".join(out)


@st.composite
def model_nodes(draw, level=0):
    nodes = []
    for _ in range(draw(st.integers(0, 3))):
        args = [MEntry(v, draw(entry_types)) for v in draw(st.lists(values, max_size=3))]
        props = {k: MEntry(v, draw(entry_types)) for k, v in draw(st.dictionaries(names, values, max_size=2)).items()}
        children = None
        if level < 2 and draw(st.booleans()):
            children = draw(model_nodes(level + 1))
        nodes.append(MNode(draw(names), draw(st.none() | names), args, props, children))
    return nodes


@st.composite
def generated_documents(draw):
    nodes = draw(model_nodes())
    indent = draw(st.sampled_from(["    ", "  ", "\t"]))
    text = render_nodes(draw, nodes, indent, 0)
    # The generator never puts a line ending inside a value or comment, so this changes only the line ends.
    if draw(st.booleans()):
        text = text.replace("\n", "\r\n")
    return text, nodes


def corpus_is_usable(text):
    """Small valid files that print stably and read as plain values with no duplicate keys."""
    try:
        doc = rkdl.parse(text)
        model = [read_model(n) for n in doc.children]
        again = rkdl.parse(str(doc))
        if str(again) != str(doc):
            return False
        check_children(again.children, model, "corpus")
    except (rkdl.ParseError, ValueError, AssertionError):
        return False
    if has_duplicate_props(doc):
        return False
    return every_new_block_prints_stably(text)


def every_new_block_prints_stably(text):
    """Leaves out files where giving a node `{}` puts a comment after `}` (test_comment_after_a_block_survives)."""
    for i in range(len(list(walk(rkdl.parse(text))))):
        doc = rkdl.parse(text)
        _, node = list(walk(doc))[i]
        if node.children is None:
            node.children = []
        try:
            if str(rkdl.parse(str(doc))) != str(doc):
                return False
        except rkdl.ParseError:
            return False
    return True


def load_corpus():
    texts = []
    for path in spec_inputs():
        text = read_case(path)
        if text is None or path.stat().st_size > 1500:
            continue
        # A final newline keeps a trailing `// comment` from swallowing appended nodes
        # (test_append_after_a_final_line_comment).
        if not text.endswith("\n"):
            text += "\n"
        if corpus_is_usable(text):
            texts.append(text)
    return texts


CORPUS = load_corpus()


def test_corpus_is_loaded():
    if not have_kdl_rs():
        pytest.skip(MISSING_KDL_RS)
    assert CORPUS


start_documents = [st.just(("", [])), generated_documents()]
if CORPUS:
    start_documents.append(st.sampled_from(CORPUS).map(lambda text: (text, None)))
start_documents = st.one_of(*start_documents)


# The state machine


class EditSequences(RuleBasedStateMachine):
    def __init__(self):
        super().__init__()
        self.docs = []
        self.trees = []  # model of each document's top-level nodes
        self.detached = []  # (handle, model) of detached roots we still hold
        self.nodes = {}  # id(model) -> (handle, model): node handles kept from earlier steps
        self.entries = []  # (handle, model entry, model owner): entry handles kept from earlier steps
        self.crlf = []  # per document: whether it's written with `\r\n` line endings

    # Finding things

    def roots(self):
        """Every top-level sibling list: one per document plus each detached root on its own."""
        for k, tree in enumerate(self.trees):
            yield ("doc", k), tree
        for handle, m in self.detached:
            yield ("detached", handle), [m]

    def all_nodes(self):
        for _, tree in self.roots():
            for root in tree:
                yield from root.walk()

    def locate(self, target):
        """(root, path of indices) for a model node, or None if we lost track of it."""
        for root, tree in self.roots():
            for i, m in enumerate(tree):
                path = self.path_in(m, target)
                if path is not None:
                    return root, [i, *path]
        return None

    def path_in(self, m, target):
        if m is target:
            return []
        for i, child in enumerate(m.children or ()):
            path = self.path_in(child, target)
            if path is not None:
                return [i, *path]
        return None

    def lineage(self, target):
        """The model nodes from the top of target's tree down to target itself."""
        for _, tree in self.roots():
            for m in tree:
                path = self.path_in(m, target)
                if path is not None:
                    out = [m]
                    for i in path:
                        m = m.children[i]
                        out.append(m)
                    return out
        raise AssertionError("node not found")

    def resolve(self, target):
        """Looks a model node up fresh through the API, without the pool."""
        root, path = self.locate(target)
        if root[0] == "doc":
            node = self.docs[root[1]].children[path[0]]
        else:
            node = root[1]
        for i in path[1:]:
            node = node.children[i]
        return node

    def container(self, target):
        """The document or parent node a model node can be looked up in by name, or None for a detached root."""
        root, path = self.locate(target)
        if root[0] == "doc":
            container, steps = self.docs[root[1]], path[:-1]
        elif len(path) == 1:
            return None
        else:
            container, steps = root[1], path[1:-1]
        for i in steps:
            container = container.children[i]
        return container

    def handle(self, data, m):
        """A handle for a model node: a kept one or a fresh lookup, which we may keep."""
        if id(m) in self.nodes and data.draw(st.booleans(), label="use kept handle"):
            return self.nodes[id(m)][0]
        node = self.resolve(m)
        if data.draw(st.booleans(), label="keep handle"):
            self.nodes[id(m)] = (node, m)
        return node

    def parents(self):
        """Every place a node can go: (children view getter, model list, owning model node or None)."""
        out = []
        for k, tree in enumerate(self.trees):
            out.append((("doc", k), tree, None))
        for m in self.all_nodes():
            if m.children is not None:
                out.append((("node", m), m.children, m))
        return out

    def view(self, data, place):
        kind, what = place
        if kind == "doc":
            return self.docs[what].children
        return self.handle(data, what).children

    def has_nodes(self):
        return any(True for _ in self.all_nodes())

    def pick_node(self, data):
        return data.draw(st.sampled_from(list(self.all_nodes())), label="node")

    def detach(self, handle, m):
        self.detached.append((handle, m))

    def forget_subtree(self, m):
        """Stops tracking a subtree that no handle can reach any more."""
        gone = {id(n) for n in m.walk()}
        for key in gone:
            self.nodes.pop(key, None)
        self.entries = [(e, me, owner) for e, me, owner in self.entries if id(owner) not in gone]

    def take_out(self, data, m):
        """Removes a node from wherever it is, returning a detached handle for it."""
        for i, (handle, root) in enumerate(self.detached):
            if root is m:
                del self.detached[i]
                return handle
        for place, siblings, _ in self.parents():
            for i, sibling in enumerate(siblings):
                if sibling is m:
                    popped = self.view(data, place).pop(i)
                    siblings.pop(i)
                    return popped
        raise AssertionError("node not found")

    # Setup

    @initialize(first=start_documents, second=start_documents)
    def start(self, first, second):
        for text, expected in (first, second):
            note(f"document {len(self.docs)}: {text!r}")
            doc = rkdl.parse(text)
            if expected is None:
                tree = [read_model(n) for n in doc.children]
                for root in tree:
                    for m in root.walk():
                        m.foreign = True
            else:
                check_children(doc.children, expected, "generated")
                # The generated model knows the layout too.
                tree = [m.copy() for m in expected]
            self.docs.append(doc)
            self.trees.append(tree)
            self.crlf.append("\r\n" in text)

    # Layout bookkeeping

    def placed(self, place, owner, was_empty, new):
        """Updates the layout flags after `new` model nodes went into owner's block (None for a document).

        A block written on one line stays that way unless a node that doesn't fit on a line goes in, or the block was
        empty. Otherwise it spreads over lines, and so does every block around it.

        A node from the corpus can bring line endings rkdl doesn't restyle, the ones inside multi-line strings. A CRLF
        document stops being one then.
        """
        if owner is not None and (was_empty or not all(n.flat for n in new)):
            for m in self.lineage(owner):
                m.flat = m.one_line = False
        if any(d.foreign for n in new for d in n.walk()):
            top = owner and self.lineage(owner)[0]
            for k, tree in enumerate(self.trees):
                if (place[0] == "doc" and place[1] == k) or any(root is top for root in tree):
                    self.crlf[k] = False

    # Children

    @rule(data=st.data(), name=names, ty=st.none() | names, args=st.lists(values, max_size=2),
          props=st.dictionaries(names, values, max_size=2), block=st.booleans())  # fmt: skip
    def insert_new(self, data, name, ty, args, props, block):
        place, siblings, owner = data.draw(st.sampled_from(self.parents()), label="parent")
        index = data.draw(st.integers(-len(siblings) - 1, len(siblings) + 1), label="index")
        node, m = self.new_node(name, ty, args, props, block)
        was_empty = not siblings
        self.view(data, place).insert(index, node)
        siblings.insert(index, m)
        self.placed(place, owner, was_empty, [m])
        if data.draw(st.booleans(), label="keep new node"):
            self.nodes[id(m)] = (node, m)

    def new_node(self, name, ty=None, args=(), props=None, block=False):
        props = props or {}
        node = rkdl.Node(name, *args, type=ty, props=props)
        m = MNode(name, ty, [MEntry(v) for v in args], {k: MEntry(v) for k, v in props.items()})
        if block:
            node.children = []
            m.children = []
        m.flat = True
        m.one_line = block
        return node, m

    def node_to_insert(self, data, owner, name):
        """A detached node to put into owner's block: a new one, a copy, or a detached root we hold.

        Returns (handle, model).
        """
        fits = [(handle, m) for handle, m in self.detached if owner is None or not any(n is owner for n in m.walk())]
        hows = ["new"] + ["copy"] * self.has_nodes() + ["detached"] * bool(fits)
        how = data.draw(st.sampled_from(hows), label="how")
        if how == "new":
            return self.new_node(name)
        if how == "copy":
            m = self.pick_node(data)
            return self.handle(data, m).copy(), m.copy()
        handle, m = data.draw(st.sampled_from(fits), label="detached root")
        self.detached = [(h, d) for h, d in self.detached if d is not m]
        self.nodes[id(m)] = (handle, m)
        return handle, m

    @precondition(lambda self: any(siblings for _, siblings, _ in self.parents()))
    @rule(data=st.data(), how=st.sampled_from(["pop", "remove", "del"]))
    def remove_child(self, data, how):
        place, siblings, _ = data.draw(st.sampled_from([p for p in self.parents() if p[1]]), label="parent")
        index = data.draw(st.integers(-len(siblings), len(siblings) - 1), label="index")
        m = siblings[index]
        kept = self.nodes.get(id(m), (None,))[0]
        view = self.view(data, place)
        if how == "pop":
            handle = view.pop(index)
        elif how == "remove":
            handle = self.handle(data, m)
            view.remove(handle)
        else:
            handle = kept
            del view[index]
        siblings.pop(index)
        if kept is not None:
            assert handle is kept
        if handle is not None:
            self.detach(handle, m)
        else:
            self.forget_subtree(m)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def move(self, data):
        m = self.pick_node(data)
        targets = [p for p in self.parents() if p[2] is None or not any(n is p[2] for n in m.walk())]
        place, siblings, owner = data.draw(st.sampled_from(targets), label="new parent")
        handle = self.take_out(data, m)
        index = data.draw(st.integers(-len(siblings) - 1, len(siblings) + 1), label="index")
        was_empty = not siblings
        self.view(data, place).insert(index, handle)
        siblings.insert(index, m)
        self.placed(place, owner, was_empty, [m])
        self.nodes[id(m)] = (handle, m)

    @rule(data=st.data())
    def insert_attached_raises(self, data):
        attached = [m for k in range(2) for root in self.trees[k] for m in root.walk()]
        if not attached:
            return
        m = data.draw(st.sampled_from(attached), label="attached node")
        place, _, _ = data.draw(st.sampled_from(self.parents()), label="parent")
        with pytest.raises(ValueError):
            self.view(data, place).append(self.handle(data, m))

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def copy_and_insert(self, data):
        m = self.pick_node(data)
        copy = self.handle(data, m).copy()
        cm = m.copy()
        place, siblings, owner = data.draw(st.sampled_from(self.parents()), label="parent")
        index = data.draw(st.integers(-len(siblings) - 1, len(siblings) + 1), label="index")
        was_empty = not siblings
        self.view(data, place).insert(index, copy)
        siblings.insert(index, cm)
        self.placed(place, owner, was_empty, [cm])
        if data.draw(st.booleans(), label="keep copy"):
            self.nodes[id(cm)] = (copy, cm)

    @precondition(lambda self: any(siblings for _, siblings, _ in self.parents()))
    @rule(data=st.data(), name=names)
    def replace_child(self, data, name):
        place, siblings, owner = data.draw(st.sampled_from([p for p in self.parents() if p[1]]), label="parent")
        index = data.draw(st.integers(-len(siblings), len(siblings) - 1), label="index")
        old = siblings[index]
        old_handle = self.nodes.get(id(old), (None,))[0]
        if old_handle is None and data.draw(st.booleans(), label="hold old"):
            old_handle = self.resolve(old)
        handle, m = self.node_to_insert(data, owner, name)
        self.view(data, place)[index] = handle
        siblings[index] = m
        self.placed(place, owner, False, [m])
        if old_handle is not None:
            self.detach(old_handle, old)
        else:
            self.forget_subtree(old)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def reverse_children(self, data):
        place, siblings, _ = data.draw(st.sampled_from(self.parents()), label="parent")
        self.view(data, place).reverse()
        siblings.reverse()

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def count_children(self, data):
        place, siblings, _ = data.draw(st.sampled_from(self.parents()), label="parent")
        m = self.pick_node(data)
        view = self.view(data, place)
        assert view.count(self.handle(data, m)) == sum(s is m for s in siblings)
        assert view.count(rkdl.Node(m.name)) == 0

    @rule(data=st.data(), name=names, through_node=st.booleans())
    def extend_children(self, data, name, through_node):
        place, siblings, owner = data.draw(st.sampled_from(self.parents()), label="parent")
        new = [self.node_to_insert(data, owner, name) for _ in range(data.draw(st.integers(0, 2), label="count"))]
        handles = [handle for handle, _ in new]
        was_empty = not siblings
        if through_node:
            # `x.children += ...` reads the view, extends it, then assigns it back.
            kind, what = place
            holder = self.docs[what] if kind == "doc" else self.handle(data, what)
            holder.children += handles
        else:
            view = self.view(data, place)
            before = view
            view += handles
            assert view is before
        siblings.extend(m for _, m in new)
        self.placed(place, owner, was_empty and bool(new), [m for _, m in new])
        for handle, m in new:
            self.nodes[id(m)] = (handle, m)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), block=st.sampled_from([None, []]))
    def set_children(self, data, block):
        m = self.pick_node(data)
        node = self.handle(data, m)
        old = m.children or []
        handles = list(node.children) if old else []
        node.children = None if block is None else []
        m.children = None if block is None else []
        for handle, child in zip(handles, old):
            self.detach(handle, child)

    @precondition(lambda self: self.detached)
    @rule(data=st.data())
    def drop_detached(self, data):
        i = data.draw(st.integers(0, len(self.detached) - 1), label="detached")
        _, m = self.detached.pop(i)
        self.forget_subtree(m)

    @rule(data=st.data())
    def forget_handles(self, data):
        for key in list(self.nodes):
            if data.draw(st.booleans(), label="forget"):
                del self.nodes[key]
        self.entries = [e for e in self.entries if data.draw(st.booleans(), label="keep entry")]
        gc.collect()

    # Node attributes

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), name=names)
    def rename(self, data, name):
        m = self.pick_node(data)
        self.handle(data, m).name = name
        m.name = name

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), ty=st.none() | names)
    def set_type(self, data, ty):
        m = self.pick_node(data)
        self.handle(data, m).type = ty
        m.type = ty

    # Entries

    def keep_entry(self, data, entry, me, owner):
        if data.draw(st.booleans(), label="keep entry"):
            self.entries.append((entry, me, owner))

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), new_key=names, value=values)
    def set_prop(self, data, new_key, value):
        m = self.pick_node(data)
        key = data.draw(st.sampled_from([new_key, *m.props]), label="key")
        self.handle(data, m).props[key] = value
        if key in m.props:
            m.props[key].write(value)
        else:
            m.props[key] = MEntry(value)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def del_prop(self, data):
        m = self.pick_node(data)
        if not m.props:
            return
        key = data.draw(st.sampled_from(list(m.props)), label="key")
        props = self.handle(data, m).props
        if data.draw(st.booleans(), label="pop"):
            assert same_value(props.pop(key), m.props[key].value, m.props[key].type == ANY_TYPE)
        else:
            del props[key]
        m.props.pop(key).alive = False

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), value=values)
    def append_arg(self, data, value):
        m = self.pick_node(data)
        self.handle(data, m).args.append(value)
        m.args.append(MEntry(value))

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), value=values)
    def insert_arg(self, data, value):
        m = self.pick_node(data)
        index = data.draw(st.integers(-len(m.args) - 2, len(m.args) + 2), label="index")
        self.handle(data, m).args.insert(index, value)
        m.args.insert(index, MEntry(value))

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), value=values)
    def set_arg(self, data, value):
        m = self.pick_node(data)
        if not m.args:
            return
        index = data.draw(st.integers(-len(m.args), len(m.args) - 1), label="index")
        self.handle(data, m).args[index] = value
        m.args[index].write(value)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def pop_arg(self, data):
        m = self.pick_node(data)
        if not m.args:
            return
        index = data.draw(st.integers(-len(m.args), len(m.args) - 1), label="index")
        value = self.handle(data, m).args.pop(index)
        me = m.args.pop(index)
        me.alive = False
        assert same_value(value, me.value, me.type == ANY_TYPE)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def reverse_args(self, data):
        m = self.pick_node(data)
        self.handle(data, m).args.reverse()
        # The values move and the slots stay, so an entry handle keeps its place and sees another value.
        contents = [(e.value, e.type) for e in m.args]
        for e, (value, ty) in zip(m.args, reversed(contents)):
            e.value, e.type = value, ty

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), more=st.lists(values, max_size=3))
    def extend_args(self, data, more):
        m = self.pick_node(data)
        args = self.handle(data, m).args
        before = args
        args += more
        assert args is before
        m.args.extend(MEntry(v) for v in more)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def popitem_prop(self, data):
        m = self.pick_node(data)
        props = self.handle(data, m).props
        if not m.props:
            with pytest.raises(KeyError):
                props.popitem()
            return
        key, value = props.popitem()
        expected_key, me = m.props.popitem()
        me.alive = False
        assert key == expected_key
        assert same_value(value, me.value, me.type == ANY_TYPE)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data(), ty=entry_types)
    def set_entry_type(self, data, ty):
        m = self.pick_node(data)
        choices = [("arg", i) for i in range(len(m.args))] + [("prop", k) for k in m.props]
        if not choices:
            return
        kind, which = data.draw(st.sampled_from(choices), label="entry")
        node = self.handle(data, m)
        if kind == "arg":
            entry, me = node.args.entry(which), m.args[which]
        else:
            entry, me = node.props.entry(which), m.props[which]
        entry.type = ty
        me.type = ty
        self.keep_entry(data, entry, me, m)

    @precondition(lambda self: self.has_nodes())
    @rule(data=st.data())
    def keep_some_entry(self, data):
        m = self.pick_node(data)
        node = self.handle(data, m)
        for i, me in enumerate(m.args):
            self.keep_entry(data, node.args.entry(i), me, m)
        for k, me in m.props.items():
            self.keep_entry(data, node.props.entry(k), me, m)

    @precondition(lambda self: self.entries)
    @rule(data=st.data(), value=values)
    def write_kept_entry(self, data, value):
        entry, me, _ = data.draw(st.sampled_from(self.entries), label="kept entry")
        if me.alive:
            entry.value = value
            me.write(value)
        else:
            with pytest.raises(RuntimeError, match="entry was removed"):
                entry.value = value

    # Formatting

    @rule(k=st.integers(0, 1))
    def fmt(self, k):
        self.docs[k].fmt()
        # kdl-rs's autoformat writes `\n` line endings and spreads every block over lines.
        self.crlf[k] = False
        for root in self.trees[k]:
            for m in root.walk():
                m.flat = m.one_line = False

    # Invariants

    @invariant()
    def documents_match_the_model(self):
        for doc, tree in zip(self.docs, self.trees):
            text = str(doc)
            again = rkdl.parse(text)
            check_children(again.children, tree, "reparsed")
            # Whitespace right after a `}` is the one shape kdl-rs reads back differently (notes/upstream.md), and rkdl
            # never writes it.
            assert str(again) == text, text
            check_children(doc.children, tree, "live")

    @invariant()
    def views_compare_by_content(self):
        for m in self.all_nodes():
            node = self.resolve(m)
            # nan != nan, so a dict holding one never compares equal.
            if not any(isinstance(e.value, float) and math.isnan(e.value) for e in m.props.values()):
                assert node.props == {k: e.value for k, e in m.props.items()}, m
            assert node.entries == node.entries

    @invariant()
    def line_endings_keep_the_document_style(self):
        for k, doc in enumerate(self.docs):
            text = str(doc)
            if self.crlf[k]:
                assert re.search(r"(?<!\r)\n", text) is None, text

    @invariant()
    def one_line_blocks_stay_on_one_line(self):
        for m in self.all_nodes():
            if m.one_line and m.children is not None:
                text = str(self.resolve(m)).rstrip("\r\n")
                assert "\n" not in text, text

    @invariant()
    def kept_handles_still_point_at_their_nodes(self):
        for handle, m in self.nodes.values():
            assert self.resolve(m) is handle
            container = self.container(m)
            if container is not None:
                assert any(n is handle for n in container.get_all(m.name))
            check_node(handle, m, "kept")
        for handle, m in self.detached:
            check_node(handle, m, "detached")

    @invariant()
    def kept_entries_still_point_at_their_entries(self):
        for entry, me, owner in self.entries:
            if not me.alive:
                with pytest.raises(RuntimeError, match="entry was removed"):
                    _ = entry.value
                continue
            node = self.resolve(owner)
            if any(a is me for a in owner.args):
                index = next(i for i, a in enumerate(owner.args) if a is me)
                assert node.args.entry(index) is entry
                assert entry.name is None
            else:
                key = next(k for k, p in owner.props.items() if p is me)
                assert node.props.entry(key) is entry
                assert entry.name == key
            check_entry(entry, me, "kept entry")


TestEditSequences = EditSequences.TestCase
TestEditSequences.settings = PROFILE
