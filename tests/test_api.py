"""The convenience API: keeping spellings on write, building nodes, load and dump, and the list and dict operations."""

import copy
import io

import pytest
import rkdl
from conftest import is_valid_case, run_threads, spec_inputs, walk


def node(text):
    doc = rkdl.parse(text)
    return doc, doc.children[0]


class Celsius:
    def __init__(self, degrees):
        self.degrees = degrees

    def __eq__(self, other):
        return isinstance(other, Celsius) and other.degrees == self.degrees


def strict_celsius(value):
    if not isinstance(value, float):
        raise TypeError("celsius is a float")
    return Celsius(value)


CELSIUS = rkdl.Type("celsius", Celsius, decode=strict_celsius, encode=lambda c: c.degrees)


# A1: writes keep the original spelling


def test_self_assignment_changes_no_file_in_the_corpus():
    changed = []
    for path in spec_inputs():
        if not is_valid_case(path):
            continue
        try:
            doc = rkdl.parse(path.read_bytes().decode("utf-8"))
        except (UnicodeDecodeError, rkdl.ParseError):
            continue  # kdl-rs rejects a few valid files
        before = str(doc)
        for _, n in walk(doc):
            for i in range(len(n.args)):
                n.args[i] = n.args[i]
            for key in list(n.props):
                n.props[key] = n.props[key]
        if str(doc) != before:
            changed.append(path.name)
    assert changed == []


@pytest.mark.parametrize(
    "text",
    ["n 0x10", "n #nan", "n (u8)7", 'n #"r"#', "n 1_000", "n 1.5e10", 'n """\n  a\n  b\n  """', "n (celsius)2.5"],
)
def test_writing_the_same_value_changes_nothing(text):
    doc = rkdl.Parser(types=[CELSIUS]).parse(text)
    doc["n"].args[0] = doc["n"].args[0]
    doc["n"].args.entry(0).value = doc["n"].args[0]
    assert str(doc) == text


def test_writing_another_float_zero_is_a_write():
    doc, n = node("n 0.0")
    n.args[0] = -0.0
    assert str(doc) == "n -0.0"


@pytest.mark.parametrize(
    ("text", "value", "expected"),
    [
        ('n "a"', "b", 'n "b"'),
        ("n a", "b", "n b"),
        ("n a", "two words", 'n "two words"'),
        ('n #"a\\b"#', "c\\d", 'n #"c\\d"#'),
        ('n ##"a"##', "b", 'n ##"b"##'),
        # A raw string takes more hashes when the value needs them.
        ('n #"a"#', 'x"#y', 'n ##"x"#y"##'),
        # A raw string can't hold a line break, or a code point KDL bans, so those are quoted instead.
        ('n #"a"#', "x\ny", 'n "x\\ny"'),
        ('n #"a"#', "bell\x07", 'n "bell\\u{7}"'),
        ('n """\n  a\n  b\n  """', "c\nd", 'n """\n  c\n  d\n  """'),
        ('n """\n    a\n    """', 'say """hi"""\n', 'n """\n    say ""\\"hi""\\"\n\n    """'),
        ('n #"""\n  a\n  """#', "c\\n\nd", 'n #"""\n  c\\n\n  d\n  """#'),
        # A multi-line string without a line break in the new value goes on one line, staying raw if it was.
        ('n """\n  a\n  """', "b", 'n "b"'),
        ('n #"""\n  a\n  """#', "b", 'n #"b"#'),
        # A line of only whitespace can't be written in a multi-line string.
        ('n """\n  a\n  """', "x\n  \ny", 'n "x\\n  \\ny"'),
        ('n "a"', 5, "n 5"),
    ],
)
def test_strings_keep_their_quoting(text, value, expected):
    doc, n = node(text)
    n.args[0] = value
    assert str(doc) == expected
    assert rkdl.parse(expected)["n"].args[0] == value


def test_multi_line_strings_keep_crlf():
    doc, n = node('n """\r\n  a\r\n  """\r\n')
    n.args[0] = "b\nc"
    assert str(doc) == 'n """\r\n  b\r\n  c\r\n  """\r\n'


@pytest.mark.parametrize(
    ("text", "value", "expected"),
    [
        ("n 0x10", 255, "n 0xff"),
        ("n 0xAB", 255, "n 0xFF"),
        ("n 0o7", 8, "n 0o10"),
        ("n 0b1", -5, "n -0b101"),
        ("n 1_000", 1234567, "n 1_234_567"),
        ("n 0xdead_beef", 0x1_0000_0000, "n 0x1_0000_0000"),
        ("n 12_34", 123456, "n 12_34_56"),
        # Irregular grouping is dropped, and the radix kept.
        ("n 123_4", 5, "n 5"),
        ("n 0x1_00_0", 1, "n 0x1"),
        ("n +1", 2, "n +2"),
        ("n +1", -2, "n -2"),
        ("n 1.5e10", 2.5e-7, "n 2.5e-7"),
        ("n 1E+3", 2.0e5, "n 2E+5"),
        ("n 1.5e10", float("inf"), "n #inf"),
        ("n 1.5", 2.25, "n 2.25"),
        ("n 0x10", 1.5, "n 1.5"),
        ("n 1.5", 2, "n 2"),
    ],
)
def test_numbers_keep_their_radix_grouping_and_exponent(text, value, expected):
    doc, n = node(text)
    n.args[0] = value
    assert str(doc) == expected
    assert rkdl.parse(expected)["n"].args[0] == value


def test_props_and_annotations_keep_their_spelling():
    doc, n = node('n "k"=0x1 ("t")"v" (u16)0xff')
    n.props["k"] = 2
    n.args[0] = "w"
    n.args[1] = 256
    assert str(doc) == 'n "k"=0x2 ("t")"w" (u16)0x100'
    n.args.entry(0).type = "other"
    assert str(doc) == 'n "k"=0x2 ("other")"w" (u16)0x100'


def test_quoted_names_keys_and_types_stay_quoted():
    doc, n = node('("t")"name" "key"=1 bare=2')
    n.name = "new"
    n.type = "u"
    n.props.entry("key").name = "renamed"
    n.props.entry("bare").name = "also"
    assert str(doc) == '("u")"new" "renamed"=1 also=2'
    n.name = "new"
    assert str(doc) == '("u")"new" "renamed"=1 also=2'


# A2: Node(..., type=...)


def test_node_type_must_be_a_string():
    with pytest.raises(TypeError, match=r'props=\{"type"'):
        rkdl.Node("input", type=5)
    node = rkdl.Node("input", type=None, props={"type": 5})
    assert str(node) == "input type=5"
    assert str(rkdl.Node("input", type="text")) == "(text)input"


def test_node_rejects_a_key_given_twice():
    with pytest.raises(ValueError, match='"a"'):
        rkdl.Node("x", props={"a": 1}, a=2)


def test_node_children_must_be_nodes():
    with pytest.raises(TypeError, match=r'props=\{"children"'):
        rkdl.Node("x", children=5)
    with pytest.raises(TypeError):
        rkdl.Node("x", children=[rkdl.Node("a"), "b"])
    assert str(rkdl.Node("x", props={"children": 5})) == "x children=5"


# A3: load and dump


CRLF = b"a 1\r\nb {\r\n    c\r\n}\r\n"


def test_load_and_dump_through_a_path_keep_bytes(tmp_path):
    source, target = tmp_path / "in.kdl", tmp_path / "out.kdl"
    source.write_bytes(CRLF)
    doc = rkdl.load(source)
    doc["b"].children.append(rkdl.Node("d"))
    rkdl.dump(doc, str(target))
    assert target.read_bytes() == b"a 1\r\nb {\r\n    c\r\n    d\r\n}\r\n"
    assert str(rkdl.load(str(source))) == CRLF.decode()


def test_load_and_dump_through_binary_files_keep_bytes(tmp_path):
    doc = rkdl.load(io.BytesIO(CRLF))
    out = io.BytesIO()
    rkdl.dump(doc, out)
    assert out.getvalue() == CRLF
    path = tmp_path / "x.kdl"
    with open(path, "wb") as f:
        rkdl.dump(doc, f)
    with open(path, "rb") as f:
        assert str(rkdl.load(f)) == CRLF.decode()
    assert path.read_bytes() == CRLF


def test_load_and_dump_through_text_files(tmp_path):
    out = io.StringIO()
    rkdl.dump(rkdl.load(io.StringIO("a 1\n")), out)
    assert out.getvalue() == "a 1\n"
    path = tmp_path / "x.kdl"
    path.write_bytes(CRLF)
    with open(path, newline="") as f:
        assert str(rkdl.load(f)) == CRLF.decode()


def test_load_rejects_bytes_that_are_not_utf8():
    with pytest.raises(UnicodeDecodeError):
        rkdl.load(io.BytesIO(b"a \xff"))


# A4: nodes with custom types


def test_node_only_knows_builtin_types():
    with pytest.raises(TypeError):
        rkdl.Node("t", Celsius(4.0))


def test_parser_node_uses_the_parsers_types():
    parser = rkdl.Parser(types=[CELSIUS])
    n = parser.node("t", Celsius(4.0), type="x", props={"max": Celsius(9.5)}, children=[parser.node("c")])
    assert str(n) == "(x)t (celsius)4.0 max=(celsius)9.5 {\n    c\n}"
    assert n.args[0] == Celsius(4.0)
    n.args.append(Celsius(1.5))
    assert n.args[1] == Celsius(1.5)


def test_doc_node_uses_the_documents_types_and_does_not_insert():
    doc = rkdl.Parser(types=[CELSIUS]).parse("a\n")
    n = doc.node("t", Celsius(4.0))
    assert len(doc) == 1
    assert n.parent is None
    doc.children.append(n)
    assert str(doc) == "a\nt (celsius)4.0\n"
    assert doc["t"].args[0] == Celsius(4.0)


def test_doc_node_takes_the_documents_line_endings():
    doc = rkdl.parse("a\r\n")
    n = doc.node("t", children=[rkdl.Node("c")])
    assert str(n) == "t {\r\n    c\r\n}"


# A5: plain writes to custom-typed entries


def test_a_plain_value_the_type_reads_keeps_the_annotation():
    doc = rkdl.Parser(types=[CELSIUS]).parse("n (celsius)21.5 (celsius)1.0\n")
    n = doc["n"]
    n.args[0] = 22.0
    assert str(doc) == "n (celsius)22.0 (celsius)1.0\n"
    assert n.args[0] == Celsius(22.0)
    # strict_celsius refuses an int, so the annotation goes.
    n.args[1] = 3
    assert str(doc) == "n (celsius)22.0 3\n"


def test_a_plain_write_to_a_builtin_type_is_unchanged():
    doc, n = node("n (u8)1 (u8)2\n")
    n.args[0] = 3
    n.args[1] = 300
    assert str(doc) == "n (u8)3 300\n"


def test_a_decode_that_raises_a_base_exception_propagates():
    def stop(value):
        raise KeyboardInterrupt

    stopping = rkdl.Type("stop", Celsius, decode=stop, encode=lambda c: c.degrees)
    doc = rkdl.Parser(types=[stopping]).parse("n 1\n")
    entry = doc["n"].args.entry(0)
    with pytest.raises(KeyboardInterrupt):
        entry.type = "stop"


# A6: iteration and membership


def test_iterating_a_document_or_node_gives_its_children():
    doc = rkdl.parse("a { b; c }\nd\n")
    assert [n.name for n in doc] == ["a", "d"]
    assert [n.name for n in doc["a"]] == ["b", "c"]
    assert list(doc["d"]) == []


def test_in_tests_by_key():
    doc = rkdl.parse("(t)a { b }\nd\n")
    assert "a" in doc
    assert ("t", "a") in doc
    assert (None, "d") in doc
    assert "b" not in doc
    assert "b" in doc["a"]
    assert "x" not in doc["d"]
    with pytest.raises(TypeError, match="children"):
        _ = doc["d"] in doc
    with pytest.raises(TypeError):
        _ = 5 in doc


def test_iter_searches_descendants_depth_first():
    doc = rkdl.parse("a { b { x }; x }\n(t)x { x }\n")
    assert [n.name for n in doc.iter()] == ["a", "b", "x", "x", "x", "x"]
    found = list(doc.iter("x"))
    assert [n.parent.name if n.parent else None for n in found] == ["b", "a", None, "x"]
    assert list(doc.iter(("t", "x"))) == [doc[("t", "x")]]
    assert [n.name for n in doc["a"].iter()] == ["b", "x", "x"]
    assert list(doc["a"]["b"]["x"].iter()) == []


def test_iter_walks_a_snapshot():
    doc = rkdl.parse("a { b }\nc\n")
    seen = []
    for n in doc.iter():
        seen.append(n.name)
        n.detach()
    assert seen == ["a", "b", "c"]
    assert str(doc) == ""


def test_iter_handles_deep_documents():
    depth = 900
    doc = rkdl.parse("a {" * depth + "}" * depth)
    assert sum(1 for _ in doc.iter()) == depth


# A6: building and replacing children


def test_node_children_builds_a_tree():
    tree = rkdl.Node("a", children=[rkdl.Node("b", 1), rkdl.Node("c", children=[])])
    assert str(tree) == "a {\n    b 1\n    c {}\n}"
    assert str(rkdl.Node("a", children=[])) == "a {}"
    assert str(rkdl.Node("a", children=None)) == "a"


def test_assigning_children_replaces_them():
    doc = rkdl.parse("p {\n    old 1\n}\nq\n")
    old = doc["p"]["old"]
    doc["p"].children = (rkdl.Node(n) for n in ["x", "y"])
    assert str(doc) == "p {\n    x\n    y\n}\nq\n"
    assert old.parent is None
    doc["q"].children = [old]
    assert str(doc) == "p {\n    x\n    y\n}\nq {\n    old 1\n}\n"
    doc.children = [rkdl.Node("only")]
    assert str(doc) == "only\n"


def test_assigning_children_is_all_or_nothing():
    doc = rkdl.parse("p {\n    a\n}\nq\n")
    new = rkdl.Node("new")
    for bad in ([new, doc["q"]], [new, new], [new, "x"]):
        with pytest.raises((ValueError, TypeError)):
            doc["p"].children = bad
        assert str(doc) == "p {\n    a\n}\nq\n"
        assert new.parent is None
    with pytest.raises(ValueError, match="inside itself"):
        new.children = [new]
    assert str(new) == "new"


def test_inserting_a_node_inside_itself_names_the_problem():
    outer = rkdl.Node("outer", children=[rkdl.Node("inner")])
    with pytest.raises(ValueError, match="inside itself"):
        outer["inner"].children = [outer]
    with pytest.raises(ValueError, match="inside itself"):
        outer.children.extend([rkdl.Node("ok"), outer])
    assert str(outer) == "outer {\n    inner\n}"


def test_children_slice_assignment():
    doc = rkdl.parse("p {\n    a\n    b\n    c\n    d\n}\n")
    p = doc["p"]
    b, c = p["b"], p["c"]
    p.children[1:3] = [rkdl.Node("x")]
    assert [n.name for n in p] == ["a", "x", "d"]
    assert b.parent is None and c.parent is None
    p.children[::2] = [rkdl.Node("m"), rkdl.Node("n")]
    assert [n.name for n in p] == ["m", "x", "n"]
    p.children[3:] = [b, c]
    assert [n.name for n in p] == ["m", "x", "n", "b", "c"]
    with pytest.raises(ValueError, match="extended slice of size 3"):
        p.children[::2] = [rkdl.Node("y")]
    p.children[:] = []
    assert str(doc) == "p {}\n"


def test_children_extend_is_all_or_nothing():
    doc = rkdl.parse("p { a }\nq\n")
    new = rkdl.Node("new")
    with pytest.raises(ValueError, match="already in a document"):
        doc["p"].children.extend([new, doc["q"]])
    assert str(doc) == "p { a }\nq\n"
    assert new.parent is None


def test_assigning_children_while_another_thread_removes_the_block():
    doc = rkdl.parse("p\n")
    p = doc["p"]
    rounds = 300

    def open_block():
        for _ in range(rounds):
            p.children = []

    def close_block():
        for _ in range(rounds):
            p.children = None

    run_threads(open_block, close_block)
    assert str(doc) in ("p\n", "p {}\n")


# A6: args and props


def test_args_slice_assignment():
    doc, n = node("n 1 k=v 2 3\n")
    n.args[1:3] = [7, 8, 9]
    assert str(doc) == "n 1 k=v 7 8 9\n"
    n.args[::2] = ["a", "b"]
    assert n.args == ["a", 7, "b", 9]
    with pytest.raises(ValueError, match="extended slice"):
        n.args[::2] = [1]
    n.args[1:] = []
    assert str(doc) == "n a k=v\n"
    # New arguments go after the last one, as `append` puts them.
    n.args[5:0] = [1, 2]
    assert str(doc) == "n a 1 2 k=v\n"


def test_args_slice_assignment_is_all_or_nothing():
    doc, n = node("n 1 2\n")
    with pytest.raises(TypeError):
        n.args[0:1] = [5, object()]
    assert str(doc) == "n 1 2\n"


def test_assigning_args_and_props_replaces_them():
    doc, n = node("n 0x1 2 a=1 b=2 a=3\n")
    n.args = [0x10, "x"]
    assert str(doc) == "n 0x10 x a=1 b=2 a=3\n"
    n.props = {"b": 5, "c": 6}
    assert str(doc) == "n 0x10 x b=5 c=6\n"
    n.args = []
    n.props = {}
    assert str(doc) == "n\n"


def test_augmented_assignment_to_args_and_props():
    doc, n = node("n 1 a=1\n")
    n.args[0] = 5
    n.args += [2]
    n.props |= {"b": 2}
    n.props |= [("c", 3)]
    assert str(doc) == "n 5 2 a=1 b=2 c=3\n"


def test_props_or_and_copy_are_plain_dicts():
    _, n = node("n a=1 b=2\n")
    assert n.props | {"b": 3} == {"a": 1, "b": 3}
    assert {"b": 3, "z": 0} | n.props == {"b": 2, "z": 0, "a": 1}
    assert type(n.props | {}) is dict
    copied = n.props.copy()
    assert type(copied) is dict and copied == {"a": 1, "b": 2}
    with pytest.raises(TypeError):
        _ = n.props | [("a", 1)]


def test_props_update_is_all_or_nothing():
    doc, n = node("n a=1\n")
    with pytest.raises(TypeError):
        n.props.update({"a": 2, "b": object()})
    with pytest.raises(TypeError):
        n.props.update({"c": 3, 4: 5})
    assert str(doc) == "n a=1\n"
    n.props.update([("b", 1), ("b", 2)], c=3)
    assert str(doc) == "n a=1 b=2 c=3\n"


def test_props_setdefault():
    doc, n = node("n a=1\n")
    assert n.props.setdefault("a", 5) == 1
    assert n.props.setdefault("b", 5) == 5
    assert n.props.setdefault("c") is None
    with pytest.raises(TypeError):
        n.props.setdefault("d", object())
    assert str(doc) == "n a=1 b=5 c=#null\n"


def test_index_takes_start_and_stop():
    doc = rkdl.parse("n 1 2 1 2\np { a; b; a }\n")
    args = doc["n"].args
    assert args.index(1, 1) == 2
    assert args.index(2, -1) == 3
    with pytest.raises(ValueError):
        args.index(1, 1, 2)
    children = doc["p"].children
    first, second = doc["p"].get_all("a")
    assert children.index(second, 1) == 2
    assert children.index(first, -5, 1) == 0
    with pytest.raises(ValueError):
        children.index(first, 1)


def test_args_and_children_equal_lists():
    doc = rkdl.parse("n 1 2 { a; b }\n")
    n = doc["n"]
    assert n.args == [1, 2]
    assert n.args != [1]
    assert n.args != (1, 2)
    assert n.args == n.args
    assert n.children == [n["a"], n["b"]]
    assert n.children != [n["b"], n["a"]]
    assert n.children != (n["a"], n["b"])
    assert n.children != [1, 2]


# A6: parent and detach


def test_parent():
    doc = rkdl.parse("a { b { c } }\n")
    assert doc["a"].parent is None
    assert doc["a"]["b"].parent is doc["a"]
    assert doc["a"]["b"]["c"].parent is doc["a"]["b"]
    with pytest.raises(AttributeError):
        doc["a"]["b"].parent = None


def test_detach_takes_a_node_out_from_anywhere():
    doc = rkdl.parse("a {\n    b {\n        c\n    }\n}\nd\n")
    b = doc["a"]["b"]
    c = b["c"]
    assert c.detach() is c
    assert str(doc) == "a {\n    b {}\n}\nd\n"
    assert c.parent is None
    assert c.detach() is c
    assert doc["d"].detach().name == "d"
    assert str(doc) == "a {\n    b {}\n}\n"
    doc["a"].children.append(c)
    assert c.parent is doc["a"]


# A6: documents and copies


def test_an_empty_document():
    doc = rkdl.Document()
    assert str(doc) == ""
    doc.children.append(rkdl.Node("a"))
    assert str(doc) == "a\n"


def test_document_copy_keeps_types_and_shares_nothing():
    doc = rkdl.Parser(types=[CELSIUS]).parse("t (celsius)1.0\r\n")
    for copied in (doc.copy(), copy.copy(doc), copy.deepcopy(doc)):
        assert str(copied) == str(doc)
        assert copied["t"].args[0] == Celsius(1.0)
        copied["t"].args.append(Celsius(2.0))
        copied.children.append(rkdl.Node("new"))
        assert str(copied) == "t (celsius)1.0 (celsius)2.0\r\nnew\r\n"
    assert str(doc) == "t (celsius)1.0\r\n"


def test_node_copies():
    doc = rkdl.parse("a 1 { b }\n")
    for copied in (copy.copy(doc["a"]), copy.deepcopy(doc["a"])):
        assert copied is not doc["a"]
        assert copied.parent is None
        assert str(copied) == str(doc["a"])
    holder = copy.deepcopy({"x": doc["a"]})
    assert holder["x"].parent is None


# A6: renaming a property


def test_renaming_a_property_keeps_its_place_and_text():
    doc, n = node("n a=0x10 1 b=2\n")
    entry = n.props.entry("a")
    entry.name = "two words"
    assert str(doc) == 'n "two words"=0x10 1 b=2\n'
    assert n.props == {"two words": 16, "b": 2}
    with pytest.raises(ValueError, match="argument"):
        n.args.entry(0).name = "x"
    with pytest.raises(ValueError, match="argument"):
        entry.name = None
    assert str(doc) == 'n "two words"=0x10 1 b=2\n'


# reprs never raise


def test_reprs_of_removed_and_blockless_things():
    doc, n = node("n 1 {\n    c\n}\n")
    entry = n.args.entry(0)
    del n.args[0]
    assert repr(entry) == "<Entry (removed)>"
    children = n.children
    n.children = None
    assert repr(children) == "Children(<no children block>)"
    assert repr(doc) == "<Document with 1 nodes>"


def test_reprs_survive_a_raising_repr():
    class Loud:
        def __repr__(self):
            raise RuntimeError("no")

    loud = rkdl.Type("loud", Loud, decode=lambda v: Loud(), encode=lambda v: 1)
    doc = rkdl.Parser(types=[loud]).parse("n (loud)1 k=(loud)2\n")
    n = doc["n"]
    assert repr(n.args) == "Args([1])"
    assert repr(n.props) == "Props({'k': 2})"
    assert repr(n.args.entry(0)) == "<Entry (loud)1>"
