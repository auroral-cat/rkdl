"""The convenience API: keeping spellings on write, building nodes, load and dump, and the list and dict operations."""

import copy
import datetime
import decimal
import io
import ipaddress
import re
import uuid

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


# Typed values in valid spellings that converting them would change: Python reads `(date-time)"...Z"` and writes it
# back as `+00:00`, a `Decimal` writes `1e5` as `1E+5`, and so on. Writing back the value an entry reads as must
# leave its text alone, whatever the type.


def int_spellings(low, high):
    for value in (0, 7, 127):
        yield from [f"{value}", f"+{value}", f"0x{value:X}", f"0x{value:x}", f"0o{value:o}", f"0b{value:b}"]
    yield "1_000" if high >= 1000 else "1_0"
    if low < 0:
        yield from ["-0x7F", "-1_0", "-0b1", "-0"]


FLOAT_SPELLINGS = ["1", "+1", "0x10", "1.0", "1.50", "1e3", "1.5E+3", "+2.5e-3", "1_000.5", "-0.0", "#inf", "#nan"]

STRING_SPELLINGS = {
    "decimal": ["1e5", "1.50", "-0", "Infinity", "0.10"],
    "date-time": [
        "2026-01-01T09:00:00Z",
        "2026-01-01 09:00:00",
        "2026-01-01T09:00:00.500Z",
        "2026-01-01T09:00:00.5+01:00",
        "2026-01-01T09:00",
        "20260101T090000Z",
    ],
    "date": ["2020-01-01", "20200101", "2020-W01-1"],
    "time": ["10:00:00Z", "10:00", "10:00:00.500", "10:00:00+01:00", "T10:00"],
    "ipv4": ["10.0.0.1"],
    "ipv6": ["0:0::1", "ABCD::1", "::ffff:10.0.0.1", "2001:db8:0:0:0:0:0:1"],
    "uuid": [
        "ABCDEF00-0000-0000-0000-000000000000",
        "{abcdef00-0000-0000-0000-000000000000}",
        "urn:uuid:abcdef00-0000-0000-0000-000000000000",
        "abcdef00000000000000000000000000",
    ],
    "regex": ["a\\d+", "(?i)x"],
    "base64": ["aGk=", ""],
}


def typed_spellings():
    """`(type)value` texts for every built-in reserved type. Spellings the running Python can't read are left out,
    since `fromisoformat` takes more forms in newer versions."""
    from rkdl._rkdl import _debug_reserved_types
    from rkdl._rkdl import _debug_to_python as to_python

    ranges = {
        **{f"i{bits}": (-(2 ** (bits - 1)), 2 ** (bits - 1) - 1) for bits in (8, 16, 32, 64, 128)},
        **{f"u{bits}": (0, 2**bits - 1) for bits in (8, 16, 32, 64, 128)},
        "isize": (-(2**63), 2**63 - 1),
        "usize": (0, 2**64 - 1),
    }
    texts = []
    for name, plain in _debug_reserved_types():
        if name in ranges:
            texts += [f"({name}){s}" for s in int_spellings(*ranges[name])]
        elif name in ("f32", "f64", "decimal64", "decimal128"):
            texts += [f"({name}){s}" for s in FLOAT_SPELLINGS]
            if name.startswith("decimal"):
                texts += [f"({name})1.000000000000000000001", f"({name})0.10"]
        else:
            strings = STRING_SPELLINGS[name] if not plain else ["P1D"]
            texts += [f'({name})"{s}"' for s in strings] + [f'({name})#"{s}"#' for s in strings]
            if plain:
                texts.append(f"({name})P1D")

    def readable(text):
        try:
            to_python(text)
        except ValueError:
            return False
        return True

    return [t for t in texts if readable(t)]


TYPED_SPELLINGS = typed_spellings()


def test_typed_spellings_cover_every_reserved_type():
    from rkdl._rkdl import _debug_reserved_types

    covered = {text[1 : text.index(")")] for text in TYPED_SPELLINGS}
    assert covered == {name for name, _ in _debug_reserved_types()}
    assert len(TYPED_SPELLINGS) > 300


def self_assign_arg(n):
    n.args[0] = n.args[0]


def self_assign_prop(n):
    n.props["k"] = n.props["k"]


def self_assign_entries(n):
    for e in n.entries:
        e.value = e.value


def bulk_props(n):
    n.props = {**n.props, "new": 1}


def bulk_args(n):
    n.args = [*n.args, 2]


@pytest.mark.parametrize(
    ("write", "added"),
    [
        (self_assign_arg, ""),
        (self_assign_prop, ""),
        (self_assign_entries, ""),
        (bulk_props, " new=1"),
        (bulk_args, None),
    ],
)
def test_writing_back_a_typed_value_keeps_its_spelling(write, added):
    changed = []
    for text in TYPED_SPELLINGS:
        source = f"n {text} k={text}"
        expected = f"n {text} 2 k={text}" if added is None else source + added
        doc = rkdl.parse(source)
        write(doc["n"])
        if str(doc) != expected:
            changed.append((text, str(doc)))
    assert changed == []


def test_an_equal_value_built_anew_keeps_the_spelling():
    import datetime
    import uuid

    doc, n = node('n (date-time)"2026-01-01T09:00:00Z" (uuid)"ABCDEF00-0000-0000-0000-000000000000"')
    n.args[0] = datetime.datetime(2026, 1, 1, 9, tzinfo=datetime.UTC)
    n.args[1] = uuid.UUID("abcdef00-0000-0000-0000-000000000000")
    assert str(doc) == 'n (date-time)"2026-01-01T09:00:00Z" (uuid)"ABCDEF00-0000-0000-0000-000000000000"'


def test_an_equal_value_that_writes_differently_is_a_write():
    import datetime
    import decimal

    # The same instant in another time zone is equal by ==, but it's another value to write.
    doc, n = node('n (date-time)"2026-01-01T09:00:00Z" (decimal)"1e5"')
    plus_one = datetime.timezone(datetime.timedelta(hours=1))
    n.args[0] = datetime.datetime(2026, 1, 1, 10, tzinfo=plus_one)
    # Decimal("100000") == Decimal("1e5"), but it has other digits.
    n.args[1] = decimal.Decimal(100000)
    assert str(doc) == 'n (date-time)"2026-01-01T10:00:00+01:00" (decimal)"100000"'


class Hex:
    def __init__(self, number):
        self.number = number


HEX = rkdl.Type("hex", Hex, decode=lambda s: Hex(int(s, 16)), encode=lambda h: format(h.number, "x"))


def test_a_custom_value_that_writes_back_the_same_keeps_its_spelling():
    doc = rkdl.Parser(types=[HEX]).parse('n (hex)"FF" (hex)"0a"')
    n = doc["n"]
    n.args = [Hex(255), Hex(11)]
    assert str(doc) == 'n (hex)"FF" (hex)"b"'


def raising_hex(error):
    def decode(value):
        raise error

    return rkdl.Parser(types=[rkdl.Type("hex", Hex, decode=decode, encode=lambda h: h.number)])


def test_an_old_value_that_cant_be_read_is_simply_overwritten():
    # Checking whether the write is a no-op reads the old value, and here that raises.
    doc = raising_hex(ValueError("no")).parse("n (hex)1")
    doc["n"].args[0] = Hex(2)
    assert str(doc) == "n (hex)2"


def test_a_base_exception_while_reading_the_old_value_propagates():
    doc = raising_hex(KeyboardInterrupt).parse("n (hex)1")
    with pytest.raises(KeyboardInterrupt):
        doc["n"].args[0] = Hex(2)
    assert str(doc) == "n (hex)1"


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

# rkdl.load is Parser().load, and a Parser with custom types loads the same way.
LOADERS = pytest.mark.parametrize(
    "load", [rkdl.load, rkdl.Parser().load, rkdl.Parser(types=[CELSIUS]).load], ids=["rkdl", "Parser", "custom"]
)


@LOADERS
def test_load_and_dump_through_a_path_keep_bytes(tmp_path, load):
    source, target = tmp_path / "in.kdl", tmp_path / "out.kdl"
    source.write_bytes(CRLF)
    doc = load(source)
    doc["b"].children.append(rkdl.Node("d"))
    rkdl.dump(doc, str(target))
    assert target.read_bytes() == b"a 1\r\nb {\r\n    c\r\n    d\r\n}\r\n"
    assert str(load(str(source))) == CRLF.decode()


@LOADERS
def test_load_and_dump_through_binary_files_keep_bytes(tmp_path, load):
    doc = load(io.BytesIO(CRLF))
    out = io.BytesIO()
    rkdl.dump(doc, out)
    assert out.getvalue() == CRLF
    path = tmp_path / "x.kdl"
    with open(path, "wb") as f:
        rkdl.dump(doc, f)
    with open(path, "rb") as f:
        assert str(load(f)) == CRLF.decode()
    assert path.read_bytes() == CRLF


@LOADERS
def test_load_and_dump_through_text_files(tmp_path, load):
    out = io.StringIO()
    rkdl.dump(load(io.StringIO("a 1\n")), out)
    assert out.getvalue() == "a 1\n"
    path = tmp_path / "x.kdl"
    path.write_bytes(CRLF)
    with open(path, newline="") as f:
        assert str(load(f)) == CRLF.decode()
    # Without newline="", a text file turns CRLF into LF as it reads.
    with open(path, encoding="utf-8") as f:
        assert str(load(f)) == CRLF.decode().replace("\r\n", "\n")


@LOADERS
def test_load_rejects_bytes_that_are_not_utf8(load):
    with pytest.raises(UnicodeDecodeError):
        load(io.BytesIO(b"a \xff"))


def test_parser_load_reads_through_its_types(tmp_path):
    path = tmp_path / "x.kdl"
    path.write_bytes(b"t (celsius)21.5\r\n")
    parser = rkdl.Parser(types=[CELSIUS])
    for source in (path, str(path), io.BytesIO(path.read_bytes())):
        doc = parser.load(source)
        assert doc["t"].args[0] == Celsius(21.5)
        assert str(doc) == "t (celsius)21.5\r\n"
    # Without the Type, the annotation is one rkdl doesn't know.
    assert rkdl.load(path)["t"].args[0] == 21.5


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
    # strict_celsius refuses an int, so it doesn't fit.
    message = "3 doesn't fit (celsius): its decode raised TypeError('celsius is a float')"
    with pytest.raises(ValueError, match=re.escape(message)) as raised:
        n.args[1] = 3
    assert isinstance(raised.value.__cause__, TypeError)
    assert str(doc) == "n (celsius)22.0 (celsius)1.0\n"


# A plain value a custom decode accepts is written in the Type's own form, encode(decode(value)).

LOOSE_CELSIUS = rkdl.Type("celsius", Celsius, decode=lambda v: Celsius(float(v)), encode=lambda c: c.degrees)
VERSION = rkdl.Type(
    "version",
    tuple,
    decode=lambda s: tuple(int(part) for part in s.split(".")),
    encode=lambda v: ".".join(str(part) for part in v),
)


@pytest.mark.parametrize(
    ("types", "text", "value", "written"),
    [
        ([LOOSE_CELSIUS], "n (celsius)180.0", "21.5", "n (celsius)21.5"),
        ([LOOSE_CELSIUS], "n (celsius)180.0", 14, "n (celsius)14.0"),
        ([LOOSE_CELSIUS], "n (celsius)180.0", rkdl.Value("3", type="celsius"), "n (celsius)3.0"),
        ([VERSION], 'n (version)"1.0"', "1.02.3", 'n (version)"1.2.3"'),
        ([rkdl.Type("u8", int, int, int)], "n (u8)1", 300, "n (u8)300"),
    ],
)
def test_a_plain_value_a_custom_decode_accepts_is_written_in_the_types_form(types, text, value, written):
    doc = rkdl.Parser(types=types).parse(text + "\n")
    doc["n"].args[0] = value
    assert str(doc) == written + "\n"


def test_an_equal_plain_write_over_a_custom_type_keeps_the_spelling():
    doc = rkdl.Parser(types=[LOOSE_CELSIUS, VERSION]).parse('n (celsius)21.50 (version)#"1.2"#\n')
    n = doc["n"]
    n.args[0] = "21.5"
    n.args[1] = "1.02"
    assert str(doc) == 'n (celsius)21.50 (version)#"1.2"#\n'


def test_a_plain_write_whose_encode_fails_changes_nothing():
    class Refused(Exception):
        pass

    def encode(c):
        raise Refused

    refusing = rkdl.Type("celsius", Celsius, decode=lambda v: Celsius(float(v)), encode=encode)
    returns_a_list = rkdl.Type("celsius", Celsius, decode=lambda v: Celsius(float(v)), encode=lambda c: [c.degrees])
    for ty, raised in [(refusing, Refused), (returns_a_list, TypeError)]:
        doc = rkdl.Parser(types=[ty]).parse("n (celsius)1 k=(celsius)1\n")
        n = doc["n"]
        with pytest.raises(raised):
            n.args[0] = "2.5"
        with pytest.raises(raised):
            n.props.update(k=2.5)
        assert str(doc) == "n (celsius)1 k=(celsius)1\n"


def test_a_plain_write_whose_decode_fails_changes_nothing():
    doc = rkdl.Parser(types=[LOOSE_CELSIUS]).parse("n (celsius)1\n")
    with pytest.raises(ValueError, match=re.escape("'hot' doesn't fit (celsius): its decode raised ValueError(")):
        doc["n"].args[0] = "hot"
    assert str(doc) == "n (celsius)1\n"


def test_a_plain_write_to_a_builtin_type_keeps_it_when_it_fits():
    doc, n = node("n (u8)1 (u8)2\n")
    n.args[0] = 3
    assert str(doc) == "n (u8)3 (u8)2\n"
    with pytest.raises(ValueError, match=re.escape("'x' doesn't fit (u8): expected an integer")):
        n.args[1] = "x"
    assert str(doc) == "n (u8)3 (u8)2\n"


@pytest.mark.parametrize(
    ("ty", "old", "fits", "read", "misfit", "reason"),
    [
        (
            "date",
            '"2026-01-01"',
            "2029-01-01",
            "datetime.date(2029, 1, 1)",
            "next tuesday",
            "Invalid isoformat string: 'next tuesday'",
        ),
        (
            "date-time",
            '"2026-01-01T09:00:00"',
            "2029-01-01T10:00:00",
            "datetime.datetime(2029, 1, 1, 10, 0)",
            "soon",
            "Invalid isoformat string: 'soon'",
        ),
        ("time", '"09:00:00"', "10:30:00", "datetime.time(10, 30)", "half past", "Invalid isoformat string"),
        ("ipv4", '"10.0.0.1"', "10.0.0.2", "IPv4Address('10.0.0.2')", "::1", "Expected 4 octets"),
        ("ipv6", '"::1"', "::2", "IPv6Address('::2')", "10.0.0.2", "At least 3 parts expected"),
        (
            "uuid",
            '"00000000-0000-0000-0000-000000000000"',
            "12345678-1234-5678-1234-567812345678",
            "UUID('12345678-1234-5678-1234-567812345678')",
            "not-a-uuid",
            "badly formed hexadecimal UUID string",
        ),
        ("regex", '"a+"', "b+", "re.compile('b+')", "(unclosed", "missing ), unterminated subpattern"),
        ("base64", '"aGk="', "aGV5", "b'hey'", "not base64!", ""),
        ("decimal", '"1.5"', "2.25", "Decimal('2.25')", "lots", ""),
    ],
)
def test_a_plain_string_a_builtin_type_reads_keeps_the_annotation(ty, old, fits, read, misfit, reason):
    text = f"n ({ty}){old} k=({ty}){old}\n"
    doc, n = node(text)
    with pytest.raises(ValueError, match=re.escape(f"{misfit!r} doesn't fit ({ty}): {reason}")):
        n.props["k"] = misfit
    assert str(doc) == text
    n.args[0] = fits
    assert str(doc) == f'n ({ty})"{fits}" k=({ty}){old}\n'
    assert repr(n.args[0]) == read
    assert str(rkdl.parse(str(doc))) == str(doc)


@pytest.mark.parametrize(
    ("ty", "old", "value", "text"),
    [
        # A number the type reads keeps it, an int too.
        ("decimal64", "1.5", 2, "(decimal64)2"),
        ("decimal128", "1.5", 0.1, "(decimal128)0.1"),
        ("f64", "1.5", 2, "(f64)2.0"),
    ],
)
def test_a_plain_number_a_builtin_type_reads_keeps_it(ty, old, value, text):
    doc, n = node(f"n ({ty}){old}\n")
    n.args[0] = value
    assert str(doc) == f"n {text}\n"


@pytest.mark.parametrize(
    ("ty", "old", "value", "message"),
    [
        ("u8", "1", True, "True doesn't fit (u8): expected an integer"),
        ("f32", "1.5", "1.5", "'1.5' doesn't fit (f32): expected a number"),
        ("date", '"2026-01-01"', 5, "5 doesn't fit (date): expected a date"),
        ("date", '"2026-01-01"', None, "None doesn't fit (date): expected a date"),
        ("decimal64", "1.5", "1.5", "'1.5' doesn't fit (decimal64): expected a number"),
        ("decimal", '"1.5"', 1, "1 doesn't fit (decimal): expected a decimal"),
        ("email", '"a@b.c"', 5, "5 doesn't fit (email): expected a string"),
    ],
)
def test_other_plain_values_over_a_builtin_type_raise(ty, old, value, message):
    text = f"n ({ty}){old}\n"
    doc, n = node(text)
    with pytest.raises(ValueError, match=re.escape(message)):
        n.args[0] = value
    assert str(doc) == text


def test_changing_an_annotation_is_explicit():
    doc, n = node("n retries=(u8)7\n")
    entry = n.props.entry("retries")
    entry.type = None
    n.props["retries"] = "weekly"
    assert str(doc) == "n retries=weekly\n"
    n.props["retries"] = 7
    entry.type = "i32"
    n.props["retries"] = -1
    assert str(doc) == "n retries=(i32)-1\n"
    with pytest.raises(ValueError, match="out of range 0..=255"):
        entry.type = "u8"


def test_an_unknown_annotation_isnt_checked():
    doc, n = node("n (mine)1\n")
    n.args[0] = "x"
    n.args[0] = None
    assert str(doc) == "n (mine)#null\n"


# Writes over an annotated entry: each annotation rkdl knows, with a value of its class, a plain value it reads as (or
# a number it converts), and one that doesn't fit, through every route that writes over an existing entry.

FIT_CASES = [
    # (annotation, old value text, value of its class, read back as, plain value it reads, read back as, misfit, message)
    ("u8", "1", 200, 200, 7.0, 7, "weekly", "'weekly' doesn't fit (u8): expected an integer"),
    ("i32", "1", -5, -5, -3.0, -3, 2**40, "1099511627776 doesn't fit (i32): out of range -2147483648..=2147483647"),
    ("f64", "1.5", 2.5, 2.5, 2, 2.0, True, "True doesn't fit (f64): expected a number"),
    ("f32", "1.5", 0.5, 0.5, 3, 3.0, 1e39, "1e+39 doesn't fit (f32): out of range"),
    (
        "decimal64",
        "1.5",
        decimal.Decimal("1.25"),
        decimal.Decimal("1.25"),
        2,
        decimal.Decimal(2),
        "2",
        "'2' doesn't fit (decimal64): expected a number",
    ),
    (
        "decimal",
        '"1.5"',
        decimal.Decimal("1.25"),
        decimal.Decimal("1.25"),
        "2.5",
        decimal.Decimal("2.5"),
        1.5,
        "1.5 doesn't fit (decimal): expected a decimal",
    ),
    (
        "date",
        '"2026-01-01"',
        datetime.date(2029, 1, 1),
        datetime.date(2029, 1, 1),
        "2029-01-02",
        datetime.date(2029, 1, 2),
        datetime.datetime(2029, 1, 1, 9),
        "datetime.datetime(2029, 1, 1, 9, 0) doesn't fit (date): would lose its time",
    ),
    (
        "date-time",
        '"2026-01-01T09:00:00"',
        datetime.datetime(2029, 1, 1, 9),
        datetime.datetime(2029, 1, 1, 9),
        "2029-01-01T10:00:00",
        datetime.datetime(2029, 1, 1, 10),
        datetime.date(2029, 1, 1),
        "datetime.date(2029, 1, 1) doesn't fit (date-time): expected a date-time",
    ),
    (
        "time",
        '"09:00:00"',
        datetime.time(10, 30),
        datetime.time(10, 30),
        "11:00:00",
        datetime.time(11),
        None,
        "None doesn't fit (time): expected a time",
    ),
    (
        "ipv4",
        '"10.0.0.1"',
        ipaddress.IPv4Address("10.0.0.2"),
        ipaddress.IPv4Address("10.0.0.2"),
        "10.0.0.3",
        ipaddress.IPv4Address("10.0.0.3"),
        ipaddress.IPv6Address("::1"),
        "IPv6Address('::1') doesn't fit (ipv4): expected an IPv4 address",
    ),
    (
        "ipv6",
        '"::1"',
        ipaddress.IPv6Address("::2"),
        ipaddress.IPv6Address("::2"),
        "::3",
        ipaddress.IPv6Address("::3"),
        "::g",
        "'::g' doesn't fit (ipv6): ",
    ),
    (
        "uuid",
        '"00000000-0000-0000-0000-000000000000"',
        uuid.UUID(int=1),
        uuid.UUID(int=1),
        "00000000-0000-0000-0000-000000000002",
        uuid.UUID(int=2),
        2,
        "2 doesn't fit (uuid): expected a UUID",
    ),
    ("regex", '"a+"', re.compile("b+"), re.compile("b+"), "c+", re.compile("c+"), "(", "'(' doesn't fit (regex): "),
    ("base64", '"aGk="', b"hey", b"hey", "aGk=", b"hi", "hi", "'hi' doesn't fit (base64): "),
    ("email", '"a@b.c"', "d@e.f", "d@e.f", "g@h.i", "g@h.i", 5, "5 doesn't fit (email): expected a string"),
]

WRITE_ROUTES = {
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


def written(n, route):
    return n.props.entry("k") if route.startswith(("prop", "node.props")) else n.args.entry(0)


@pytest.mark.parametrize("route", WRITE_ROUTES)
@pytest.mark.parametrize(("ty", "old", "value", "read", "plain", "plain_read", "misfit", "message"), FIT_CASES)
def test_writes_over_a_builtin_annotation_fit_it_or_raise(
    route, ty, old, value, read, plain, plain_read, misfit, message
):
    write = WRITE_ROUTES[route]
    text = f"n ({ty}){old} k=({ty}){old}\n"
    doc, n = node(text)
    with pytest.raises(ValueError, match="^" + re.escape(message)):
        write(n, misfit)
    assert str(doc) == text
    for v, expected in [(value, read), (plain, plain_read)]:
        write(n, v)
        entry = written(n, route)
        assert entry.type == ty
        assert entry.value == expected
        assert type(entry.value) is type(expected)
        assert str(rkdl.parse(str(doc))) == str(doc)


CUSTOM_CASES = [
    (Celsius(3.0), Celsius(3.0), "(celsius)3.0"),
    (4.0, Celsius(4.0), "(celsius)4.0"),
    (3, None, "3 doesn't fit (celsius): its decode raised TypeError('celsius is a float')"),
    ("x", None, "'x' doesn't fit (celsius): its decode raised TypeError('celsius is a float')"),
    (datetime.date(2029, 1, 1), None, "datetime.date(2029, 1, 1) doesn't fit (celsius): expected a Celsius"),
]


@pytest.mark.parametrize("route", WRITE_ROUTES)
@pytest.mark.parametrize(("value", "read", "expected"), CUSTOM_CASES)
def test_writes_over_a_custom_annotation_fit_it_or_raise(route, value, read, expected):
    write = WRITE_ROUTES[route]
    text = "n (celsius)1.0 k=(celsius)1.0\n"
    doc = rkdl.Parser(types=[CELSIUS]).parse(text)
    n = doc["n"]
    if read is None:
        with pytest.raises(ValueError, match="^" + re.escape(expected)):
            write(n, value)
        assert str(doc) == text
    else:
        write(n, value)
        entry = written(n, route)
        assert entry.value == read
        assert expected in str(doc)


def test_a_misfit_among_several_writes_changes_nothing():
    text = 'n (u8)1 (date)"2026-01-01" k=(u8)2 j=(ipv4)"10.0.0.1"\n'
    doc, n = node(text)
    with pytest.raises(ValueError, match=re.escape("'x' doesn't fit (date): ")):
        n.args = [5, "x"]
    with pytest.raises(ValueError, match=re.escape("'10.0.0.300' doesn't fit (ipv4): ")):
        n.props.update(k=3, j="10.0.0.300")
    assert str(doc) == text


def test_a_kept_builtin_annotation_keeps_the_strings_spelling():
    doc, n = node('n (date)#"2026-01-01"# (ipv4)"10.0.0.1"\n')
    n.args[0] = "2029-01-01"
    n.args[1] = "10.0.0.1"
    assert str(doc) == 'n (date)#"2029-01-01"# (ipv4)"10.0.0.1"\n'


OUT_OF_RANGE = "300 doesn't fit (u8): out of range 0..=255"


@pytest.mark.parametrize(
    "write",
    [
        lambda n: n.args.__setitem__(0, 300),
        lambda n: n.args.__setitem__(slice(0, 2), [1, 300]),
        lambda n: n.args.__setitem__(slice(None), [1, 300, 4]),
        lambda n: setattr(n, "args", [1, 300]),
        lambda n: n.args.entry(1).__setattr__("value", 300),
        lambda n: n.props.__setitem__("k", 300),
        lambda n: n.props.update({"j": 1, "k": 300}),
        lambda n: setattr(n, "props", {"j": 1, "k": 300}),
        lambda n: n.props.entry("k").__setattr__("value", 300),
    ],
)
def test_an_out_of_range_write_to_a_builtin_type_raises_and_changes_nothing(write):
    text = "n (u8)1 (u8)2 j=(u8)3 k=(u8)4\n"
    doc, n = node(text)
    with pytest.raises(ValueError, match=re.escape(OUT_OF_RANGE)):
        write(n)
    assert str(doc) == text


@pytest.mark.parametrize(
    "write",
    [
        lambda n: n.args.__setitem__(0, 3.5),
        lambda n: n.args.__setitem__(slice(0, 2), [1, 3.5]),
        lambda n: setattr(n, "args", [1, 3.5]),
        lambda n: n.args.entry(1).__setattr__("value", 3.5),
        lambda n: n.props.update({"j": 1, "k": 3.5}),
        lambda n: setattr(n, "props", {"j": 1, "k": 3.5}),
        lambda n: n.props.entry("k").__setattr__("value", 3.5),
    ],
)
def test_a_fractional_float_over_an_integer_type_raises_and_changes_nothing(write):
    text = "n (u8)1 (u8)2 j=(u8)3 k=(u8)4\n"
    doc, n = node(text)
    with pytest.raises(ValueError, match=re.escape("3.5 doesn't fit (u8): expected an integer")):
        write(n)
    assert str(doc) == text


def test_a_whole_float_over_an_integer_type_converts():
    doc, n = node("n (u8)1 k=(i32)2\n")
    n.args[0] = 7.0
    n.props["k"] = -3.0
    assert str(doc) == "n (u8)7 k=(i32)-3\n"
    assert n.args[0] == 7
    assert type(n.args[0]) is int


def test_a_type_whose_decode_raises_is_named_as_written():
    def refuse(value):
        raise LookupError("no")

    spaced = rkdl.Type("my type", Celsius, decode=refuse, encode=lambda c: c.degrees)
    doc = rkdl.Parser(types=[spaced]).parse("n 5\n")
    with pytest.raises(
        ValueError, match=re.escape("""5 doesn't fit ("my type"): its decode raised LookupError('no')""")
    ):
        doc["n"].args.entry(0).type = "my type"
    assert str(doc) == "n 5\n"


def test_new_entries_have_no_range_to_check():
    doc, n = node("n (u8)1\n")
    n.args.append(300)
    n.args.insert(0, 301)
    n.args.extend([302])
    n.props["k"] = 303
    assert str(doc) == "n 301 (u8)1 300 302 k=303\n"


def test_float_types_check_their_range():
    doc, n = node("n (f32)1.5 (f64)2.5\n")
    with pytest.raises(ValueError, match=re.escape("1e+39 doesn't fit (f32): out of range ±3.4028234663852886e+38")):
        n.args[0] = 1e39
    n.args[0] = 3
    n.args[1] = 1e39
    assert str(doc) == "n (f32)3.0 (f64)1e39\n"


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


def test_props_update_takes_a_property_named_other():
    doc, n = node("n a=1\n")
    n.props.update(other=2)
    assert str(doc) == "n a=1 other=2\n"


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
    # A view equals another view the way a list equals a list.
    assert n.children == n.children
    assert n.children != [n["a"]]
    assert n.children != doc.children
    doc = rkdl.parse("x {}\ny {}\n")
    assert doc["x"].children == doc["y"].children


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


def test_renaming_a_property_onto_another_key_raises():
    doc, n = node("n k=1 j=2 j=3\n")
    with pytest.raises(ValueError, match='already has a property "j"'):
        n.props.entry("k").name = "j"
    assert str(doc) == "n k=1 j=2 j=3\n"
    assert n.props == {"k": 1, "j": 3}


def test_renaming_a_property_to_its_own_name_changes_nothing():
    # Even where the key is a duplicate: the rename doesn't add one.
    doc, n = node('n "j"=2 j=3\n')
    n.entries[0].name = "j"
    n.entries[1].name = "j"
    assert str(doc) == 'n "j"=2 j=3\n'


# reprs don't raise an Exception


def test_reprs_of_removed_and_blockless_things():
    doc, n = node("n 1 {\n    c\n}\n")
    entry = n.args.entry(0)
    del n.args[0]
    assert repr(entry) == "<Entry (removed)>"
    children = n.children
    n.children = None
    assert repr(children) == "Children(<no children block>)"
    assert repr(doc) == "<Document with 1 nodes>"


def test_children_repr_lists_the_nodes():
    doc = rkdl.parse('server { listen; "log level" }\nempty {}\n')
    assert repr(doc.children) == "Children([<Node server>, <Node empty>])"
    assert repr(doc["server"].children) == 'Children([<Node listen>, <Node "log level">])'
    assert repr(doc["empty"].children) == "Children([])"


def test_reprs_survive_a_raising_repr():
    class Loud:
        def __repr__(self):
            raise RuntimeError("no")

    loud = rkdl.Type("loud", Loud, decode=lambda v: Loud(), encode=lambda v: 1)
    doc = rkdl.Parser(types=[loud]).parse("n (loud)1 k=(loud)2\n")
    n = doc["n"]
    assert repr(n.args) == "Args([<kdl: (loud)1>])"
    assert repr(n.props) == "Props({'k': <kdl: (loud)2>})"
    assert repr(n.args.entry(0)) == "<Entry <kdl: (loud)1>>"


@pytest.mark.parametrize("error", [KeyboardInterrupt, SystemExit])
def test_reprs_let_a_base_exception_from_decode_through(error):
    def decode(value):
        raise error

    doc = rkdl.Parser(types=[rkdl.Type("stop", int, decode=decode, encode=int)]).parse("n (stop)1 k=(stop)2\n")
    n = doc["n"]
    for shown in (n.args, n.props, n.args.entry(0), n.props.entry("k")):
        with pytest.raises(error):
            repr(shown)
