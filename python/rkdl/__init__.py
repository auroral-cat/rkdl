from ._rkdl import (
    Diagnostic,
    Document,
    Formatter,
    ParseError,
    Parser,
    Type,
    __version__,
    parse,
)

__all__ = [
    "Diagnostic",
    "Document",
    "Formatter",
    "ParseError",
    "Parser",
    "Type",
    "__version__",
    "dump",
    "load",
    "parse",
]


def load(fp):
    return parse(fp.read())


def dump(doc, fp):
    fp.write(str(doc))
