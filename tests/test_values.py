import datetime
import decimal
import enum
import ipaddress
import math
import re
import uuid

import pytest
import rkdl
from conftest import run_threads
from rkdl._rkdl import _debug_from_python as from_python
from rkdl._rkdl import _debug_reserved_types
from rkdl._rkdl import _debug_to_python as to_python


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ('"hi"', "hi"),
        ("hi", "hi"),
        ("0x1F", 31),
        ("-5", -5),
        ("1.5", 1.5),
        ("#true", True),
        ("#false", False),
        ("#null", None),
        ("#inf", math.inf),
        ("#-inf", -math.inf),
        ("170141183460469231731687303715884105727", 2**127 - 1),
    ],
)
def test_plain_to_python(text, expected):
    value = to_python(text)
    assert value == expected
    assert type(value) is type(expected)


def test_nan_to_python():
    assert math.isnan(to_python("#nan"))


def test_unknown_type_stays_plain():
    assert to_python('(mine)"x"') == "x"
    assert to_python("(mine)5") == 5


KNOWN = [
    ("(i8)-128", -128),
    ("(i8)127", 127),
    ("(i16)-32768", -32768),
    ("(i32)2147483647", 2147483647),
    ("(i64)-9223372036854775808", -(2**63)),
    ("(i128)170141183460469231731687303715884105727", 2**127 - 1),
    ("(isize)9223372036854775807", 2**63 - 1),
    ("(u8)255", 255),
    ("(u16)65535", 65535),
    ("(u32)4294967295", 2**32 - 1),
    ("(u64)18446744073709551615", 2**64 - 1),
    ("(u128)0", 0),
    ("(usize)18446744073709551615", 2**64 - 1),
    ("(f32)1.5", 1.5),
    ("(f64)1", 1.0),
    ("(f64)#inf", math.inf),
    ("(decimal64)0.1", decimal.Decimal("0.1")),
    ("(decimal128)12", decimal.Decimal(12)),
    ('(decimal)"3.14159265358979323846264338327950288"', decimal.Decimal("3.14159265358979323846264338327950288")),
    ('(date-time)"2024-05-06T07:08:09+00:00"', datetime.datetime(2024, 5, 6, 7, 8, 9, tzinfo=datetime.timezone.utc)),
    ('(date)"2024-05-06"', datetime.date(2024, 5, 6)),
    ('(time)"07:08:09"', datetime.time(7, 8, 9)),
    ('(ipv4)"10.0.0.1"', ipaddress.IPv4Address("10.0.0.1")),
    ('(ipv6)"::1"', ipaddress.IPv6Address("::1")),
    (
        '(uuid)"12345678-1234-5678-1234-567812345678"',
        uuid.UUID("12345678-1234-5678-1234-567812345678"),
    ),
    ('(base64)"aGk="', b"hi"),
    ('(duration)"P1D"', "P1D"),
    ('(url)"https://kdl.dev"', "https://kdl.dev"),
    ('(email)"a@b.c"', "a@b.c"),
    ('(country-2)"NZ"', "NZ"),
    ('(base85)"87cURD]i"', "87cURD]i"),
]


@pytest.mark.parametrize(("text", "expected"), KNOWN)
def test_known_to_python(text, expected):
    value = to_python(text)
    assert value == expected
    assert type(value) is type(expected)


def test_regex_to_python():
    assert to_python(r'(regex)#"\d+"#') == re.compile(r"\d+")


@pytest.mark.parametrize(
    ("flags", "named"),
    [
        (re.IGNORECASE, "re.IGNORECASE"),
        (re.IGNORECASE | re.MULTILINE, "re.IGNORECASE|re.MULTILINE"),
        (re.ASCII, "re.ASCII"),
    ],
)
def test_regex_flags_cant_be_written(flags, named):
    # KDL keeps only the pattern, so the flags would be lost.
    pattern = re.compile(r"\d", flags)
    with pytest.raises(ValueError, match=re.escape(named)):
        from_python(pattern)
    with pytest.raises(ValueError, match=re.escape(named)):
        from_python(pattern, "regex")
    doc = rkdl.parse("n 1\n")
    with pytest.raises(ValueError, match="put flags inline"):
        doc["n"].args.append(pattern)
    assert str(doc) == "n 1\n"


def test_inline_regex_flags_are_part_of_the_pattern():
    pattern = re.compile(r"(?i)\d")
    assert from_python(pattern) == (r'"(?i)\\d"', "regex")
    assert to_python(r'(regex)"(?i)\\d"') == pattern


def test_decimal_from_integer_is_exact():
    assert to_python("(decimal128)170141183460469231731687303715884105727") == decimal.Decimal(2**127 - 1)


@pytest.mark.parametrize(
    "text",
    [
        "(i8)128",
        "(i8)-129",
        "(u8)256",
        "(u8)-1",
        "(u64)18446744073709551616",
        "(isize)9223372036854775808",
        "(i32)1.5",
        '(i32)"1"',
        "(f32)1e39",
        '(f64)"1.0"',
        "(f64)#null",
        "(decimal64)#true",
        '(decimal64)"1.5"',
        '(decimal)"pi"',
        "(decimal)1.5",
        '(date-time)"yesterday"',
        "(date)5",
        '(date)"2024-13-01"',
        '(time)"25:00"',
        '(ipv4)"::1"',
        '(ipv6)"10.0.0.1"',
        '(uuid)"nope"',
        '(regex)"("',
        '(base64)"not base64!"',
        "(duration)5",
    ],
)
def test_known_type_that_does_not_fit_raises(text):
    ty = text[1 : text.index(")")]
    with pytest.raises(ValueError, match=re.escape(f"({ty})")):
        to_python(text)


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        ("hi", ("hi", None)),
        ("a b", ('"a b"', None)),
        # kdl-rs would print these bare, and they wouldn't parse back.
        ("+.7", ('"+.7"', None)),
        ("-.7", ('"-.7"', None)),
        ("+.5e3", ('"+.5e3"', None)),
        ('+."\\', ('"+.\\"\\\\"', None)),
        ("+.x", ("+.x", None)),
        (5, ("5", None)),
        (1.5, ("1.5", None)),
        (1e20, ("1e20", None)),
        (True, ("#true", None)),
        (False, ("#false", None)),
        (None, ("#null", None)),
        (math.inf, ("#inf", None)),
        (-math.inf, ("#-inf", None)),
        (math.nan, ("#nan", None)),
        (2**127 - 1, ("170141183460469231731687303715884105727", None)),
        (-(2**127), ("-170141183460469231731687303715884105728", None)),
    ],
)
def test_plain_from_python(value, expected):
    assert from_python(value) == expected


# The spec's "Disallowed Literal Code Points" (less surrogates, which a str can't hold here), plus the newlines a
# single-line string can't hold literally.
BANNED = [
    *range(0x09), *range(0x0E, 0x20), 0x7F, 0x200E, 0x200F, *range(0x202A, 0x202F), *range(0x2066, 0x206A), 0xFEFF,
]  # fmt: skip
NEWLINES = [0x0A, 0x0B, 0x0C, 0x0D, 0x85, 0x2028, 0x2029]


@pytest.mark.parametrize("char", [chr(c) for c in BANNED + NEWLINES], ids=lambda c: f"U+{ord(c):04X}")
def test_code_points_kdl_bans_are_escaped(char):
    doc = rkdl.parse("n 1\n")
    node = doc["n"]
    node.args[0] = char
    node.props[char] = char
    node.name = char
    node.args.entry(0).type = char
    text = str(doc)
    assert char not in text.removesuffix("\n")
    again = rkdl.parse(text)[char]
    assert again.args.entry(0).type == char
    assert again.args[0] == char
    assert again.props[char] == char


def test_escapes_use_short_forms_where_kdl_has_them():
    assert from_python("\b\f\n\r\t\x00 ") == ('"\\b\\f\\n\\r\\t\\u{0}\\u{2028}"', None)


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (datetime.datetime(2024, 5, 6, 7, 8, 9), ('"2024-05-06T07:08:09"', "date-time")),
        (datetime.date(2024, 5, 6), ('"2024-05-06"', "date")),
        (datetime.time(7, 8, 9), ('"07:08:09"', "time")),
        (ipaddress.IPv4Address("10.0.0.1"), ('"10.0.0.1"', "ipv4")),
        (ipaddress.IPv6Address("::1"), ("::1", "ipv6")),
        (uuid.UUID(int=1), ('"00000000-0000-0000-0000-000000000001"', "uuid")),
        (re.compile(r"\d+"), (r'"\\d+"', "regex")),
        (b"hi", ('"aGk="', "base64")),
        (decimal.Decimal("3.14159265358979323846"), ('"3.14159265358979323846"', "decimal")),
    ],
)
def test_inferred_from_python(value, expected):
    assert from_python(value) == expected


@pytest.mark.parametrize(
    ("value", "current", "expected"),
    [
        (255, "u8", ("255", "u8")),
        (-1, "i8", ("-1", "i8")),
        (1, "i32", ("1", "i32")),
        (2**64 - 1, "usize", ("18446744073709551615", "usize")),
        (1.5, "f64", ("1.5", "f64")),
        (2, "f64", ("2.0", "f64")),
        (1.5, "f32", ("1.5", "f32")),
        (decimal.Decimal("0.5"), "decimal64", ("0.5", "decimal64")),
        (decimal.Decimal("0.5"), "decimal", ('"0.5"', "decimal")),
        (datetime.date(2024, 5, 6), "date", ('"2024-05-06"', "date")),
        ("P1D", "duration", ("P1D", "duration")),
        ("x", "mine", ("x", "mine")),
        (5, "mine", ("5", "mine")),
        (None, "mine", ("#null", "mine")),
    ],
)
def test_current_type_kept_when_value_fits(value, current, expected):
    assert from_python(value, current) == expected


@pytest.mark.parametrize(
    ("value", "current", "expected"),
    [
        (256, "u8", ("256", None)),
        (-1, "u32", ("-1", None)),
        # bool is checked before int.
        (True, "u8", ("#true", None)),
        (True, "i32", ("#true", None)),
        (1.5, "i32", ("1.5", None)),
        (True, "f64", ("#true", None)),
        (1e39, "f32", ("1e39", None)),
        ("2024-05-06", "date", ('"2024-05-06"', None)),
        (datetime.datetime(2024, 5, 6), "date", ('"2024-05-06T00:00:00"', "date-time")),
        (datetime.date(2024, 5, 6), "date-time", ('"2024-05-06"', "date")),
        (5, "duration", ("5", None)),
        (re.compile(rb"x"), "regex", None),
        (datetime.date(2024, 5, 6), "mine", ('"2024-05-06"', "date")),
    ],
)
def test_current_type_replaced_when_value_does_not_fit(value, current, expected):
    if expected is None:
        with pytest.raises(TypeError):
            from_python(value, current)
    else:
        assert from_python(value, current) == expected


def test_int_outside_i128_overflows():
    with pytest.raises(OverflowError, match=re.escape("int doesn't fit in a KDL integer (i128)")):
        from_python(2**127)
    with pytest.raises(OverflowError, match=re.escape("(i128)")):
        from_python(-(2**127) - 1)
    with pytest.raises(OverflowError, match=re.escape("(i128)")):
        from_python(2**127, "u128")


@pytest.mark.xfail(strict=True, reason="kdl-rs doesn't parse the i128::MIN literal, which rkdl writes for -(2**127)")
def test_i128_min_round_trips():
    doc = rkdl.parse("n 1\n")
    doc["n"].args[0] = -(2**127)
    assert rkdl.parse(str(doc))["n"].args[0] == -(2**127)


def test_lone_surrogate_raises_value_error():
    with pytest.raises(ValueError):
        from_python("\ud800")
    with pytest.raises(ValueError):
        from_python("\ud800", "email")


def test_unsupported_class_raises_type_error():
    with pytest.raises(TypeError, match="object"):
        from_python(object())
    with pytest.raises(TypeError):
        from_python([1, 2])


class Celsius:
    def __init__(self, degrees):
        self.degrees = degrees

    def __eq__(self, other):
        return isinstance(other, Celsius) and other.degrees == self.degrees


CELSIUS = rkdl.Type("celsius", Celsius, decode=Celsius, encode=lambda c: c.degrees)


def test_user_type_round_trip():
    assert to_python("(celsius)21.5", [CELSIUS]) == Celsius(21.5)
    assert from_python(Celsius(21.5), None, [CELSIUS]) == ("21.5", "celsius")
    assert from_python(Celsius(3), "celsius", [CELSIUS]) == ("3", "celsius")


def test_user_type_unregistered_is_plain_or_type_error():
    assert to_python("(celsius)21.5") == 21.5
    with pytest.raises(TypeError):
        from_python(Celsius(1))


def test_user_type_overrides_builtin():
    upper = rkdl.Type("date", str, decode=str.upper, encode=str.lower)
    assert to_python('(date)"tuesday"', [upper]) == "TUESDAY"
    # Reading no longer validates the old built-in's format.
    assert to_python('(date)"whenever"', [upper]) == "WHENEVER"
    assert from_python("TODAY", "date", [upper]) == ("today", "date")
    # A date is no longer inferred as `date`, since that name now means something else.
    with pytest.raises(TypeError):
        from_python(datetime.date(2024, 5, 6), None, [upper])
    assert from_python(datetime.datetime(2024, 5, 6), None, [upper]) == ('"2024-05-06T00:00:00"', "date-time")


def test_user_types_inferred_before_builtins():
    stamp = rkdl.Type("stamp", datetime.date, decode=datetime.date.fromisoformat, encode=lambda d: d.isoformat())
    assert from_python(datetime.date(2024, 5, 6), None, [stamp]) == ('"2024-05-06"', "stamp")
    # datetime is a date subclass, so the user type claims it too.
    assert from_python(datetime.datetime(2024, 5, 6), None, [stamp]) == ('"2024-05-06T00:00:00"', "stamp")


def test_user_types_inferred_in_order():
    first = rkdl.Type("first", int, decode=int, encode=int)
    second = rkdl.Type("second", int, decode=int, encode=int)

    class Flag(enum.IntEnum):
        ON = 1

    assert from_python(Flag.ON, None, [first, second]) == ("1", "first")
    assert from_python(Flag.ON, None, [second, first]) == ("1", "second")


def test_plain_values_never_inferred_as_user_types():
    count = rkdl.Type("count", int, decode=int, encode=int)
    assert from_python(5, None, [count]) == ("5", None)
    assert from_python(5, "count", [count]) == ("5", "count")


def test_plain_subclass_without_user_type_stays_plain():
    class Color(enum.IntEnum):
        RED = 1

    assert from_python(Color.RED) == ("1", None)


def test_user_encode_must_return_plain():
    bad = rkdl.Type("bad", Celsius, decode=Celsius, encode=lambda c: c)
    with pytest.raises(TypeError, match="bad"):
        from_python(Celsius(1), None, [bad])


def test_user_decode_errors_propagate():
    def decode(value):
        raise RuntimeError("nope")

    broken = rkdl.Type("broken", Celsius, decode=decode, encode=lambda c: c.degrees)
    with pytest.raises(RuntimeError, match="nope"):
        to_python("(broken)1", [broken])


def test_type_requires_callables():
    with pytest.raises(TypeError):
        rkdl.Type("x", int, decode=1, encode=int)


def test_type_attributes():
    assert CELSIUS.name == "celsius"
    assert CELSIUS.cls is Celsius


def test_parser_accepts_types():
    assert rkdl.Parser(types=[CELSIUS]).parse("a (celsius)1")["a"].args[0] == Celsius(1)


def test_types_come_from_the_document_a_node_is_in():
    typed = rkdl.Parser(types=[CELSIUS]).parse("t (celsius)21\n")
    node = typed.children.pop(0)
    # A detached node keeps the types of the document it left.
    assert node.args[0] == Celsius(21)
    plain = rkdl.parse("")
    plain.children.append(node)
    assert type(node.args[0]) is int
    assert node.args[0] == 21
    typed.children.append(plain.children.pop(0))
    assert node.args[0] == Celsius(21)


# Through a document


def test_custom_type_reads_through_a_document():
    doc = rkdl.Parser(types=[CELSIUS]).parse("t (celsius)21.5 low=(celsius)-3 plain=1\n")
    t = doc["t"]
    assert t.args[0] == Celsius(21.5)
    assert t.args.entry(0).value == Celsius(21.5)
    assert t.props["low"] == Celsius(-3)
    assert t.props["plain"] == 1


def test_custom_type_writes_through_a_document():
    doc = rkdl.Parser(types=[CELSIUS]).parse("t 1 k=2\n")
    t = doc["t"]
    t.args.append(Celsius(3))
    t.props["k"] = Celsius(4)
    t.args.entry(0).value = Celsius(5)
    assert str(doc) == "t (celsius)5 (celsius)3 k=(celsius)4\n"
    assert list(t.args) == [Celsius(5), Celsius(3)]
    assert t.props == {"k": Celsius(4)}


def test_callbacks_can_read_their_own_document():
    # The document is unlocked while a callback runs, so one that reads the document doesn't deadlock. The calls run
    # on a thread so a deadlock fails the test instead of hanging it.
    docs = []

    def decode(degrees):
        return Celsius(degrees + docs[0]["offset"].args[0])

    def encode(c):
        return c.degrees - docs[0]["offset"].args[0]

    offset = rkdl.Type("offset", Celsius, decode=decode, encode=encode)
    doc = rkdl.Parser(types=[offset]).parse("offset 10\nt (offset)1\n")
    docs.append(doc)
    t = doc["t"]
    assert run_threads(lambda: t.args[0]) == [Celsius(11)]
    run_threads(lambda: t.args.append(Celsius(15)))
    assert str(doc) == "offset 10\nt (offset)1 (offset)5\n"


VALUE_WRITES = {
    "args.append": lambda n, v: n.args.append(v),
    "args.insert": lambda n, v: n.args.insert(0, v),
    # A good value first: nothing is written unless every value converts.
    "args.extend": lambda n, v: n.args.extend([5, v]),
    "args +=": lambda n, v: n.args.__iadd__([5, v]),
    "args[i] =": lambda n, v: n.args.__setitem__(0, v),
    "props[new] =": lambda n, v: n.props.__setitem__("new", v),
    "props[old] =": lambda n, v: n.props.__setitem__("k", v),
    "entry.value =": lambda n, v: setattr(n.args.entry(0), "value", v),
    "Node(arg)": lambda n, v: n.children.append(rkdl.Node("x", v)),
    "Node(prop)": lambda n, v: n.children.append(rkdl.Node("x", p=v)),
}

NAME_WRITES = {
    "node.name =": lambda n, v: setattr(n, "name", v),
    "node.type =": lambda n, v: setattr(n, "type", v),
    "entry.type =": lambda n, v: setattr(n.args.entry(0), "type", v),
    "Node(name)": lambda n, v: n.children.append(rkdl.Node(v)),
    "Node(type=)": lambda n, v: n.children.append(rkdl.Node("x", type=v)),
}

TOO_BIG = [2**127, -(2**127) - 1]
LONE_SURROGATE = "a\ud800b"


class Unsupported:
    pass


@pytest.mark.parametrize("write", VALUE_WRITES)
@pytest.mark.parametrize(
    ("value", "error"),
    [*[(v, OverflowError) for v in TOO_BIG], (LONE_SURROGATE, ValueError), (Unsupported(), TypeError)],
)
def test_bad_value_leaves_the_document_unchanged(write, value, error):
    doc = rkdl.parse("n 1 k=2 {\n    c\n}\n")
    before = str(doc)
    with pytest.raises(error):
        VALUE_WRITES[write](doc["n"], value)
    assert str(doc) == before


# Names are strings, so an int of any size is the wrong type.
@pytest.mark.parametrize("write", NAME_WRITES)
@pytest.mark.parametrize(
    ("value", "error"),
    [*[(v, TypeError) for v in TOO_BIG], (LONE_SURROGATE, ValueError), (Unsupported(), TypeError)],
)
def test_bad_name_leaves_the_document_unchanged(write, value, error):
    doc = rkdl.parse("n 1 k=2 {\n    c\n}\n")
    before = str(doc)
    with pytest.raises(error):
        NAME_WRITES[write](doc["n"], value)
    assert str(doc) == before


PLAIN_STRING_TYPES = [name for name, plain in _debug_reserved_types() if plain]


@pytest.mark.parametrize("ty", PLAIN_STRING_TYPES)
def test_plain_string_reserved_type(ty):
    doc = rkdl.parse(f'n ({ty})"x" k=({ty})"y"\n')
    n = doc["n"]
    assert type(n.args[0]) is str
    assert n.args[0] == "x"
    assert n.props["k"] == "y"
    # A string still fits, so the annotation stays. An integer doesn't, so it goes.
    n.args[0] = "z"
    n.props["k"] = 5
    assert str(doc) == f"n ({ty})z k=5\n"
    with pytest.raises(ValueError, match=re.escape(f"({ty})")):
        _ = rkdl.parse(f"n ({ty})5\n")["n"].args[0]


class Idx:
    """An index through `__index__`, the way numpy integers are."""

    def __init__(self, i):
        self.i = i

    def __index__(self):
        return self.i


@pytest.mark.parametrize("huge", [2**70, -(2**70), Idx(2**70)])
def test_indexes_past_isize_are_out_of_range(huge):
    # Any int works as an index, like a list's: past isize it's out of range, not an OverflowError.
    doc = rkdl.parse("n 1 2 { c }\n")
    n = doc["n"]
    for view in (n.args, n.entries, n.children, doc.children):
        with pytest.raises(IndexError):
            _ = view[huge]
    for view in (n.args, n.children):
        with pytest.raises(IndexError):
            view.pop(huge)
        with pytest.raises(IndexError):
            del view[huge]
    with pytest.raises(IndexError):
        n.args[huge] = 3
    with pytest.raises(IndexError):
        n.children[huge] = rkdl.Node("d")
    with pytest.raises(IndexError):
        n.args.entry(huge)
    assert str(doc) == "n 1 2 { c }\n"


def test_insert_past_isize_clamps():
    doc = rkdl.parse("n 1 2 { c }\n")
    n = doc["n"]
    n.args.insert(2**70, 3)
    n.args.insert(-(2**70), 0)
    n.children.insert(Idx(2**70), rkdl.Node("d"))
    n.children.insert(-(2**70), rkdl.Node("b"))
    assert list(n.args) == [0, 1, 2, 3]
    assert [c.name for c in n.children] == ["b", "c", "d"]
