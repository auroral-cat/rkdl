"""`rkdl.Value`: writing a value together with its type annotation."""

import copy
import datetime
import decimal
import gc
import pickle
import re
import weakref

import pytest
import rkdl
from rkdl import Value


class Celsius:
    def __init__(self, degrees):
        self.degrees = degrees

    def __eq__(self, other):
        return isinstance(other, Celsius) and other.degrees == self.degrees


def strict_celsius(v):
    if not isinstance(v, float):
        raise TypeError("celsius is a float")
    return Celsius(v)


CELSIUS = rkdl.Type("celsius", Celsius, decode=strict_celsius, encode=lambda c: c.degrees)
NEW_YEAR = datetime.date(2029, 1, 1)


# The object itself.


def test_value_holds_a_value_and_an_annotation():
    port = Value(8080, type="u16")
    assert port.value == 8080
    assert port.type == "u16"
    assert Value(8080).type is None
    assert Value(8080, None).type is None


def test_value_repr():
    assert repr(Value(8080, type="u16")) == "Value(8080, type='u16')"
    assert repr(Value(NEW_YEAR)) == "Value(datetime.date(2029, 1, 1))"


def test_value_equality_and_hash():
    assert Value(1, "u8") == Value(1, "u8")
    assert Value(1, "u8") == Value(1.0, "u8")
    assert Value(1, "u8") != Value(1, "u16")
    assert Value(1, "u8") != Value(1)
    assert Value(1) != 1
    assert hash(Value(1, "u8")) == hash(Value(1, "u8"))
    assert len({Value(1, "u8"), Value(1, "u8"), Value(1)}) == 2
    with pytest.raises(TypeError):
        hash(Value([1]))


def test_value_is_frozen():
    port = Value(8080, type="u16")
    with pytest.raises(AttributeError):
        port.value = 1
    with pytest.raises(AttributeError):
        port.type = "u8"


def test_value_matches_by_position():
    match Value(8080, type="u16"):
        case Value(port, "u16"):
            assert port == 8080
        case _:
            pytest.fail("didn't match")


@pytest.mark.parametrize("dup", [copy.copy, copy.deepcopy, lambda v: pickle.loads(pickle.dumps(v))])
def test_value_copies_and_pickles(dup):
    for original in [Value(8080, type="u16"), Value(NEW_YEAR), Value([1, 2], type="mine")]:
        assert dup(original) == original
    nested = Value([1], type="mine")
    assert copy.deepcopy(nested).value is not nested.value
    assert copy.copy(nested).value is nested.value


def test_value_refuses_a_type_that_isnt_a_str():
    with pytest.raises(TypeError, match="^type must be a str or None, not int$"):
        Value(1, 8)


def test_value_refuses_another_value():
    with pytest.raises(TypeError, match="can't hold another Value"):
        Value(Value(1, "u8"), "u16")


class Marker:
    pass


def test_a_cycle_through_a_value_is_collected():
    # A list can't be weakly referenced, so a marker in the cycle stands in for it.
    box = [Marker()]
    box.append(Value(box, type="mine"))
    marker_ref = weakref.ref(box[0])
    del box
    gc.collect()
    assert marker_ref() is None


# Writing one, through every route.

# Writes over an existing entry, which starts as `(u8)1`.
OVER_ROUTES = {
    "args[i]": lambda n, v: n.args.__setitem__(0, v),
    "args slice": lambda n, v: n.args.__setitem__(slice(0, 1), [v]),
    "node.args": lambda n, v: setattr(n, "args", [v]),
    "arg entry.value": lambda n, v: setattr(n.args.entry(0), "value", v),
    "props[k]": lambda n, v: n.props.__setitem__("k", v),
    "props.update": lambda n, v: n.props.update(k=v),
    "props |=": lambda n, v: n.props.__ior__({"k": v}),
    "node.props": lambda n, v: setattr(n, "props", {"k": v}),
    "prop entry.value": lambda n, v: setattr(n.props.entry("k"), "value", v),
}

# Writes that make a new entry on a node with none.
NEW_ROUTES = {
    "append": lambda n, v: n.args.append(v),
    "insert": lambda n, v: n.args.insert(0, v),
    "extend": lambda n, v: n.args.extend([v]),
    "args +=": lambda n, v: n.args.__iadd__([v]),
    "args slice grows": lambda n, v: n.args.__setitem__(slice(0, 0), [v]),
    "node.args": lambda n, v: setattr(n, "args", [v]),
    "new props[k]": lambda n, v: n.props.__setitem__("k", v),
    "new props.update": lambda n, v: n.props.update({"k": v}),
    "new node.props": lambda n, v: setattr(n, "props", {"k": v}),
    "props.setdefault": lambda n, v: n.props.setdefault("k", v),
}

# How each route builds a node with the entry: positional or keyword, through each way to make a node.
BUILD_ROUTES = {
    "Node(arg)": lambda v: rkdl.Node("n", v),
    "Node(prop=)": lambda v: rkdl.Node("n", k=v),
    "Node(props=)": lambda v: rkdl.Node("n", props={"k": v}),
    "doc.node(arg)": lambda v: rkdl.parse("").node("n", v),
    "parser.node(prop=)": lambda v: rkdl.Parser().node("n", k=v),
}

# (Value, the entry's text afterwards, what it reads as, or the start of the ValueError's message.)
CASES = [
    pytest.param(Value(300, type="u16"), "(u16)300", 300, id="fits"),
    pytest.param(Value("2029-01-01", type="date"), '(date)"2029-01-01"', NEW_YEAR, id="plain-it-reads"),
    pytest.param(Value(NEW_YEAR, type="date"), '(date)"2029-01-01"', NEW_YEAR, id="its-class"),
    pytest.param(Value(7.0, type="u8"), "(u8)7", 7, id="converts"),
    pytest.param(Value(300, type="u8"), None, "300 doesn't fit (u8): out of range 0..=255", id="out-of-range"),
    pytest.param(
        Value(datetime.datetime(2029, 1, 1, 9), type="date"),
        None,
        "datetime.datetime(2029, 1, 1, 9, 0) doesn't fit (date): would lose its time",
        id="wrong-class",
    ),
    pytest.param(Value("weekly", type="u8"), None, "'weekly' doesn't fit (u8): not an integer", id="plain-misfit"),
    pytest.param(Value(NEW_YEAR, type="mine"), '(mine)"2029-01-01"', "2029-01-01", id="unknown-non-plain"),
    pytest.param(Value("x", type="mine"), "(mine)x", "x", id="unknown-plain"),
    pytest.param(Value(NEW_YEAR), '"2029-01-01"', "2029-01-01", id="none-non-plain"),
    pytest.param(Value(7), "7", 7, id="none-plain"),
    pytest.param(Value(decimal.Decimal("1.10")), '"1.10"', "1.10", id="none-decimal"),
    pytest.param(
        Value(decimal.Decimal("1.10"), type="decimal64"), "(decimal64)1.10", decimal.Decimal("1.10"), id="d64"
    ),
]


def written(n, route):
    """The entry a route writes: property `k`, or the first argument."""
    return n.props.entry("k") if "prop" in route else n.args.entry(0)


def check(doc, entry, text, read):
    assert str(entry).endswith(text)
    assert entry.value == read
    assert type(entry.value) is type(read)
    assert str(rkdl.parse(str(doc))) == str(doc)


@pytest.mark.parametrize("route", OVER_ROUTES)
@pytest.mark.parametrize(("value", "text", "read"), CASES)
def test_writing_a_value_over_an_entry(route, value, text, read):
    before = "n (u8)1 k=(u8)1\n"
    doc = rkdl.parse(before)
    n = doc["n"]
    if text is None:
        with pytest.raises(ValueError, match="^" + re.escape(read)):
            OVER_ROUTES[route](n, value)
        assert str(doc) == before
    else:
        OVER_ROUTES[route](n, value)
        check(doc, written(n, route), text, read)


@pytest.mark.parametrize("route", NEW_ROUTES)
@pytest.mark.parametrize(("value", "text", "read"), CASES)
def test_writing_a_value_into_a_new_entry(route, value, text, read):
    doc = rkdl.parse("n\n")
    n = doc["n"]
    if text is None:
        with pytest.raises(ValueError, match="^" + re.escape(read)):
            NEW_ROUTES[route](n, value)
        assert str(doc) == "n\n"
    else:
        NEW_ROUTES[route](n, value)
        check(doc, written(n, route), text, read)


@pytest.mark.parametrize("route", BUILD_ROUTES)
@pytest.mark.parametrize(("value", "text", "read"), CASES)
def test_building_a_node_with_a_value(route, value, text, read):
    if text is None:
        with pytest.raises(ValueError, match="^" + re.escape(read)):
            BUILD_ROUTES[route](value)
    else:
        n = BUILD_ROUTES[route](value)
        doc = rkdl.parse("")
        doc.children.append(n)
        check(doc, written(n, route), text, read)


def test_node_example():
    n = rkdl.Node("server", Value(8080, type="u16"), started=Value(NEW_YEAR), tz=Value("UTC", type="mine"))
    assert str(n) == 'server (u16)8080 started="2029-01-01" tz=(mine)UTC'


def test_setdefault_returns_what_the_property_reads_as():
    doc = rkdl.parse("n\n")
    props = doc["n"].props
    assert props.setdefault("when", Value("2029-01-01", type="date")) == NEW_YEAR
    assert props.setdefault("plain", Value(NEW_YEAR)) == "2029-01-01"
    assert props.setdefault("when", Value(5, type="u8")) == NEW_YEAR
    assert str(doc) == 'n when=(date)"2029-01-01" plain="2029-01-01"\n'


# Custom types.


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (Value(Celsius(3.0), type="celsius"), "n (celsius)3.0\n"),
        (Value(4.0, type="celsius"), "n (celsius)4.0\n"),
        (Value(Celsius(3.0)), "n 3.0\n"),
        (Value(Celsius(3.0), type="mine"), "n (mine)3.0\n"),
        (Value(3, type="celsius"), "3 doesn't fit (celsius): its decode raised TypeError('celsius is a float')"),
        (Value(NEW_YEAR, type="celsius"), "datetime.date(2029, 1, 1) doesn't fit (celsius): not a Celsius"),
    ],
)
def test_a_value_with_a_custom_type(value, expected):
    doc = rkdl.Parser(types=[CELSIUS]).parse("n (u8)1\n")
    if expected.startswith("n "):
        doc["n"].args[0] = value
        assert str(doc) == expected
    else:
        with pytest.raises(ValueError, match="^" + re.escape(expected)):
            doc["n"].args[0] = value
        assert str(doc) == "n (u8)1\n"


def test_value_type_none_on_a_value_no_type_claims():
    doc = rkdl.parse("n 1\n")
    with pytest.raises(TypeError, match="can't convert object to a KDL value"):
        doc["n"].args[0] = Value(object())
    assert str(doc) == "n 1\n"


# All or nothing.


def test_a_misfit_among_several_values_changes_nothing():
    text = "n (u8)1 2 k=(u8)3\n"
    doc = rkdl.parse(text)
    n = doc["n"]
    with pytest.raises(ValueError, match=re.escape("300 doesn't fit (u8)")):
        n.args = [Value(5, type="u16"), Value(300, type="u8")]
    with pytest.raises(ValueError, match=re.escape("300 doesn't fit (u8)")):
        n.args.extend([Value(5, type="u16"), Value(300, type="u8")])
    with pytest.raises(ValueError, match=re.escape("300 doesn't fit (u8)")):
        n.props.update(k=Value(5, type="u16"), j=Value(300, type="u8"))
    with pytest.raises(ValueError, match=re.escape("300 doesn't fit (u8)")):
        n.props = {"k": Value(NEW_YEAR), "j": Value(300, type="u8")}
    assert str(doc) == text
    with pytest.raises(ValueError, match=re.escape("300 doesn't fit (u8)")):
        rkdl.Node("n", Value(1, type="u8"), k=Value(300, type="u8"))


# Spelling.


@pytest.mark.parametrize(
    "value",
    [Value(7, type="u8"), Value(7.0, type="u8")],
)
def test_the_same_value_and_annotation_keep_their_spelling(value):
    text = 'n ("u8")0x07 k=(date)#"2029-01-01"#\n'
    doc = rkdl.parse(text)
    doc["n"].args[0] = value
    doc["n"].props["k"] = Value(NEW_YEAR, type="date")
    assert str(doc) == text


def test_a_new_annotation_keeps_the_values_spelling_where_it_can():
    doc = rkdl.parse("n (u8)0x07\n")
    doc["n"].args[0] = Value(8, type="u16")
    assert str(doc) == "n (u16)0x8\n"
    doc["n"].args[0] = Value(8)
    assert str(doc) == "n 0x8\n"


# A plain write over an annotation rkdl doesn't know keeps it, whatever the value is.


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (NEW_YEAR, 'n (mine)"2029-01-01"\n'),
        (decimal.Decimal("1.5"), 'n (mine)"1.5"\n'),
        (b"hi", 'n (mine)"aGk="\n'),
        (Celsius(2.0), "n (mine)2.0\n"),
    ],
)
def test_an_unknown_annotation_stays_over_a_non_plain_value(value, expected):
    doc = rkdl.Parser(types=[CELSIUS]).parse("n (mine)1\n")
    doc["n"].args[0] = value
    assert str(doc) == expected


def test_setdefault_names_the_property_when_its_value_cant_be_read_back():
    # Celsius(3) encodes to the int 3, which strict_celsius refuses to decode.
    doc = rkdl.Parser(types=[CELSIUS]).parse("server\n")
    with pytest.raises(ValueError, match="^" + re.escape("server prop k: 3 doesn't fit (celsius): its decode raised")):
        doc["server"].props.setdefault("k", Value(Celsius(3), type="celsius"))
    assert str(doc) == "server\n"
