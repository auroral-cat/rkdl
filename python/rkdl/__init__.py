import collections.abc
import io
import os

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


def load(source):
    """Parses a KDL document from a path, a binary file or a text file.

    Paths and binary files are read as UTF-8 bytes, so line endings survive exactly. A text file opened without
    `newline=""` turns CRLF into LF as it reads.

    The same as `Parser().load(source)`.

    Args:
        source: A path, or a file opened for reading.

    Returns:
        The document.

    Raises:
        ParseError: The text isn't valid KDL. Its `path` names the file, when it has a name.
        UnicodeDecodeError: The bytes aren't UTF-8.
    """
    return Parser().load(source)


def dump(doc, target):
    """Writes a document's text to a path, a binary file or a text file.

    Paths and binary files get UTF-8 bytes, exactly as `str(doc)` has them. A text file opened without `newline=""`
    may turn LF into the platform's line ending as it writes.

    Args:
        doc: The document to write.
        target: A path, or a file opened for writing.
    """
    if isinstance(target, (str, os.PathLike)):
        with open(target, "wb") as f:
            f.write(str(doc).encode("utf-8"))
    elif _is_binary(target):
        target.write(str(doc).encode("utf-8"))
    else:
        target.write(str(doc))


def _read(source):
    # Parser.load's reading: the text, and the path to name in a ParseError, if there is one.
    if isinstance(source, (str, os.PathLike)):
        path = os.fsdecode(source)
        with open(source, "rb") as f:
            data = f.read()
    else:
        path = _path_of(source)
        data = source.read()
    if isinstance(data, (bytes, bytearray, memoryview)):
        data = bytes(data).decode("utf-8")
    return data, path


def _path_of(f):
    # open(fd) names the file by its descriptor, and sys.stdin is "<stdin>": neither is a path.
    name = getattr(f, "name", None)
    if not isinstance(name, (str, os.PathLike)):
        return None
    name = os.fsdecode(name)
    if name.startswith("<") and name.endswith(">"):
        return None
    return name


def _is_binary(f):
    if isinstance(f, io.TextIOBase):
        return False
    mode = getattr(f, "mode", "")
    return isinstance(f, (io.RawIOBase, io.BufferedIOBase)) or (isinstance(mode, str) and "b" in mode)
