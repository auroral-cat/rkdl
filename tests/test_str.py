import pytest

import rkdl

NESTED = """\
// above parent
parent 1 key=v {
    // above child
    child /* inline */ 2
    /-gone
    other { deep; }
} // after
next
"""


def test_str_node_keeps_inner_comments():
    doc = rkdl.parse(NESTED)
    assert str(doc["parent"]) == (
        "parent 1 key=v {\n"
        "    // above child\n"
        "    child /* inline */ 2\n"
        "    /-gone\n"
        "    other { deep; }\n"
        "}"
    )
    # kdl-rs gives a comment after `}` to the next node, as leading text.
    assert str(doc["next"]) == "next"
    assert str(doc["parent"]["child"]) == "child /* inline */ 2"
    assert str(doc["next"]) == "next"


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
