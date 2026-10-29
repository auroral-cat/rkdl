import collections.abc

from ._rkdl import (
    Children,
    Diagnostic,
    Document,
    Formatter,
    Node,
    ParseError,
    Parser,
    Type,
    __version__,
    parse,
)

__all__ = [
    "Children",
    "Diagnostic",
    "Document",
    "Formatter",
    "Node",
    "ParseError",
    "Parser",
    "Type",
    "__version__",
    "dump",
    "load",
    "parse",
]


collections.abc.MutableSequence.register(Children)


def load(fp):
    return parse(fp.read())


def dump(doc, fp):
    fp.write(str(doc))
