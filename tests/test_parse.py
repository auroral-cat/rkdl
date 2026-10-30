import io

import rkdl

SOURCE = """\
// a header comment
server "web" port=0x1F90 {
    /* block comment */ path #"C:\\raw\\path"#
    /-disabled 1
    mode 0o755 0b1010 // trailing comment
}
/-gone {
    child
}
empty {}
"""


def test_round_trip_is_byte_for_byte():
    assert str(rkdl.parse(SOURCE)) == SOURCE


def test_parser_parses_like_parse():
    assert str(rkdl.Parser().parse(SOURCE)) == SOURCE


def test_load_dump_round_trip():
    doc = rkdl.load(io.StringIO(SOURCE))
    out = io.StringIO()
    rkdl.dump(doc, out)
    assert out.getvalue() == SOURCE


def test_version():
    assert isinstance(rkdl.__version__, str)
