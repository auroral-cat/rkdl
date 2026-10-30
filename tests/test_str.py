import pytest
import rkdl

NESTED = """\
// above parent
parent 1 key=v {
    // above child
    child /* inline */ 2
    /-gone
    other { deep; }
}
// after
next
"""


def test_str_node_keeps_inner_comments():
    doc = rkdl.parse(NESTED)
    assert str(doc) == NESTED
    assert str(doc["parent"]) == (
        "parent 1 key=v {\n    // above child\n    child /* inline */ 2\n    /-gone\n    other { deep; }\n}"
    )
    # The comment above a node is part of that node's leading text, which str(node) leaves out.
    assert str(doc["next"]) == "next"
    assert str(doc["parent"]["child"]) == "child /* inline */ 2"


@pytest.mark.xfail(strict=True, reason="kdl-rs: comments between `}` and the newline are dropped, newline and all")
@pytest.mark.parametrize("comment", ["// end", "/* end */"])
def test_comment_after_a_block_survives(comment):
    text = f"a {{}} {comment}\nb\n"
    assert str(rkdl.parse(text)) == text


def test_str_node_reflects_edits():
    doc = rkdl.parse("a 1 {\n    b\n}\n")
    doc["a"].args[0] = "x y"
    doc["a"].props["k"] = 2
    assert str(doc["a"]) == 'a "x y" k=2 {\n    b\n}'


def test_str_detached_node():
    node = rkdl.Node("server", "web", type="svc", port=8080)
    assert str(node) == "(svc)server web port=8080"
    assert repr(node) == "<Node server>"


@pytest.mark.parametrize("value", ["+.7", "-.7", "+.5e3", "-.5"])
def test_ambiguous_string_round_trips(value):
    doc = rkdl.parse("n\n")
    doc["n"].props["k"] = value
    doc["n"].args.append(value)
    again = rkdl.parse(str(doc))
    assert again["n"].props["k"] == value
    assert again["n"].args[0] == value


def test_plain_string_stays_bare():
    doc = rkdl.parse("n\n")
    doc["n"].props["k"] = "plain.value"
    assert str(doc) == "n k=plain.value\n"


def round_trip(doc):
    return rkdl.parse(str(doc))


@pytest.mark.parametrize("name", ["+.7", "-.5", "+.1e2"])
def test_ambiguous_node_name_round_trips(name):
    doc = rkdl.parse("a\n")
    doc["a"].name = name
    doc.children.append(rkdl.Node(name, name, type=name, props={name: name}))
    again = round_trip(doc)
    assert [n.name for n in again.children] == [name, name]
    new = again.children[1]
    assert new.type == name
    assert new.args[0] == name
    assert new.props[name] == name


@pytest.mark.parametrize("key", ["-.5", "+.7"])
def test_ambiguous_prop_key_round_trips(key):
    doc = rkdl.parse("n 1\n")
    doc["n"].props[key] = 2
    assert round_trip(doc)["n"].props[key] == 2


@pytest.mark.parametrize("ty", ["+.1", "-.5"])
def test_ambiguous_type_round_trips(ty):
    doc = rkdl.parse("n 1 k=2\n")
    doc["n"].type = ty
    doc["n"].args.entry(0).type = ty
    doc["n"].props.entry("k").type = ty
    again = round_trip(doc)["n"]
    assert again.type == ty
    assert again.args.entry(0).type == ty
    assert again.props.entry("k").type == ty


def test_plain_identifiers_stay_bare():
    doc = rkdl.parse("n\n")
    node = doc["n"]
    node.name = "plain.name"
    node.type = "t"
    node.props["k.ey"] = 1
    assert str(doc) == "(t)plain.name k.ey=1\n"
