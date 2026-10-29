import collections.abc

from ._rkdl import (
    Args,
    Children,
    Diagnostic,
    Document,
    Entries,
    Entry,
    Formatter,
    Node,
    ParseError,
    Parser,
    Props,
    Type,
    __version__,
    parse,
)

__all__ = [
    "Args",
    "Children",
    "Diagnostic",
    "Document",
    "Entries",
    "Entry",
    "Formatter",
    "Node",
    "ParseError",
    "Parser",
    "Props",
    "Type",
    "__version__",
    "dump",
    "load",
    "parse",
]


collections.abc.MutableSequence.register(Children)
collections.abc.MutableSequence.register(Args)
collections.abc.MutableMapping.register(Props)
collections.abc.Sequence.register(Entries)


def load(fp):
    return parse(fp.read())


def dump(doc, fp):
    fp.write(str(doc))
