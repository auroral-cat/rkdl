"""Single edits over every valid document in the KDL spec test suite.

Each case parses a spec document, applies one edit to one node, and checks that the result parses, means what the
edit did, leaves the text outside the edited node alone, and indents new nodes like their neighbours.
"""

import functools
import math

import pytest
import rkdl
from conftest import SPEC_INPUT, is_valid_case, read_case, skip_module_without_spec, spec_inputs, walk

skip_module_without_spec()


# A small KDL scanner, independent of rkdl, that finds where each node's text starts and ends. A node's text runs
# from the end of the previous node (so it includes its leading comments, as in kdl-rs) through its terminator.

NEWLINES = ("\r\n", "\r", "\n", "\x85", "\x0b", "\x0c", "\u2028", "\u2029")
SPACES = set("\t \u00a0\u1680\u202f\u205f\u3000") | {chr(c) for c in range(0x2000, 0x200B)}


class Span:
    def __init__(self, start, core):
        self.start = start  # leading comments and whitespace start here
        self.core = core  # the type annotation or name starts here
        self.body_end = None  # before the terminator
        self.end = None  # after the terminator
        self.block = None  # (index of "{", index of "}") of the real children block
        self.children = None


class Scanner:
    def __init__(self, text):
        self.s = text
        self.n = len(text)

    def newline(self, i):
        for nl in NEWLINES:
            if self.s.startswith(nl, i):
                return len(nl)
        return 0

    def line_comment(self, i):
        while i < self.n and not self.newline(i):
            i += 1
        return i + self.newline(i)

    def block_comment(self, i):
        depth = 0
        while i < self.n:
            if self.s.startswith("/*", i):
                depth += 1
                i += 2
            elif self.s.startswith("*/", i):
                depth -= 1
                i += 2
                if depth == 0:
                    return i
            else:
                i += 1
        raise ValueError("unterminated block comment")

    def escline(self, i):
        i += 1
        while i < self.n and self.s[i] in SPACES:
            i += 1
        if self.s.startswith("//", i):
            return self.line_comment(i)
        return i + self.newline(i)

    def string(self, i):
        s = self.s
        hashes = 0
        while s.startswith("#", i + hashes):
            hashes += 1
        j = i + hashes
        if hashes:
            quote = '"""' if s.startswith('"""', j) else '"'
            close = quote + "#" * hashes
            k = s.find(close, j + len(quote))
            if k < 0:
                raise ValueError("unterminated raw string")
            return k + len(close)
        quote = '"""' if s.startswith('"""', j) else '"'
        j += len(quote)
        while j < self.n:
            if s[j] == "\\":
                j += 2
            elif s.startswith(quote, j):
                return j + len(quote)
            else:
                j += 1
        raise ValueError("unterminated string")

    def is_string(self, i):
        j = i
        while self.s.startswith("#", j):
            j += 1
        return self.s.startswith('"', j)

    def line_space(self, i):
        """Skips whitespace, newlines and comments."""
        while i < self.n:
            c = self.s[i]
            if c in SPACES:
                i += 1
            elif self.newline(i):
                i += self.newline(i)
            elif self.s.startswith("//", i):
                i = self.line_comment(i)
            elif self.s.startswith("/*", i):
                i = self.block_comment(i)
            elif c == "\\":
                i = self.escline(i)
            else:
                break
        return i

    def nodes(self, i):
        out = []
        while True:
            start = i
            while True:
                i = self.line_space(i)
                if self.s.startswith("/-", i):
                    i = self.line_space(i + 2)
                    i = self.body(i, Span(i, i)).body_end
                elif i < self.n and self.s[i] == ";":
                    i += 1
                else:
                    break
            if i >= self.n or self.s[i] == "}":
                return out, i
            span = self.body(i, Span(start, i))
            i = span.body_end
            if i < self.n and self.s[i] == ";":
                i += 1
            elif self.s.startswith("//", i):
                i = self.line_comment(i)
            else:
                i += self.newline(i)
            span.end = i
            out.append(span)

    def body(self, i, span):
        s = self.s
        while i < self.n:
            c = s[i]
            if c in ";}" or self.newline(i) or s.startswith("//", i):
                break
            if s.startswith("/*", i):
                i = self.block_comment(i)
            elif s.startswith("/-", i):
                i = self.line_space(i + 2)
                if i < self.n and s[i] == "{":
                    _, j = self.nodes(i + 1)
                    i = j + 1
            elif c == "\\":
                i = self.escline(i)
            elif c == "{":
                children, j = self.nodes(i + 1)
                if j >= self.n or s[j] != "}":
                    raise ValueError("unclosed block")
                span.block = (i, j)
                span.children = children
                i = j + 1
            elif self.is_string(i):
                i = self.string(i)
            else:
                i += 1
        span.body_end = i
        return span


def scan(text):
    nodes, end = Scanner(text).nodes(1 if text.startswith("\ufeff") else 0)
    if end != len(text):
        raise ValueError(f"scanner stopped at {end} of {len(text)}")
    return nodes


def span_at(spans, path):
    for i in path[:-1]:
        spans = spans[i].children
    return spans[path[-1]]


# Reading a document's meaning through the API.


def value_key(entry):
    try:
        v = entry.value
    except ValueError as e:  # a typed value that doesn't fit its type
        return ("error", type(e).__name__, str(entry).strip())
    if isinstance(v, float) and math.isnan(v):
        return ("float", "nan")
    return (type(v).__name__, v)


def snapshot_node(node):
    entries = list(node.entries)
    return {
        "type": node.type,
        "name": node.name,
        "args": [(e.type, value_key(e)) for e in entries if e.name is None],
        "props": {e.name: (e.type, value_key(e)) for e in entries if e.name is not None},
        "entries": len(entries),
        "children": None if node.children is None else [snapshot_node(c) for c in node.children],
    }


def snapshot(doc):
    return [snapshot_node(n) for n in doc.children]


def node_at(doc, path):
    node = doc.children[path[0]]
    for i in path[1:]:
        node = node.children[i]
    return node


def siblings_of(doc, path):
    return doc.children if len(path) == 1 else node_at(doc, path[:-1]).children


def model_siblings(model, path):
    for i in path[:-1]:
        model = model[i]["children"]
    return model


ADDED = {
    "type": None,
    "name": "added",
    "args": [(None, ("int", 1))],
    "props": {},
    "entries": 1,
    "children": None,
}


def new_key(node):
    key = "k"
    while key in node.props:
        key += "k"
    return key


def different_kind(value):
    return "text" if isinstance(value, int) and not isinstance(value, bool) else 7


class AnyType:
    def __eq__(self, other):
        return True


# The edits. Each takes (doc, path), edits the document, and returns a function that applies the same edit to a
# snapshot model.


def edit_append_child(doc, path):
    node = node_at(doc, path)
    if node.children is None:
        node.children = []
    node.children.append(rkdl.Node("added", 1))

    def model(m):
        n = model_siblings(m, path)[path[-1]]
        n["children"] = (n["children"] or []) + [dict(ADDED)]

    return model


def edit_insert_child(doc, path):
    node = node_at(doc, path)
    if node.children is None:
        node.children = []
    node.children.insert(0, rkdl.Node("added", 1))

    def model(m):
        n = model_siblings(m, path)[path[-1]]
        n["children"] = [dict(ADDED)] + (n["children"] or [])

    return model


def edit_append_sibling(doc, path):
    siblings_of(doc, path).insert(path[-1] + 1, rkdl.Node("added", 1))
    return lambda m: model_siblings(m, path).insert(path[-1] + 1, dict(ADDED))


def edit_insert_sibling(doc, path):
    siblings_of(doc, path).insert(path[-1], rkdl.Node("added", 1))
    return lambda m: model_siblings(m, path).insert(path[-1], dict(ADDED))


def edit_pop(doc, path):
    siblings_of(doc, path).pop(path[-1])
    return lambda m: model_siblings(m, path).pop(path[-1])


def edit_new_prop(doc, path):
    node = node_at(doc, path)
    key = new_key(node)
    node.props[key] = "v"

    def model(m):
        n = model_siblings(m, path)[path[-1]]
        n["props"][key] = (None, ("str", "v"))
        n["entries"] += 1

    return model


def edit_append_arg(doc, path):
    node_at(doc, path).args.append(1)

    def model(m):
        n = model_siblings(m, path)[path[-1]]
        n["args"].append((None, ("int", 1)))
        n["entries"] += 1

    return model


def edit_set_first(doc, path):
    node = node_at(doc, path)
    if len(node.args):
        value = different_kind(node.args.entry(0).value)
        node.args[0] = value

        def model(m):
            model_siblings(m, path)[path[-1]]["args"][0] = (AnyType(), value_key_of(value))

    else:
        key = next(iter(node.props))
        value = different_kind(node.props.entry(key).value)
        node.props[key] = value

        def model(m):
            model_siblings(m, path)[path[-1]]["props"][key] = (AnyType(), value_key_of(value))

    return model


def value_key_of(value):
    return (type(value).__name__, value)


def edit_type_on(doc, path):
    node_at(doc, path).type = "added"

    def model(m):
        model_siblings(m, path)[path[-1]]["type"] = "added"

    return model


def edit_type_off(doc, path):
    node_at(doc, path).type = None

    def model(m):
        model_siblings(m, path)[path[-1]]["type"] = None

    return model


def edit_rename(doc, path):
    node_at(doc, path).name = "renamed"

    def model(m):
        model_siblings(m, path)[path[-1]]["name"] = "renamed"

    return model


EDITS = {
    "append_child": edit_append_child,
    "insert_child": edit_insert_child,
    "append_sibling": edit_append_sibling,
    "insert_sibling": edit_insert_sibling,
    "pop": edit_pop,
    "new_prop": edit_new_prop,
    "append_arg": edit_append_arg,
    "set_first": edit_set_first,
    "type_on": edit_type_on,
    "type_off": edit_type_off,
    "rename": edit_rename,
}

CHILD_EDITS = {"append_child", "insert_child"}
SIBLING_EDITS = {"append_sibling", "insert_sibling"}
HEADER_EDITS = {"new_prop", "append_arg", "set_first", "type_on", "type_off", "rename"}


def applies(edit, node):
    if edit == "set_first":
        return len(node.args) > 0 or len(node.props) > 0
    if edit == "type_off":
        return node.type is not None
    return True


# Collecting the cases.


def valid_files():
    return [p for p in spec_inputs() if is_valid_case(p)]


def reparses(text):
    """Whether printing the unedited parse gives a document that means the same."""
    doc = rkdl.parse(text)
    try:
        return snapshot(rkdl.parse(str(doc))) == snapshot(doc)
    except rkdl.ParseError:
        return False


def collect():
    cases = []
    for file in valid_files():
        text = read_case(file)
        if not reparses(text):
            continue  # test_spec.py's test_round_trip reports these
        for path, node in walk(rkdl.parse(text)):
            for edit in EDITS:
                if applies(edit, node):
                    cases.append(
                        pytest.param(file.name, path, edit, id=f"{file.name}:{'.'.join(map(str, path))}:{edit}")
                    )
    return cases


CASES = collect()


@functools.cache
def run(file, path, edit):
    """Returns the text before and after the edit, and the expected snapshot."""
    doc = rkdl.parse(read_case(SPEC_INPUT / file))
    old = str(doc)
    expected = snapshot(doc)
    model = EDITS[edit](doc, path)
    model(expected)
    return old, str(doc), expected


ARGS = ("file", "path", "edit")


# The checks.


@pytest.mark.parametrize(ARGS, CASES)
def test_meaning(file, path, edit):
    _, new, expected = run(file, path, edit)
    assert snapshot(rkdl.parse(new)) == expected


def common_prefix(a, b):
    n = min(len(a), len(b))
    i = 0
    while i < n and a[i] == b[i]:
        i += 1
    return i


def common_suffix(a, b):
    n = min(len(a), len(b))
    i = 0
    while i < n and a[-1 - i] == b[-1 - i]:
        i += 1
    return i


@pytest.mark.parametrize(ARGS, CASES)
def test_untouched(file, path, edit):
    old, new, _ = run(file, path, edit)
    spans = scan(old)
    span = span_at(spans, path)
    p, s = common_prefix(old, new), common_suffix(old, new)
    if edit in SIBLING_EDITS:
        # A pure insertion, at the node's start (before) or after its body (after).
        lo, hi = (span.start, span.core) if edit == "insert_sibling" else (span.body_end, span.end)
        assert len(new) > len(old)
        q_lo, q_hi = max(len(old) - s, lo), min(p, hi)
        assert q_lo <= q_hi, f"not a pure insertion in {lo}..{hi}: prefix {p}, suffix {s}"
        return
    if edit == "pop":
        lo, hi = span.start, span.end
        parent = span_at(spans, path[:-1]) if len(path) > 1 else None
        if parent is not None and len(parent.children) == 1:
            # Emptying a block prints it as `{}` by design, dropping the whitespace before `}`
            # (test_removing_last_child_empties_block).
            hi = parent.block[1]
        # A node that was next on the same line takes over the start of the line, so the spaces before it go.
        while hi < len(old) and old[hi] in SPACES:
            hi += 1
    elif edit in HEADER_EDITS:
        lo, hi = span.core, span.block[0] if span.block else span.end
    else:
        lo, hi = span.core, span.end
    tail = len(old) - hi
    assert p >= lo, f"text before the node changed at {p} (node starts at {lo})"
    assert s >= tail, f"text after the node changed at {len(old) - s} (node ends at {hi})"
    assert len(new) - tail >= lo


def before_on_line(text, i):
    """The text between the start of i's line and i."""
    j = i
    while j > 0 and not any(text.startswith(nl, j - len(nl)) for nl in NEWLINES):
        j -= 1
    return text[j:i].lstrip("\ufeff")


def indent_of_line(text, i):
    before = before_on_line(text, i)
    return before[: len(before) - len(before.lstrip("".join(SPACES)))]


def single_line(text, block):
    a, b = block
    return not any(nl in text[a:b] for nl in NEWLINES)


@pytest.mark.parametrize(ARGS, [p for p in CASES if p.values[2] in CHILD_EDITS | SIBLING_EDITS])
def test_indent(file, path, edit):
    old, new, _ = run(file, path, edit)
    old_spans, new_spans = scan(old), scan(new)
    if len(path) > 1 and single_line(old, span_at(old_spans, path[:-1]).block):
        pytest.skip("single-line block")
    node = span_at(old_spans, path)
    if edit in CHILD_EDITS:
        added = span_at(new_spans, (*path, 0 if edit == "insert_child" else len(node.children or [])))
        got = before_on_line(new, added.core)
        if not node.children:
            parent = indent_of_line(old, node.core)
            assert got.strip("".join(SPACES)) == "" and got.startswith(parent) and len(got) > len(parent), (
                f"new child placed after {got!r} under parent indent {parent!r}"
            )
            return
        if single_line(old, node.block):
            pytest.skip("single-line block")
        want = indent_of_line(old, node.children[0 if edit == "insert_child" else -1].core)
    else:
        added = span_at(new_spans, (*path[:-1], path[-1] + (edit == "append_sibling")))
        got = before_on_line(new, added.core)
        want = indent_of_line(old, node.core)
    assert got == want, f"new node placed after {got!r}, sibling indent {want!r}"


def test_scanner_agrees_with_rkdl():
    """The scanner finds the same tree as rkdl on every document it's used on, before any edit."""

    def shape(nodes):
        return [None if n.children is None else shape(n.children) for n in nodes]

    def span_shape(spans):
        return [None if s.block is None else span_shape(s.children) for s in spans]

    for file in valid_files():
        if reparses(read_case(file)):
            doc = rkdl.parse(read_case(file))
            assert span_shape(scan(str(doc))) == shape(doc.children), file.name
