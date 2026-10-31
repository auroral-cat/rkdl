import datetime
import decimal
import enum
import ipaddress
import json
import math
import pickle
import re
import subprocess
import sys
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
    ('(date-time)"2024-05-06T07:08:09+00:00"', datetime.datetime(2024, 5, 6, 7, 8, 9, tzinfo=datetime.UTC)),
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
    with pytest.raises(ValueError, match="KDL keeps only the pattern") as info:
        doc["n"].args.append(pattern)
    assert "inline" not in str(info.value)
    assert info.value.__notes__ == ["Put the flags inline instead, like (?i)"]
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
    assert from_python("\b\f\n\r\t\x00\u2028") == ('"\\b\\f\\n\\r\\t\\u{0}\\u{2028}"', None)


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
        # A whole float converts, as an int does over a float annotation.
        (3.0, "u8", ("3", "u8")),
        (-0.0, "i8", ("0", "i8")),
        (1.5, "f64", ("1.5", "f64")),
        (2, "f64", ("2.0", "f64")),
        (1.5, "f32", ("1.5", "f32")),
        (decimal.Decimal("0.5"), "decimal64", ("0.5", "decimal64")),
        (decimal.Decimal("0.5"), "decimal", ('"0.5"', "decimal")),
        (datetime.date(2024, 5, 6), "date", ('"2024-05-06"', "date")),
        ("2024-05-06", "date", ('"2024-05-06"', "date")),
        ("P1D", "duration", ("P1D", "duration")),
        ("x", "mine", ("x", "mine")),
        (5, "mine", ("5", "mine")),
        (None, "mine", ("#null", "mine")),
    ],
)
def test_current_type_kept_when_value_fits(value, current, expected):
    assert from_python(value, current) == expected


@pytest.mark.parametrize(
    ("value", "current", "message"),
    [
        # bool is checked before int.
        (True, "u8", "True doesn't fit (u8): expected an integer"),
        (True, "i32", "True doesn't fit (i32): expected an integer"),
        (True, "f64", "True doesn't fit (f64): expected a number"),
        ("7", "u8", "'7' doesn't fit (u8): expected an integer"),
        (None, "u8", "None doesn't fit (u8): expected an integer"),
        ("May 6th", "date", "'May 6th' doesn't fit (date): expected an ISO 8601 date, like 2026-01-31"),
        (datetime.datetime(2024, 5, 6), "date", "doesn't fit (date): would lose its time"),
        (datetime.date(2024, 5, 6), "date-time", "doesn't fit (date-time): expected a date-time"),
        (5, "duration", "5 doesn't fit (duration): expected a string"),
        (b"hi", "uuid", "b'hi' doesn't fit (uuid): expected a UUID"),
        (decimal.Decimal("1.5"), "u8", "Decimal('1.5') doesn't fit (u8): expected an integer"),
    ],
)
def test_a_value_that_does_not_fit_its_annotation_raises(value, current, message):
    with pytest.raises(ValueError, match=re.escape(message)):
        from_python(value, current)


@pytest.mark.parametrize(
    ("value", "text"),
    [
        (datetime.date(2024, 5, 6), '"2024-05-06"'),
        (decimal.Decimal("1.50"), '"1.50"'),
        (uuid.UUID(int=1), '"00000000-0000-0000-0000-000000000001"'),
        (b"hi", '"aGk="'),
    ],
)
def test_an_unknown_annotation_stays_over_a_non_plain_value(value, text):
    # A label rkdl doesn't know isn't checked, so it stays, and the value is written as its KDL text.
    assert from_python(value, "mine") == (text, "mine")


@pytest.mark.parametrize(
    ("value", "current", "message"),
    [
        (256, "u8", "256 doesn't fit (u8): out of range 0..=255"),
        (-1, "u32", "-1 doesn't fit (u32): out of range 0..=4294967295"),
        (128, "i8", "128 doesn't fit (i8): out of range -128..=127"),
        (1e39, "f32", "1e+39 doesn't fit (f32): out of range ±3.4028234663852886e+38"),
        (2**200, "f32", "doesn't fit (f32): out of range ±3.4028234663852886e+38"),
        (2**1024, "f64", "doesn't fit (f64): out of range ±1.7976931348623157e+308"),
        (1.5, "i32", "1.5 doesn't fit (i32): expected an integer"),
        (float("nan"), "u8", "nan doesn't fit (u8): expected an integer"),
        (float("inf"), "u8", "inf doesn't fit (u8): expected an integer"),
        (300.0, "u8", "300.0 doesn't fit (u8): out of range 0..=255"),
        (1e40, "i128", "1e+40 doesn't fit (i128): out of range"),
        (-1e40, "i128", "-1e+40 doesn't fit (i128): out of range"),
    ],
)
def test_a_number_out_of_its_types_range_raises(value, current, message):
    with pytest.raises(ValueError, match=re.escape(message)):
        from_python(value, current)


def test_int_outside_i128_overflows():
    with pytest.raises(OverflowError, match=re.escape("int doesn't fit in a KDL integer (i128)")):
        from_python(2**127)
    with pytest.raises(OverflowError, match=re.escape("(i128)")):
        from_python(-(2**127) - 1)
    # Under an integer annotation it's out of that type's range instead.
    with pytest.raises(ValueError, match=re.escape("doesn't fit (u128): out of range 0..=")):
        from_python(2**127, "u128")


@pytest.mark.xfail(strict=True, reason="kdl-rs doesn't parse the i128::MIN literal, which rkdl writes for -(2**127)")
def test_i128_min_round_trips():
    doc = rkdl.parse("n 1\n")
    doc["n"].args[0] = -(2**127)
    assert rkdl.parse(str(doc))["n"].args[0] == -(2**127)


class Rotating(int):
    """An 8-bit register, where >> rotates."""

    def __rshift__(self, n):
        n %= 8
        return Rotating((int(self) >> n | int(self) << (8 - n)) & 0xFF)


class Step(int):
    """A numbered pipeline step, where >> chains the next step."""

    def __rshift__(self, then):
        if not callable(then):
            raise TypeError("a step is followed by a callable")
        return then


class Renumbered(int):
    """An int whose conversions all give another number."""

    def __index__(self):
        return 99

    def __int__(self):
        return 99


@pytest.mark.parametrize(
    ("value", "base"),
    [(Rotating(0xA5), 0xA5), (Step(7), 7), (Renumbered(7), 7)],
    ids=["Rotating", "Step", "Renumbered"],
)
def test_an_int_subclass_writes_its_value_whatever_its_shift_does(value, base):
    # An int is written as its value; an operator or conversion the class gives its own meaning has nothing to do
    # with that.
    doc = rkdl.parse("n 1 (u8)1\n")
    doc["n"].args[0] = value
    assert str(doc) == f"n {base} (u8)1\n"
    doc["n"].args[1] = value
    assert str(doc) == f"n {base} (u8){base}\n"


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
    with pytest.raises(TypeError) as info:
        from_python(Celsius(1), None, [bad])
    assert str(info.value) == "encode for type 'bad' returned Celsius, not str, int, float, bool or None"


def test_user_encode_returning_an_int_outside_i128_overflows():
    # What encode returns is written as a plain value would be, so the same limit applies.
    too_fine = rkdl.Type("celsius", Celsius, decode=Celsius, encode=lambda c: c.degrees * 10**40)
    with pytest.raises(OverflowError, match=re.escape("int doesn't fit in a KDL integer (i128)")):
        from_python(Celsius(1), None, [too_fine])


def test_user_decode_errors_are_the_cause():
    def decode(value):
        raise RuntimeError("nope")

    broken = rkdl.Type("broken", Celsius, decode=decode, encode=lambda c: c.degrees)
    with pytest.raises(
        ValueError, match=r"^1 doesn't fit \(broken\): its decode raised RuntimeError\('nope'\)$"
    ) as info:
        to_python("(broken)1", [broken])
    assert isinstance(info.value.__cause__, RuntimeError)


def test_type_requires_callables():
    for decode, encode in [(1, int), (int, 1)]:
        with pytest.raises(TypeError, match="^Type decode and encode must be callable$"):
            rkdl.Type("x", int, decode=decode, encode=encode)


def test_type_attributes():
    assert CELSIUS.name == "celsius"
    assert CELSIUS.cls is Celsius
    assert CELSIUS.decode is Celsius
    assert CELSIUS.encode(Celsius(3)) == 3


def test_type_is_immutable():
    ty = rkdl.Type("x", int, decode=int, encode=int)
    for attr in ("name", "cls", "decode", "encode", "other"):
        with pytest.raises(AttributeError):
            setattr(ty, attr, str)
    # Like int's, a second __init__ has nothing to change.
    ty.__init__("y", str, decode=str, encode=str)
    assert (ty.name, ty.cls, ty.decode) == ("x", int, int)


# Values valid KDL holds that Python's types refuse.


@pytest.mark.parametrize(
    ("text", "shown", "cls"),
    [
        ('(regex)#"\\p{L}+"#', '(regex)#"\\p{L}+"#', "str"),
        ('(base64)"aGk"', '(base64)"aGk"', "str"),
        ('(date)"2020-1-1"', '(date)"2020-1-1"', "str"),
        ('(ipv4)"001.2.3.4"', '(ipv4)"001.2.3.4"', "str"),
        ("(u8)0x1_00", "(u8)0x1_00", "int"),
        ("(date)1.5", "(date)1.5", "float"),
    ],
)
def test_a_value_python_cant_read_says_how_to_read_it(text, shown, cls):
    doc = rkdl.parse(f"n {text}")
    ty = text[1 : text.index(")")]
    with pytest.raises(ValueError) as info:
        list(doc["n"].args)
    # The message is for whoever wrote the file, and the advice is a note for the developer.
    value = shown[shown.index(")") + 1 :]
    assert str(info.value).startswith(f"1:3: n arg 0: {value} doesn't fit ({ty}): ")
    assert "Type" not in str(info.value)
    hint = f'A Type named "{ty}" overrides the built-in one: '
    hint += f'Parser(types=[Type("{ty}", {cls}, {cls}, {cls})]) reads it as a plain {cls}'
    assert info.value.__notes__ == [hint]


def test_the_suggested_type_reads_the_value():
    plain = rkdl.Type("regex", str, str, str)
    doc = rkdl.Parser(types=[plain]).parse('n (regex)#"\\p{L}+"#')
    assert doc["n"].args[0] == "\\p{L}+"


def test_a_value_that_cant_be_read_shows_as_kdl_in_reprs():
    doc = rkdl.parse('n 1 (regex)#"\\p{L}+"# k=(u8)300')
    n = doc["n"]
    assert repr(n.args) == 'Args([1, <kdl: (regex)#"\\p{L}+"#>])'
    assert repr(n.props) == "Props({'k': <kdl: (u8)300>})"
    assert repr(n.props.entry("k")) == "<Entry k=<kdl: (u8)300>>"


# decimal64 and decimal128 keep a Decimal's digits, which the f64 kdl-rs keeps can't.


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (decimal.Decimal("1.00000000000000000000001"), "(decimal64)1.00000000000000000000001"),
        (decimal.Decimal("0.10"), "(decimal64)0.10"),
        (decimal.Decimal("1E+5"), "(decimal64)1E+5"),
        (decimal.Decimal(100), "(decimal64)100"),
        (decimal.Decimal("-0"), "(decimal64)-0.0"),
        (decimal.Decimal("NaN"), "(decimal64)#nan"),
        (decimal.Decimal("-Infinity"), "(decimal64)#-inf"),
    ],
)
def test_decimal64_writes_a_decimals_own_digits(value, expected):
    doc = rkdl.parse("n (decimal64)1.0")
    n = doc["n"]
    n.args[0] = value
    assert str(doc) == f"n {expected}"
    for read in (n.args[0], rkdl.parse(str(doc))["n"].args[0]):
        assert read == value or (read.is_nan() and value.is_nan())
        assert str(read) == str(value) or value.is_zero()


class Price(decimal.Decimal):
    def __str__(self):
        return f"${super().__str__()}"


class Ninety(int):
    def __float__(self):
        return 99.0


class FloatPrice(decimal.Decimal):
    def __float__(self):
        return 99.0


class Stamp(datetime.date):
    def isoformat(self):
        return self.strftime("%d/%m/%Y")


class Moment(datetime.datetime):
    def isoformat(self, sep="T", timespec="auto"):
        return self.strftime("%d/%m/%Y %H:%M")


class Clock(datetime.time):
    def isoformat(self, timespec="auto"):
        return self.strftime("%I:%M %p")


class Shout(float):
    def __float__(self):
        return 99.0

    def __repr__(self):
        return "99.0"


class Host(ipaddress.IPv4Address):
    def __str__(self):
        return "localhost"


class Host6(ipaddress.IPv6Address):
    def __str__(self):
        return "localhost"


class Short(uuid.UUID):
    def __str__(self):
        return self.hex[:8]


class Blob(bytes):
    # Python 3.12+ lets a class give its own buffer.
    def __buffer__(self, flags):
        return memoryview(b"other")


@pytest.mark.parametrize(
    ("old", "value", "base"),
    [
        pytest.param("(f64)1.0", Ninety(1), 1, id="int __float__"),
        pytest.param("(decimal64)1.0", FloatPrice("1.50"), decimal.Decimal("1.50"), id="Decimal __float__"),
        pytest.param("(decimal64)1.0", Price("1.50"), decimal.Decimal("1.50"), id="Decimal __str__ over decimal64"),
        pytest.param('(decimal)"1.0"', Price("1.50"), decimal.Decimal("1.50"), id="Decimal __str__ over decimal"),
        pytest.param("1.0", Price("1.50"), decimal.Decimal("1.50"), id="Decimal __str__"),
        pytest.param("(f64)1.0", Shout(1.5), 1.5, id="float __float__"),
        pytest.param("1.0", Shout(1.5), 1.5, id="float __repr__"),
        pytest.param('(date)"2026-01-01"', Stamp(2026, 10, 31), datetime.date(2026, 10, 31), id="date isoformat"),
        pytest.param("1", Stamp(2026, 10, 31), datetime.date(2026, 10, 31), id="date isoformat inferred"),
        pytest.param(
            '(date-time)"2026-01-01T00:00:00"',
            Moment(2026, 10, 31, 9, 30),
            datetime.datetime(2026, 10, 31, 9, 30),
            id="datetime isoformat",
        ),
        pytest.param("1", Clock(9, 30), datetime.time(9, 30), id="time isoformat"),
        pytest.param("1", Host("10.0.0.1"), ipaddress.IPv4Address("10.0.0.1"), id="IPv4Address __str__"),
        pytest.param("1", Host6("fe80::1%eth0"), ipaddress.IPv6Address("fe80::1%eth0"), id="IPv6Address __str__"),
        pytest.param("1", Short(int=5), uuid.UUID(int=5), id="UUID __str__"),
        pytest.param("1", Blob(b"ab"), b"ab", id="bytes __buffer__"),
    ],
)
def test_a_subclass_is_written_as_its_base_types_value(old, value, base):
    # A subclass of a built-in type is written by its base type's value; methods it overrides don't change that.
    def written(value):
        doc = rkdl.parse(f"n {old}\n")
        doc["n"].args[0] = value
        return str(doc)

    assert written(value) == written(base)
    assert rkdl.parse(written(value))["n"].args[0] == base


def test_a_str_subclass_fits_where_its_value_does():
    class Text(str):
        def __str__(self):
            return "whenever"

    doc = rkdl.parse('n (date)"2026-01-01"\n')
    doc["n"].args[0] = Text("2026-10-31")
    assert str(doc) == 'n (date)"2026-10-31"\n'
    assert doc["n"].args[0] == datetime.date(2026, 10, 31)


# The thread's decimal context belongs to the program; a file reads the same whatever it says.


@pytest.mark.parametrize(
    ("text", "why"),
    [
        ('(decimal)"weekly"', "expected a decimal number, like 1.25"),
        ("(decimal64)1e-99999999999999999999", "exponent past what Python's decimal takes"),
        ("(decimal128)1e-99999999999999999999", "exponent past what Python's decimal takes"),
    ],
)
def test_a_decimal_misfit_is_refused_whatever_the_threads_context(text, why):
    with decimal.localcontext() as context:
        context.traps[decimal.InvalidOperation] = False
        n = rkdl.parse(f"n {text}\n")["n"]
        err = read_error(lambda n: n.args[0], n)
    assert type(err) is rkdl.ReadError
    assert reason(err) == why


@pytest.mark.parametrize(
    ("text", "value"),
    [
        ('(decimal)"3.14159265358979323846264338327950288"', decimal.Decimal("3.14159265358979323846264338327950288")),
        ("(decimal64)123456.789", decimal.Decimal("123456.789")),
        ("(decimal128)1e-400", decimal.Decimal("1e-400")),
        ("(decimal64)0.0e99999999999999999999", decimal.Decimal(0)),
    ],
)
def test_a_decimal_reads_the_same_whatever_the_threads_context(text, value):
    # A tiny precision and exponent range, with every trap set.
    traps = [
        decimal.Clamped,
        decimal.DivisionByZero,
        decimal.FloatOperation,
        decimal.Inexact,
        decimal.InvalidOperation,
        decimal.Overflow,
        decimal.Rounded,
        decimal.Subnormal,
        decimal.Underflow,
    ]
    with decimal.localcontext(decimal.Context(prec=2, Emax=10, Emin=-10, traps=traps)):
        read = to_python(text)
    assert type(read) is decimal.Decimal
    assert str(read) == str(value)


def test_decimal64_reads_digits_from_the_text():
    digits = "3.14159265358979323846264338327950288"
    assert to_python(f"(decimal128){digits}") == decimal.Decimal(digits)
    assert to_python("(decimal64)1_000.000_1") == decimal.Decimal("1000.0001")
    assert to_python("(decimal64)0x10") == decimal.Decimal(16)


# A VERBOSE comment can hold what doesn't compile without the flag, and the refusal is the same either way.


@pytest.mark.parametrize(
    ("pattern", "flags", "named"),
    [
        (r"\d+  # digits (optional", re.VERBOSE, "re.VERBOSE"),
        (r"[a-z ]  # chars [", re.VERBOSE, "re.VERBOSE"),
        (r"\d+  # digits", re.VERBOSE, "re.VERBOSE"),
        (r"\d+  # digits (optional", re.VERBOSE | re.IGNORECASE, "re.IGNORECASE|re.VERBOSE"),
    ],
)
def test_a_verbose_regex_is_refused_whatever_its_comment(pattern, flags, named):
    message = f"can't write a regex compiled with {named}: KDL keeps only the pattern"
    compiled = re.compile(pattern, flags)
    for ty in (None, "regex"):
        with pytest.raises(ValueError) as info:
            from_python(compiled, ty)
        assert type(info.value) is ValueError
        assert str(info.value) == message
        assert info.value.__notes__ == ["Put the flags inline instead, like (?i)"]


def test_an_inline_verbose_flag_is_part_of_the_pattern():
    pattern = r"(?x)\d+  # digits (optional"
    doc = rkdl.parse('n (regex)"x"\n')
    doc["n"].args[0] = re.compile(pattern, re.VERBOSE)
    assert rkdl.parse(str(doc))["n"].args[0] == re.compile(pattern)


def test_only_a_regex_error_is_taken_for_a_verbose_comment(monkeypatch):
    compiled = re.compile(r"\d+  # digits (optional", re.VERBOSE)
    calls = []

    def interrupted_once(*args):
        calls.append(args)
        if len(calls) == 1:
            raise KeyboardInterrupt
        return compile_(*args)

    compile_ = re.compile
    monkeypatch.setattr(re, "compile", interrupted_once)
    with pytest.raises(KeyboardInterrupt):
        from_python(compiled)


# KDL has no signaling NaN, and writing #nan would change the value without a word.


@pytest.mark.parametrize("ty", ["decimal64", "decimal128"])
@pytest.mark.parametrize("snan", ["sNaN", "-sNaN"])
def test_a_signaling_nan_doesnt_fit_a_decimal_number(snan, ty):
    with pytest.raises(ValueError) as info:
        from_python(decimal.Decimal(snan), ty)
    assert str(info.value) == f"Decimal('{snan}') doesn't fit ({ty}): KDL has no signaling NaN"


# The reason the write side gives for a decimal64 or decimal128 past the f64 range; a read gives the same one.
DECIMAL_RANGE = "out of range ±1.7976931348623157e+308"


@pytest.mark.parametrize("ty", ["decimal64", "decimal128"])
@pytest.mark.parametrize(
    ("text", "signed"),
    [("0.0e99999999999999999999", False), ("-0.0e99999999999999999999", True), ("0e1000000000000000000", False)],
)
def test_a_decimal_zero_with_an_exponent_past_what_decimal_takes_reads_as_zero(text, signed, ty):
    read = rkdl.parse(f"n ({ty}){text}\n")["n"].args[0]
    assert read == decimal.Decimal(0)
    assert read.is_signed() == signed


@pytest.mark.parametrize("ty", ["decimal64", "decimal128"])
def test_a_decimal_exponent_past_what_decimal_takes_is_refused_for_it(ty):
    # Tiny, not past the f64 range.
    n = rkdl.parse(f"n ({ty})1e-99999999999999999999\n")["n"]
    err = read_error(lambda n: n.args[0], n)
    assert type(err) is rkdl.ReadError
    assert reason(err) == "exponent past what Python's decimal takes"


def test_only_decimals_own_refusal_is_taken_for_the_exponent(monkeypatch):
    def interrupted(*args):
        raise KeyboardInterrupt

    monkeypatch.setattr(decimal, "Decimal", interrupted)
    n = rkdl.parse("n (decimal64)1e-99999999999999999999\n")["n"]
    with pytest.raises(KeyboardInterrupt):
        n.args[0]


def test_a_tiny_decimal_reads():
    assert to_python("(decimal64)1e-400") == decimal.Decimal("1e-400")


@pytest.mark.parametrize("ty", ["decimal64", "decimal128"])
@pytest.mark.parametrize("text", ["1e99999999999999999999", "-1e99999999999999999999"])
def test_a_decimal_literal_past_both_limits_gets_the_range_reason(text, ty):
    n = rkdl.parse(f"n ({ty}){text}\n")["n"]
    assert reason(read_error(lambda n: n.args[0], n)) == DECIMAL_RANGE


def test_decimal64_out_of_the_f64_range_doesnt_fit():
    # Decimal("1e400") is no f64.
    message = "Decimal('1E+400') doesn't fit (decimal64): out of range ±1.7976931348623157e+308"
    with pytest.raises(ValueError, match=re.escape(message)):
        from_python(decimal.Decimal("1e400"), "decimal64")


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
    # A string fits. An integer doesn't.
    n.args[0] = "z"
    with pytest.raises(ValueError, match=re.escape(f"5 doesn't fit ({ty}): expected a string")):
        n.props["k"] = 5
    assert str(doc) == f'n ({ty})"z" k=({ty})"y"\n'
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


# Read errors: always ValueError, naming where the value is, in the words a write uses.


def want_a_number(value):
    if not isinstance(value, (int, float)):
        raise TypeError("want a number")
    return Celsius(value)


STRICT_CELSIUS = rkdl.Type("celsius", Celsius, decode=want_a_number, encode=lambda c: c.degrees)

ARG_READS = {
    "args[i]": lambda n: n.args[1],
    "args[-1]": lambda n: n.args[-1],
    "args slice": lambda n: n.args[:],
    "list(args)": lambda n: list(n.args),
    "reversed(args)": lambda n: list(reversed(n.args)),
    "args ==": lambda n: n.args == [1, 2],
    "in args": lambda n: 5 in n.args,
    "args.index": lambda n: n.args.index(5),
    "args.count": lambda n: n.args.count(1),
    "args.pop": lambda n: n.args.pop(),
    "args.remove": lambda n: n.args.remove(5),
    "entry.value": lambda n: n.args.entry(1).value,
    "unpack entry": lambda n: tuple(n.args.entry(1)),
    "entries": lambda n: [e.value for e in n.entries],
}

PROP_READS = {
    "props[k]": lambda n: n.props["k"],
    "props.get": lambda n: n.props.get("k"),
    "props.items": lambda n: list(n.props.items()),
    "props.values": lambda n: list(n.props.values()),
    "dict(props)": lambda n: dict(n.props),
    "props.copy": lambda n: n.props.copy(),
    "props ==": lambda n: n.props == {"k": 1},
    "props |": lambda n: n.props | {},
    "props.pop": lambda n: n.props.pop("k"),
    "props.popitem": lambda n: n.props.popitem(),
    "props.setdefault": lambda n: n.props.setdefault("k", 1),
    "entry.value": lambda n: n.props.entry("k").value,
    "unpack entry": lambda n: tuple(n.props.entry("k")),
}

# The annotated value, the parser's types, and the message after "1:10: server arg 1: " or "1:8: server prop k: ".
BAD_VALUES = {
    "built-in": ("(u8)300", [], "300 doesn't fit (u8): out of range 0..=255"),
    "custom": (
        '(celsius)"hot"',
        [STRICT_CELSIUS],
        "\"hot\" doesn't fit (celsius): its decode raised TypeError('want a number')",
    ),
}


def read_error(read, node):
    with pytest.raises(ValueError) as info:
        read(node)
    return info.value


@pytest.mark.parametrize("kind", BAD_VALUES)
@pytest.mark.parametrize("route", ARG_READS)
def test_reading_an_arg_that_doesnt_fit(route, kind):
    bad, types, core = BAD_VALUES[kind]
    doc = rkdl.Parser(types=types).parse(f"server 1 {bad}\n")
    err = read_error(ARG_READS[route], doc["server"])
    assert type(err) is rkdl.ReadError
    assert (err.path, err.line, err.column, err.key) == (None, 1, 10, 1)
    assert str(err) == f"1:10: server arg 1: {core}"
    if kind == "custom":
        assert isinstance(err.__cause__, TypeError)
        assert not hasattr(err, "__notes__")
    else:
        assert err.__cause__ is None
        assert len(err.__notes__) == 1
    assert str(doc) == f"server 1 {bad}\n"


@pytest.mark.parametrize("kind", BAD_VALUES)
@pytest.mark.parametrize("route", PROP_READS)
def test_reading_a_prop_that_doesnt_fit(route, kind):
    bad, types, core = BAD_VALUES[kind]
    doc = rkdl.Parser(types=types).parse(f"server k={bad}\n")
    err = read_error(PROP_READS[route], doc["server"])
    assert type(err) is rkdl.ReadError
    assert (err.path, err.line, err.column, err.key) == (None, 1, 8, "k")
    assert str(err) == f"1:8: server prop k: {core}"
    assert isinstance(err.__cause__, TypeError) == (kind == "custom")
    assert str(doc) == f"server k={bad}\n"


def test_the_arg_index_counts_args_only():
    n = rkdl.parse("server a=1 7 b=2 (u8)300\n")["server"]
    assert str(read_error(lambda n: n.args[1], n)).startswith("1:18: server arg 1: ")
    assert str(read_error(lambda n: n.entries[3].value, n)).startswith("1:18: server arg 1: ")


def test_read_errors_spell_names_as_a_file_does():
    n = rkdl.parse('"my server" (u8)-1 "a key"=(u8)300\n')["my server"]
    assert str(read_error(lambda n: n.args[0], n)).startswith('1:13: "my server" arg 0: -1 doesn\'t fit')
    assert str(read_error(lambda n: n.props["a key"], n)).startswith('1:20: "my server" prop "a key": 300 doesn\'t fit')


def test_read_errors_show_the_value_as_the_file_spells_it():
    n = rkdl.parse('n (u8)0x1_00 (date)"2020-1-1"\n')["n"]
    assert str(read_error(lambda n: n.args[0], n)) == "1:3: n arg 0: 0x1_00 doesn't fit (u8): out of range 0..=255"
    assert str(read_error(lambda n: n.args[1], n)).startswith('1:14: n arg 1: "2020-1-1" doesn\'t fit (date): ')


def test_a_detached_node_still_names_itself_and_where_it_was(tmp_path):
    n = rkdl.parse("n (u8)300\n").children.pop()
    assert str(read_error(lambda n: n.args[0], n)).startswith("1:3: n arg 0: 300 doesn't fit")
    path = tmp_path / "config.kdl"
    path.write_text("a\nn (u8)300\n")
    n = rkdl.load(path).children.pop()
    assert str(read_error(lambda n: n.args[0], n)).startswith(f"{path}:2:3: n arg 0: 300 doesn't fit")


def test_a_decode_that_raises_value_error_is_wrapped_too():
    def decode(value):
        raise ValueError("too hot")

    hot = rkdl.Type("celsius", Celsius, decode=decode, encode=lambda c: c.degrees)
    n = rkdl.Parser(types=[hot]).parse("server (celsius)99\n")["server"]
    err = read_error(lambda n: n.args[0], n)
    assert str(err) == "1:8: server arg 0: 99 doesn't fit (celsius): its decode raised ValueError('too hot')"
    assert str(err.__cause__) == "too hot"


def test_a_decode_that_raises_a_base_exception_propagates():
    class Stop(BaseException):
        pass

    def decode(value):
        raise Stop

    stop = rkdl.Type("celsius", Celsius, decode=decode, encode=lambda c: c.degrees)
    doc = rkdl.Parser(types=[stop]).parse("t (celsius)1\n")
    with pytest.raises(Stop):
        doc["t"].args[0]
    # Writing a plain value over the annotation asks decode whether it fits, and that propagates too.
    with pytest.raises(Stop):
        doc["t"].args.append(rkdl.Value(2, type="celsius"))
    assert str(doc) == "t (celsius)1\n"


# A decode returns a Python value; an rkdl.Value is only for writing. Read back and written, it would replace the
# entry's annotation, or reach encode, which would be blamed for it (task 72).
def test_a_decode_that_returns_an_rkdl_value_is_refused():
    kelvin = rkdl.Type(
        "kelvin", Celsius, decode=lambda k: rkdl.Value(k - 273, type="celsius"), encode=lambda c: c.degrees + 273
    )
    doc = rkdl.Parser(types=[kelvin]).parse("oven (kelvin)300 27\n")
    err = read_error(lambda n: n.args[0], doc["oven"])
    assert isinstance(err, rkdl.ReadError)
    assert str(err) == "1:6: oven arg 0: 300 doesn't fit (kelvin): its decode returned an rkdl.Value"
    assert err.__cause__ is None
    # A plain value written over the annotation must be one decode accepts: the same reason.
    with pytest.raises(ValueError) as info:
        doc["oven"].args[0] = 27
    assert str(info.value) == "27 doesn't fit (kelvin): its decode returned an rkdl.Value"
    with pytest.raises(ValueError) as info:
        doc["oven"].args.entry(1).type = "kelvin"
    assert str(info.value) == "27 doesn't fit (kelvin): its decode returned an rkdl.Value"
    assert str(doc) == "oven (kelvin)300 27\n"
    # An instance of the Type's class goes through encode alone.
    doc["oven"].args[0] = Celsius(30)
    assert str(doc) == "oven (kelvin)303 27\n"


def test_a_recursion_error_from_decode_isnt_wrapped():
    def decode(value):
        raise RecursionError("deep")

    deep = rkdl.Type("celsius", Celsius, decode=decode, encode=lambda c: c.degrees)
    doc = rkdl.Parser(types=[deep]).parse("t (celsius)1\n")
    with pytest.raises(RecursionError, match="^deep$"):
        doc["t"].args[0]
    with pytest.raises(RecursionError, match="^deep$"):
        doc["t"].args[0] = 2
    # Showing the value, and checking whether a write would change it, read it too (task 75).
    with pytest.raises(RecursionError, match="^deep$"):
        repr(doc["t"].args)
    with pytest.raises(RecursionError, match="^deep$"):
        doc["t"].args[0] = Celsius(2)
    assert str(doc) == "t (celsius)1\n"


# Hooks that recurse through rkdl, each level reading or writing the same entry, stop at Python's recursion limit
# with its RecursionError (task 77), at the default limit. Wrapping each level's RecursionError in a ReadError
# doubled the message per level, until memory ran out (task 73); an unoptimized build overflowed the C stack first
# (task 76), which the dev profile's opt-level 1 fixes. A subprocess, with its memory capped, so a crash fails only
# this test.
ENDLESS_HOOKS = """
import sys
try:
    import resource
    resource.setrlimit(resource.RLIMIT_AS, (2_000_000_000, 2_000_000_000))
except ImportError:
    pass
import rkdl
class Celsius:
    def __init__(self, degrees):
        self.degrees = degrees
calls = 0
def count():
    global calls
    calls += 1
def read(value):
    count()
    return doc["n"].args[0]
def write_back(value):
    count()
    doc["n"].args[0] = value
    return Celsius(value)
def encode(c):
    count()
    doc["n"].args[0] = c
    return c.degrees
case = sys.argv[1]
if case == "read":
    types, value = [rkdl.Type("t", object, decode=read, encode=str)], None
elif case == "encode":
    types, value = [rkdl.Type("t", Celsius, decode=Celsius, encode=encode)], Celsius(2)
else:
    types, value = [rkdl.Type("t", Celsius, decode=write_back, encode=lambda c: c.degrees)], 2
doc = rkdl.Parser(types=types).parse("n (t)1\\n")
try:
    if value is None:
        doc["n"].args[0]
    else:
        doc["n"].args[0] = value
except RecursionError:
    print("RecursionError", calls)
"""


# The decode that reads, the encode that writes, and the decode a plain value written goes through: writes take the
# most C stack a level.
@pytest.mark.parametrize("case", ["read", "encode", "write-side decode"])
def test_hooks_that_recurse_without_end_raise_recursion_error_at_pythons_limit(case):
    command = [sys.executable, "-c", ENDLESS_HOOKS, case]
    result = subprocess.run(command, check=False, capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr[-2000:]
    name, calls = result.stdout.split()
    assert name == "RecursionError"
    # Python's default limit of 1000 frames stopped it, not a limit of rkdl's.
    assert int(calls) > 900


# A decode's exception is quoted by its message, at most 200 characters of it; the whole exception is the __cause__
# (task 75).
@pytest.mark.parametrize(
    ("raised", "reason"),
    [
        (ValueError("x" * 200), "ValueError('" + "x" * 200 + "')"),
        (ValueError("x" * 201), "ValueError('" + "x" * 200 + "…')"),
        (ValueError("é" * 201), "ValueError('" + "é" * 200 + "…')"),
        (ValueError(), "ValueError()"),
        (ValueError('say "hi"\nthere'), """ValueError('say "hi"\\nthere')"""),
    ],
)
def test_a_decodes_exception_is_quoted_by_its_message_cut_short(raised, reason):
    def decode(value):
        raise raised

    n = rkdl.Parser(types=[rkdl.Type("t", object, decode=decode, encode=str)]).parse("n (t)1\n")["n"]
    err = read_error(lambda n: n.args[0], n)
    assert str(err) == f"1:3: n arg 0: 1 doesn't fit (t): its decode raised {reason}"
    assert err.__cause__ is raised


# Decodes that read their own entry, where the deepest catches the RecursionError and raises a ValueError: every level
# above fails, each with a ReadError quoting the one below. Quoting each one's repr doubled the message per level, so
# memory ran out long before the top. Its message stays short; the chain is all there as __cause__s. A subprocess with
# its memory capped, so a crash fails only this test. At Python's default limit: unoptimized, the deepest wrapping,
# which runs kdl-rs's parser, overflowed the C stack (task 76).
FAILING_DECODE_CHAIN = """
import json
import sys
try:
    import resource
    resource.setrlimit(resource.RLIMIT_AS, (2_000_000_000, 2_000_000_000))
except ImportError:
    pass
import rkdl
def decode(value):
    try:
        return doc["n"].args[0]
    except RecursionError:
        raise ValueError("too deep")
doc = rkdl.Parser(types=[rkdl.Type("loop", object, decode=decode, encode=lambda v: v)]).parse("n (loop)1\\n")
try:
    doc["n"].args[0]
except rkdl.ReadError as err:
    chain = [err]
    while chain[-1].__cause__ is not None:
        chain.append(chain[-1].__cause__)
    shown = [[type(e).__name__, str(e)] for e in chain[:2] + chain[-2:]]
    print(json.dumps({"chain": shown, "depth": len(chain)}))
"""


def test_a_chain_of_failing_decodes_ends_in_a_short_message():
    command = [sys.executable, "-c", FAILING_DECODE_CHAIN]
    result = subprocess.run(command, check=False, capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr[-2000:]
    seen = json.loads(result.stdout)
    place = "1:3: n arg 0: 1 doesn't fit (loop): its decode raised "
    (top_type, top), (below_type, below), last_two = seen["chain"][0], seen["chain"][1], seen["chain"][2:]
    assert top_type == below_type == "ReadError"
    assert top == place + "ReadError(" + repr(below[:200] + "…") + ")"
    assert last_two == [["ReadError", place + "ValueError('too deep')"], ["ValueError", "too deep"]]
    # A ReadError per decode, as deep as Python's limit, and the ValueError.
    assert seen["depth"] > 900


# A decode that reprs its own node's args: catching the RecursionError there kept the recursion going until the
# process ran out of memory. A subprocess with its memory capped, as above.
ENDLESS_REPR = """
import sys
try:
    import resource
    resource.setrlimit(resource.RLIMIT_AS, (2_000_000_000, 2_000_000_000))
except ImportError:
    pass
import rkdl
def decode(value):
    return repr(doc["n"].args)
doc = rkdl.Parser(types=[rkdl.Type("loop", object, decode=decode, encode=lambda v: v)]).parse("n (loop)1 (loop)2\\n")
try:
    repr(doc["n"].args)
except RecursionError:
    print("RecursionError")
"""


def test_a_repr_that_recurses_through_decode_raises_recursion_error():
    command = [sys.executable, "-c", ENDLESS_REPR]
    result = subprocess.run(command, check=False, capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr[-2000:]
    assert result.stdout == "RecursionError\n"


def test_setting_a_type_the_value_doesnt_fit_says_so_without_a_place():
    doc = rkdl.Parser(types=[STRICT_CELSIUS]).parse('n 300 "hot"\n')
    with pytest.raises(ValueError) as info:
        doc["n"].args.entry(0).type = "u8"
    assert str(info.value) == "300 doesn't fit (u8): out of range 0..=255"
    assert len(info.value.__notes__) == 1
    with pytest.raises(ValueError, match=r'^"hot" doesn\'t fit \(celsius\): its decode raised') as info:
        doc["n"].args.entry(1).type = "celsius"
    assert isinstance(info.value.__cause__, TypeError)
    assert str(doc) == 'n 300 "hot"\n'


def test_a_read_error_uses_a_write_errors_words():
    n = rkdl.parse("server (u8)300 (u8)7\n")["server"]
    read = read_error(lambda n: n.args[0], n)
    write = read_error(lambda n: n.args.__setitem__(1, 300), n)
    assert str(read) == f"1:8: server arg 0: {write}"


# Where a read error is: the entry's position goes in front, as ParseError's first line does.


def test_a_read_error_starts_with_the_files_path_and_the_entrys_position(tmp_path):
    path = tmp_path / "config.kdl"
    path.write_text("limits {\n    inner retries=(u8)300\n}\n")
    inner = rkdl.load(path)["limits"]["inner"]
    err = read_error(lambda n: n.props["retries"], inner)
    assert str(err) == f"{path}:2:11: inner prop retries: 300 doesn't fit (u8): out of range 0..=255"


def test_a_read_error_without_a_path_starts_with_the_position():
    limits = rkdl.parse("limits {\n    inner (u8)300\n}\n")["limits"]
    err = read_error(lambda n: n.args[0], limits["inner"])
    assert str(err) == "2:11: inner arg 0: 300 doesn't fit (u8): out of range 0..=255"


def test_a_read_error_tells_duplicate_nodes_apart(tmp_path):
    path = tmp_path / "config.kdl"
    path.write_text("inner retries=(u8)3\ninner retries=(u8)300\n")
    first, second = rkdl.load(path).children
    assert first.props["retries"] == 3
    err = read_error(lambda n: n.props["retries"], second)
    assert str(err).startswith(f"{path}:2:7: inner prop retries: 300 doesn't fit")


def test_a_read_error_for_an_entry_without_a_position_starts_with_the_node():
    # A copy, and a node moved into another document, have no positions: they aren't where the file put them.
    as_int = rkdl.Type("u8", int, int, int)
    n = rkdl.Parser(types=[as_int]).parse("n (u8)300\n").children.pop()
    assert n.args[0] == 300
    other = rkdl.parse("")
    other.children.append(n)
    assert str(read_error(lambda n: n.args[0], n)) == "n arg 0: 300 doesn't fit (u8): out of range 0..=255"
    copied = rkdl.parse("n (u8)300\n")["n"].copy()
    assert str(read_error(lambda n: n.args[0], copied)).startswith("n arg 0: 300 doesn't fit")


def test_a_write_error_names_no_place_even_over_an_entry_with_a_position(tmp_path):
    # A write error is about the program's value, not the file.
    path = tmp_path / "config.kdl"
    path.write_text("n (u8)7\n")
    n = rkdl.load(path)["n"]
    assert n.args.entry(0).line == 1
    with pytest.raises(ValueError) as info:
        n.args[0] = 300
    assert str(info.value) == "300 doesn't fit (u8): out of range 0..=255"


# ReadError: a read misfit's place as attributes, for programs that build their own messages.


def test_a_read_error_is_a_value_error_with_the_place_as_attributes(tmp_path):
    path = tmp_path / "config.kdl"
    path.write_text("limits {\n    inner 1 (u8)300 retries=(u8)300\n}\n")
    inner = rkdl.load(path)["limits"]["inner"]
    arg = read_error(lambda n: n.args[1], inner)
    assert isinstance(arg, rkdl.ReadError)
    assert isinstance(arg, ValueError)
    assert (arg.path, arg.line, arg.column, arg.key) == (str(path), 2, 13, 1)
    prop = read_error(lambda n: n.props["retries"], inner)
    assert (prop.path, prop.line, prop.column, prop.key) == (str(path), 2, 21, "retries")
    assert str(prop) == f"{path}:2:21: inner prop retries: 300 doesn't fit (u8): out of range 0..=255"


def test_a_read_error_without_a_position_has_none_for_line_and_column():
    copied = rkdl.parse("n (u8)300\n")["n"].copy()
    arg = read_error(lambda n: n.args[0], copied)
    assert type(arg) is rkdl.ReadError
    assert (arg.path, arg.line, arg.column, arg.key) == (None, None, None, 0)
    doc = rkdl.Parser(types=[STRICT_CELSIUS]).parse('n k=(celsius)"hot"\n')
    copied = doc["n"].copy()
    prop = read_error(lambda n: n.props["k"], copied)
    assert (prop.path, prop.line, prop.column, prop.key) == (None, None, None, "k")
    assert str(prop).startswith('n prop k: "hot" doesn\'t fit (celsius)')
    assert isinstance(prop.__cause__, TypeError)


@pytest.mark.parametrize("kind", BAD_VALUES)
def test_a_read_error_pickles_with_its_place_and_notes(tmp_path, kind):
    bad, types, core = BAD_VALUES[kind]
    path = tmp_path / "config.kdl"
    path.write_text(f"server k={bad}\n")
    err = read_error(lambda n: n.props["k"], rkdl.Parser(types=types).load(path)["server"])
    again = pickle.loads(pickle.dumps(err))
    assert type(again) is rkdl.ReadError
    assert str(again) == str(err) == f"{path}:1:8: server prop k: {core}"
    assert (again.path, again.line, again.column, again.key) == (str(path), 1, 8, "k")
    assert getattr(again, "__notes__", None) == getattr(err, "__notes__", None)


def test_a_read_error_has_the_entry_as_written(tmp_path):
    path = tmp_path / "config.kdl"
    path.write_text("server 1 ( u8 )300 retries = (u8)0x1FF\n")
    server = rkdl.load(path)["server"]
    arg = read_error(lambda n: n.args[1], server)
    assert arg.text == "( u8 )300" == str(server.args.entry(1))
    prop = read_error(lambda n: n.props["retries"], server)
    assert prop.text == "retries = (u8)0x1FF" == str(server.props.entry("retries"))


def test_a_read_error_on_a_built_node_has_the_text_it_would_print():
    doc = rkdl.Parser(types=[STRICT_CELSIUS]).parse("")
    built = rkdl.Node("n", rkdl.Value("hot", type="celsius"), k=rkdl.Value("cold", type="celsius"))
    doc.children.append(built)
    assert read_error(lambda n: n.args[0], built).text == "(celsius)hot" == str(built.args.entry(0))
    assert read_error(lambda n: n.props["k"], built).text == "k=(celsius)cold" == str(built.props.entry("k"))


@pytest.mark.parametrize("kind", BAD_VALUES)
def test_a_read_errors_text_survives_pickling(kind):
    bad, types, _ = BAD_VALUES[kind]
    err = read_error(lambda n: n.props["k"], rkdl.Parser(types=types).parse(f"server k={bad}\n")["server"])
    assert pickle.loads(pickle.dumps(err)).text == err.text == f"k={bad}"


def test_writes_and_entry_type_raise_plain_value_errors():
    # Both are about what the program asked for, not the file.
    n = rkdl.parse("n (u8)7 300\n")["n"]
    with pytest.raises(ValueError) as info:
        n.args[0] = 300
    assert type(info.value) is ValueError
    with pytest.raises(ValueError) as info:
        n.args.entry(1).type = "u8"
    assert type(info.value) is ValueError
    with pytest.raises(ValueError) as info:
        n.args.append(rkdl.Value(300, type="u8"))
    assert type(info.value) is ValueError


# One reason per misfit, whether the value is read from a file, checked by `entry.type`, or written by the program.

MISFITS = {
    "word over u8": ("u8", '"weekly"', "weekly", "expected an integer"),
    "fraction over u8": ("u8", "3.5", 3.5, "expected an integer"),
    "bool over u8": ("u8", "#true", True, "expected an integer"),
    "over u8's range": ("u8", "300", 300, "out of range 0..=255"),
    "over f32's range": ("f32", "1e39", 1e39, "out of range ±3.4028234663852886e+38"),
    "bool over f64": ("f64", "#true", True, "expected a number"),
    "string over decimal64": ("decimal64", '"2"', "2", "expected a number"),
    "over decimal64's range": ("decimal64", "1e400", decimal.Decimal("1e400"), DECIMAL_RANGE),
    "past decimal128's range": ("decimal128", "-1e400", decimal.Decimal("-1e400"), DECIMAL_RANGE),
    "number over decimal": ("decimal", "1", 1, "expected a decimal"),
    "number over date-time": ("date-time", "5", 5, "expected a date-time"),
    "number over date": ("date", "5", 5, "expected a date"),
    "bad date": ("date", '"May 6th"', "May 6th", "expected an ISO 8601 date, like 2026-01-31"),
    "month out of range": ("date", '"2026-13-01"', "2026-13-01", "month must be in 1..12, not 13"),
    "day out of range": ("date", '"2026-02-30"', "2026-02-30", "day must be in 1..28, not 30"),
    # Gregorian leap years: every fourth year, except centuries, except every fourth century.
    "Feb 29 in a century": ("date", '"2100-02-29"', "2100-02-29", "day must be in 1..28, not 29"),
    "Feb 30 in a fourth century": ("date", '"2000-02-30"', "2000-02-30", "day must be in 1..29, not 30"),
    "year 0": ("date", '"0000-01-01"', "0000-01-01", "year must be in 1..9999, not 0"),
    "bad date-time": ("date-time", '"soon"', "soon", "expected an ISO 8601 date-time, like 2026-01-31T09:30:00"),
    "minute out of range": ("date-time", '"2026-01-01T12:60"', "2026-01-01T12:60", "minute must be in 0..59, not 60"),
    "bad time": ("time", '"half past"', "half past", "expected an ISO 8601 time, like 09:30:00"),
    "hour out of range": ("time", '"25:00"', "25:00", "hour must be in 0..23, not 25"),
    # Python 3.14 reads hour 24 as the end of the day; rkdl refuses it on every Python (task 59).
    "hour 24": ("time", '"24:00"', "24:00", "hour must be in 0..23, not 24"),
    "hour 24 with seconds": ("time", '"24:00:00"', "24:00:00", "hour must be in 0..23, not 24"),
    "hour 24 in a date-time": (
        "date-time",
        '"2026-01-31T24:00:00"',
        "2026-01-31T24:00:00",
        "hour must be in 0..23, not 24",
    ),
    "hour 24 after a week date": (
        "date-time",
        '"2026-W05-6T24:00"',
        "2026-W05-6T24:00",
        "hour must be in 0..23, not 24",
    ),
    "second out of range": ("time", '"12:00:61"', "12:00:61", "second must be in 0..59, not 61"),
    "offset out of range": ("time", '"12:00+25:00"', "12:00+25:00", "offset must be under 24 hours, not +25:00"),
    "bad decimal": ("decimal", '"lots"', "lots", "expected a decimal number, like 1.25"),
    "bad uuid": ("uuid", '"nope"', "nope", "expected 32 hex digits"),
    "non-hex uuid": (
        "uuid",
        '"g2345678-1234-5678-1234-567812345678"',
        "g2345678-1234-5678-1234-567812345678",
        "expected 32 hex digits",
    ),
    "null over time": ("time", "#null", None, "expected a time"),
    "number over ipv4": ("ipv4", "5", 5, "expected an IPv4 address"),
    "number over ipv6": ("ipv6", "5", 5, "expected an IPv6 address"),
    "number over uuid": ("uuid", "5", 5, "expected a UUID"),
    "number over regex": ("regex", "5", 5, "expected a regex"),
    "number over base64": ("base64", "5", 5, "expected base64 data"),
    "number over email": ("email", "5", 5, "expected a string"),
    "decode refuses": ("celsius", '"hot"', "hot", "its decode raised TypeError('want a number')"),
}


def reason(err):
    return str(err).split("doesn't fit ", 1)[1].split(": ", 1)[1]


@pytest.mark.parametrize(("ty", "text", "value", "why"), MISFITS.values(), ids=MISFITS.keys())
def test_a_misfit_has_one_reason_on_every_route(ty, text, value, why):
    n = rkdl.Parser(types=[STRICT_CELSIUS]).parse(f"n ({ty}){text} {text}\n")["n"]
    read = read_error(lambda n: n.args[0], n)
    typed = read_error(lambda n: setattr(n.args.entry(1), "type", ty), n)
    written = read_error(lambda n: n.args.append(rkdl.Value(value, type=ty)), n)
    over = read_error(lambda n: n.args.__setitem__(0, value), n)
    assert [reason(e) for e in (read, typed, written, over)] == [why] * 4


@pytest.mark.parametrize(
    ("value", "ty", "why"),
    [
        # Misfits only a write makes: a file can't hold a Python object.
        (datetime.datetime(2029, 1, 1, 10), "date", "would lose its time"),
        # Only a datetime has a time to lose; any other class is just not a date.
        (datetime.time(9, 30), "date", "expected a date"),
        (datetime.date(2029, 1, 1), "celsius", "expected a Celsius"),
    ],
)
def test_misfits_only_a_write_makes(value, ty, why):
    with pytest.raises(ValueError) as info:
        from_python(value, ty, [STRICT_CELSIUS])
    assert reason(info.value) == why


# A value nothing can write is a TypeError, whatever annotation it lands on.

UNWRITABLE = {"list": [1], "object": object(), "bytes pattern": re.compile(rb"x")}
STARTS = {"u8": "1", "date": '"2029-01-01"', "celsius": "1", "email": '"a@b.c"', "mine": "1"}


@pytest.mark.parametrize("ty", STARTS)
@pytest.mark.parametrize("value", UNWRITABLE.values(), ids=UNWRITABLE.keys())
def test_an_unwritable_value_is_a_type_error_over_any_annotation(value, ty):
    parser = rkdl.Parser(types=[STRICT_CELSIUS])
    doc = parser.parse(f"n ({ty}){STARTS[ty]}\n")
    n = doc["n"]
    with pytest.raises(TypeError, match="can't convert .* to a KDL value"):
        n.args[0] = value
    with pytest.raises(TypeError, match="can't convert .* to a KDL value"):
        n.args.append(rkdl.Value(value, type=ty))
    with pytest.raises(TypeError, match="can't convert .* to a KDL value"):
        parser.node("n", rkdl.Value(value, type=ty))
    assert str(doc) == f"n ({ty}){STARTS[ty]}\n"


@pytest.mark.parametrize(
    ("value", "ty", "why"),
    [
        (Celsius(3), "u8", "expected an integer"),
        (datetime.date(2029, 1, 1), "u8", "expected an integer"),
        (datetime.date(2029, 1, 1), "celsius", "expected a Celsius"),
    ],
)
def test_a_class_some_type_claims_is_writable_so_fit_decides(value, ty, why):
    with pytest.raises(ValueError) as info:
        from_python(value, ty, [STRICT_CELSIUS])
    assert reason(info.value) == why
