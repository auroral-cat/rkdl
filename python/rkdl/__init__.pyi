"""Format-preserving KDL for Python.

Parse a KDL document, edit it, and print it back with everything you didn't touch unchanged, comments included.
"""

import os
from collections.abc import (
    Callable,
    ItemsView,
    Iterable,
    Iterator,
    KeysView,
    Mapping,
    MutableMapping,
    MutableSequence,
    Sequence,
    ValuesView,
)
from typing import Any, ClassVar, Protocol, Self, SupportsIndex, TypeAlias, TypeVar, final, overload, type_check_only

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

__version__: str

_T = TypeVar("_T")

# A node key: a name, or a `(type, name)` pair where a None type matches nodes without one.
_Key: TypeAlias = str | tuple[str | None, str]

# Values read from a document are typed Any: plain KDL values become str, int, float, bool or None, the reserved
# type annotations give datetime, Decimal, UUID and friends, and a custom Type can return anything.

@type_check_only
class _SupportsKeysAndGetItem(Protocol):
    def keys(self) -> Iterable[str]: ...
    def __getitem__(self, key: str, /) -> Any: ...

@type_check_only
class _WritesStr(Protocol):
    def write(self, text: str, /) -> object: ...

@type_check_only
class _WritesBytes(Protocol):
    def write(self, data: bytes, /) -> object: ...

@type_check_only
class _Readable(Protocol):
    def read(self) -> str | bytes | bytearray | memoryview: ...

class ParseError(ValueError):
    """The text isn't valid KDL.

    `str(error)` is a report that points at each problem in the text. The attributes give the first problem's
    position, counted in characters.

    Attributes:
        line: The first problem's line, starting at 1.
        column: The first problem's column, starting at 1.
        offset: The first problem's offset from the start of the text, starting at 0.
        diagnostics: Every problem found, in order.
        path: The file `load` read the text from, or None if it didn't come from a named file.
    """

    line: int | None
    column: int | None
    offset: int | None
    diagnostics: list[Diagnostic]
    path: str | None

@final
class Diagnostic:
    """One problem found while parsing.

    Positions count characters, not bytes. Diagnostics compare equal when all their fields do, and can be pickled.
    """

    def __new__(
        cls,
        message: str,
        label: str | None,
        help: str | None,
        offset: int,
        length: int,
        line: int,
        column: int,
    ) -> Self: ...
    @property
    def message(self) -> str:
        """What's wrong."""
    @property
    def label(self) -> str | None:
        """A short note on the problem's span of text, if there is one."""
    @property
    def help(self) -> str | None:
        """A hint on how to fix it, if there is one."""
    @property
    def offset(self) -> int:
        """Where the problem starts, in characters from the start of the text."""
    @property
    def length(self) -> int:
        """How many characters the problem spans."""
    @property
    def line(self) -> int:
        """The line the problem starts on, starting at 1."""
    @property
    def column(self) -> int:
        """The column the problem starts at, starting at 1."""
    def __getnewargs__(self) -> tuple[str, str | None, str | None, int, int, int, int]: ...
    def __eq__(self, other: object, /) -> bool: ...
    def __hash__(self) -> int: ...

@final
class Formatter:
    """Settings for `Document.fmt`, which mirror kdl-rs's `FormatConfig`.

    Args:
        indent: The text for one level of indentation. Only KDL whitespace, such as spaces or tabs.
        no_comments: Remove comments.
        entry_autoformat_keep: Leave each argument and property as written, with the space in front of it, and only
            reformat the nodes around them.

    Raises:
        ValueError: `indent` holds something other than whitespace.
    """

    def __new__(
        cls,
        indent: str = "    ",
        no_comments: bool = False,
        entry_autoformat_keep: bool = False,
    ) -> Self: ...
    @property
    def indent(self) -> str:
        """The text for one level of indentation."""
    @property
    def no_comments(self) -> bool:
        """Whether comments are removed."""
    @property
    def entry_autoformat_keep(self) -> bool:
        """Whether arguments and properties are left as written."""

@final
class Type:
    """A custom type annotation: how to read and write the values annotated with it.

    Pass Types to a `Parser`. A Type with the same name as a reserved one, such as `date`, replaces it.

    Args:
        name: The annotation, without parentheses: `"celsius"` for `(celsius)22.5`.
        cls: The Python class of the values. Writing an instance of it annotates the entry with `name`.
        decode: Turns the plain value from the document (a str, int, float, bool or None) into a Python value.
        encode: Turns an instance of `cls` into a plain value to write.

    Raises:
        TypeError: `decode` or `encode` isn't callable.
    """

    def __new__(
        type_: type[Self], name: str, cls: type[_T], decode: Callable[[Any], _T], encode: Callable[[_T], Any]
    ) -> Self: ...
    @property
    def name(self) -> str:
        """The annotation this Type handles."""
    @property
    def cls(self) -> type:
        """The Python class of the values."""
    @property
    def decode(self) -> Callable[[Any], Any]:
        """Turns a plain value from the document into a Python value."""
    @property
    def encode(self) -> Callable[[Any], Any]:
        """Turns a Python value into a plain value to write."""

@final
class Parser:
    """Parses KDL text into documents, with custom types and formatting.

    Args:
        types: Custom type annotations, used by every document this parser makes. A later Type replaces an earlier
            one with the same name.
        formatter: If given, each document is formatted with it right after parsing.
    """

    def __new__(cls, types: Sequence[Type] | None = None, formatter: Formatter | None = None) -> Self: ...
    def parse(self, text: str) -> Document:
        """Parses a KDL document.

        Args:
            text: The KDL text.

        Returns:
            The document, which reads and writes values through this parser's types.

        Raises:
            ParseError: The text isn't valid KDL.
        """
    def node(
        self,
        name: str,
        *args: Any,
        type: str | None = None,
        props: dict[str, Any] | None = None,
        children: Iterable[Node] | None = None,
        **kwargs: Any,
    ) -> Node:
        """Makes a detached node that reads and writes values through this parser's types.

        Takes the same arguments as `Node`.
        """

def parse(text: str) -> Document:
    """Parses a KDL document.

    The same as `Parser().parse(text)`.

    Args:
        text: The KDL text.

    Returns:
        The document.

    Raises:
        ParseError: The text isn't valid KDL.
    """

def load(source: str | os.PathLike[str] | _Readable) -> Document:
    """Parses a KDL document from a path, a binary file or a text file.

    Paths and binary files are read as UTF-8 bytes, so line endings survive exactly. A text file opened without
    `newline=""` turns CRLF into LF as it reads.

    Args:
        source: A path, or a file opened for reading.

    Returns:
        The document.

    Raises:
        ParseError: The text isn't valid KDL. Its `path` names the file, when it has a name.
        UnicodeDecodeError: The bytes aren't UTF-8.
    """

def dump(doc: Document, target: str | os.PathLike[str] | _WritesStr | _WritesBytes) -> None:
    """Writes a document's text to a path, a binary file or a text file.

    Paths and binary files get UTF-8 bytes, exactly as `str(doc)` has them. A text file opened without `newline=""`
    may turn LF into the platform's line ending as it writes.

    Args:
        doc: The document to write.
        target: A path, or a file opened for writing.
    """

@final
class Document:
    """A KDL document: its top-level nodes, plus every bit of formatting and every comment in the text.

    `str(doc)` gives the text back, with your edits. `Document()` is an empty document. To read custom types, parse
    an empty string with a `Parser` instead.
    """

    def __new__(cls) -> Self: ...
    @property
    def children(self) -> Children:
        """The top-level nodes, as a live list.

        Assigning a list of detached nodes replaces them all. If any of the nodes can't go in, nothing changes.
        """
    @children.setter
    def children(self, value: Iterable[Node]) -> None: ...
    def node(
        self,
        name: str,
        *args: Any,
        type: str | None = None,
        props: dict[str, Any] | None = None,
        children: Iterable[Node] | None = None,
        **kwargs: Any,
    ) -> Node:
        """Makes a detached node that reads and writes values through this document's types.

        Takes the same arguments as `Node`. The node isn't added to the document.
        """
    def copy(self) -> Document:
        """Returns a copy that shares nothing with this document."""
    def iter(self, key: _Key | None = None) -> Iterator[Node]:
        """Iterates over every node in the document, at any depth, depth first in file order.

        Editing the document while iterating is safe: the nodes are found up front.

        Args:
            key: Only nodes that match: a name, or a `(type, name)` tuple. None means every node.
        """
    @overload
    def get(self, key: _Key) -> Node | None:
        """Returns the first top-level node that matches a key, or `default`.

        Args:
            key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
            default: What to return if no node matches.
        """
    @overload
    def get(self, key: _Key, default: _T) -> Node | _T: ...
    def get_all(self, key: _Key) -> list[Node]:
        """Returns every top-level node that matches a key, in order.

        Args:
            key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
        """
    def fmt(self, formatter: Formatter | None = None) -> None:
        """Reformats the whole document in place, with kdl-rs's autoformatter.

        Args:
            formatter: The settings. None means `Formatter()`.
        """
    def __getitem__(self, key: _Key, /) -> Node:
        """Returns the first top-level node that matches a key.

        Raises:
            KeyError: No node matches.
        """
    def __contains__(self, key: _Key, /) -> bool: ...
    def __iter__(self) -> Iterator[Node]: ...
    def __len__(self) -> int: ...
    def __copy__(self) -> Document: ...
    def __deepcopy__(self, memo: object, /) -> Document: ...

@final
class Node:
    """One KDL node: a name, an optional type annotation, arguments, properties and children.

    A node read from a document is a live handle: edits through it change the document, and reading the same node
    twice gives the same object. `Node(...)` makes a detached node, which belongs to no document until you add it to
    one. A node can be in only one place at a time: take it out first (`pop`, `remove`, `del`, `detach()`), or add a
    `copy()`.

    Args:
        name: The node's name.
        *args: The arguments.
        type: The type annotation, if any.
        props: Properties, for keys that can't be keyword arguments, such as `type` or `children`.
        children: Detached nodes to put in a children block. None means no block, and `[]` an empty one.
        **kwargs: More properties.

    Raises:
        TypeError: A value can't be converted to KDL.
        ValueError: A property is given twice, or one of `children` is already in a document.
    """

    def __new__(
        cls,
        name: str,
        *args: Any,
        type: str | None = None,
        props: dict[str, Any] | None = None,
        children: Iterable[Node] | None = None,
        **kwargs: Any,
    ) -> Self: ...
    @property
    def name(self) -> str:
        """The node's name."""
    @name.setter
    def name(self, value: str) -> None: ...
    @property
    def type(self) -> str | None:
        """The node's type annotation, or None. Setting None removes it."""
    @type.setter
    def type(self, value: str | None) -> None: ...
    @property
    def args(self) -> Args:
        """The arguments, as a live list of values.

        Assigning a list replaces them all.
        """
    @args.setter
    def args(self, value: Iterable[Any]) -> None: ...
    @property
    def props(self) -> Props:
        """The properties, as a live dict of values.

        Assigning a mapping replaces them all. Keys the node already had keep their place in the text.
        """
    @props.setter
    def props(self, value: _SupportsKeysAndGetItem | Iterable[tuple[str, Any]]) -> None: ...
    @property
    def entries(self) -> Entries:
        """The arguments and properties as `Entry` objects, as a live, read-only list in the order of the text."""
    @property
    def children(self) -> Children | None:
        """The child nodes as a live list, or None if the node has no children block.

        Assigning a list of detached nodes replaces them all, and gives the node a block if it had none: `[]` makes
        an empty `{}` block. Assigning None removes the block.
        """
    @children.setter
    def children(self, value: Iterable[Node] | None) -> None: ...
    @property
    def parent(self) -> Node | None:
        """The node this one is a child of. None for a top-level node, and for a detached node."""
    def detach(self) -> Node:
        """Takes the node out of its document and returns it.

        Afterwards the node is detached, ready to be added somewhere else. A detached node stays as it is.
        """
    def copy(self) -> Node:
        """Returns a detached copy of the node and everything in it."""
    def iter(self, key: _Key | None = None) -> Iterator[Node]:
        """Iterates over every node below this one, at any depth, depth first in file order.

        The node itself isn't included. Editing while iterating is safe: the nodes are found up front.

        Args:
            key: Only nodes that match: a name, or a `(type, name)` tuple. None means every node.
        """
    @overload
    def get(self, key: _Key) -> Node | None:
        """Returns the first child that matches a key, or `default`.

        Args:
            key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
            default: What to return if no child matches.
        """
    @overload
    def get(self, key: _Key, default: _T) -> Node | _T: ...
    def get_all(self, key: _Key) -> list[Node]:
        """Returns every child that matches a key, in order.

        Args:
            key: A name, or a `(type, name)` tuple. A type of None matches nodes without a type.
        """
    def __getitem__(self, key: _Key, /) -> Node:
        """Returns the first child that matches a key.

        Raises:
            KeyError: No child matches.
        """
    def __contains__(self, key: _Key, /) -> bool: ...
    def __iter__(self) -> Iterator[Node]: ...
    def __eq__(self, other: object, /) -> bool: ...
    def __hash__(self) -> int: ...
    def __copy__(self) -> Node: ...
    def __deepcopy__(self, memo: object, /) -> Node: ...

@final
class Entry:
    """One argument or property of a node, as a live handle.

    `name, value = entry` unpacks it, and `str(entry)` is its KDL text. Get one from `node.entries`,
    `node.args.entry(i)` or `node.props.entry(key)`. The handle keeps working while its node moves, and stops
    working once the entry is removed.
    """

    @property
    def name(self) -> str | None:
        """The property's key, or None for an argument.

        A property can be renamed, but an argument can't be given a name, nor a property lose its own.
        """
    @name.setter
    def name(self, value: str) -> None: ...
    @property
    def value(self) -> Any:
        """The value, converted by its type annotation.

        Writing a value keeps the annotation, and the value has to fit it: be of the annotation's class, be a plain
        value it reads (`"2029-01-01"` over `(date)`), or be a number it converts without loss (`7.0` over `(u8)`). To
        change the annotation, set `type` first. An entry without one gets one picked from the value's class, and an
        annotation rkdl doesn't know (not reserved, no custom Type) isn't checked.

        Raises:
            ValueError: The annotation is one the value can't be read as, or the new value doesn't fit it, like 300,
                3.5 or "weekly" written over `(u8)`. The document doesn't change.
            TypeError: The new value can't be converted to KDL.
        """
    @value.setter
    def value(self, value: object) -> None: ...
    @property
    def type(self) -> str | None:
        """The type annotation, or None. Setting None removes it.

        Setting it doesn't convert the value, so a type the value can't be read as raises `ValueError`.
        """
    @type.setter
    def type(self, value: str | None) -> None: ...
    def __iter__(self) -> Iterator[Any]: ...
    def __eq__(self, other: object, /) -> bool: ...
    def __hash__(self) -> int: ...

@final
class Args(MutableSequence[Any]):
    """A node's arguments, as a live list of values.

    It works like a list, and every edit changes the node. Arguments keep their place among the properties. A value
    written over an argument keeps its annotation and has to fit it, as with `Entry.value`.
    """

    def entry(self, index: SupportsIndex) -> Entry:
        """Returns the `Entry` for an argument.

        Raises:
            IndexError: There's no argument at `index`.
        """
    def insert(self, index: SupportsIndex, value: Any) -> None:
        """Inserts an argument before the one at `index`."""
    def append(self, value: Any) -> None:
        """Adds an argument after the last one."""
    def extend(self, iterable: Iterable[Any]) -> None:
        """Adds arguments after the last one. If any value can't be converted, none are added."""
    def pop(self, index: SupportsIndex = -1) -> Any:
        """Removes an argument and returns its value."""
    def remove(self, value: Any) -> None:
        """Removes the first argument equal to `value`.

        Raises:
            ValueError: No argument is equal to `value`.
        """
    def clear(self) -> None:
        """Removes every argument."""
    def reverse(self) -> None:
        """Reverses the arguments in place. Each slot in the text keeps its whitespace; the values move."""
    def index(self, value: Any, start: SupportsIndex = 0, stop: SupportsIndex = ...) -> int:
        """Returns the position of the first argument equal to `value`.

        Raises:
            ValueError: No argument is equal to `value`.
        """
    def count(self, value: Any) -> int:
        """Returns how many arguments are equal to `value`."""
    @overload
    def __getitem__(self, index: SupportsIndex, /) -> Any: ...
    @overload
    def __getitem__(self, index: slice, /) -> list[Any]: ...
    @overload
    def __setitem__(self, index: SupportsIndex, value: Any, /) -> None: ...
    @overload
    def __setitem__(self, index: slice, value: Iterable[Any], /) -> None: ...
    def __delitem__(self, index: SupportsIndex | slice, /) -> None: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Any]: ...
    def __reversed__(self) -> Iterator[Any]: ...
    def __contains__(self, value: object, /) -> bool: ...
    def __iadd__(self, iterable: Iterable[Any], /) -> Self: ...
    def __eq__(self, other: object, /) -> bool: ...
    __hash__: ClassVar[None]  # type: ignore[assignment]

@final
class Props(MutableMapping[str, Any]):
    """A node's properties, as a live dict of values.

    It works like a dict, and every edit changes the node. A new key goes after the last property. If a key appears
    more than once in the text, the last one counts, as in KDL. A value written over a property keeps its annotation
    and has to fit it, as with `Entry.value`.
    """

    def entry(self, key: str) -> Entry:
        """Returns the `Entry` for a property.

        Raises:
            KeyError: There's no property `key`.
        """
    def get(self, key: str, default: Any = None) -> Any:
        """Returns the value of property `key`, or `default`."""
    def pop(self, key: str, *default: Any) -> Any:
        """Removes property `key` and returns its value, or `default` if there's no such property.

        Raises:
            KeyError: There's no property `key`, and no default.
        """
    def popitem(self) -> tuple[str, Any]:
        """Removes the last property and returns its key and value.

        Raises:
            KeyError: There are no properties.
        """
    def setdefault(self, key: str, default: Any = None) -> Any:
        """Returns the value of property `key`, first adding it with `default` if there's no such property."""
    @overload
    def update(self, other: _SupportsKeysAndGetItem, /, **kwargs: Any) -> None:
        """Sets properties from a mapping or an iterable of pairs, and from keyword arguments.

        If any value can't be converted, nothing changes.
        """
    @overload
    def update(self, other: Iterable[tuple[str, Any]], /, **kwargs: Any) -> None: ...
    @overload
    def update(self, /, **kwargs: Any) -> None: ...
    def clear(self) -> None:
        """Removes every property."""
    def copy(self) -> dict[str, Any]:
        """Returns the properties as a plain dict."""
    def keys(self) -> KeysView[str]:
        """Returns a live view of the keys."""
    def values(self) -> ValuesView[Any]:
        """Returns a live view of the values."""
    def items(self) -> ItemsView[str, Any]:
        """Returns a live view of the `(key, value)` pairs."""
    def __getitem__(self, key: str, /) -> Any: ...
    def __setitem__(self, key: str, value: Any, /) -> None: ...
    def __delitem__(self, key: str, /) -> None: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[str]: ...
    def __contains__(self, key: object, /) -> bool: ...
    def __or__(self, other: Mapping[str, _T], /) -> dict[str, Any | _T]: ...
    def __ror__(self, other: Mapping[str, _T], /) -> dict[str, Any | _T]: ...
    # `props |= x` keeps the live view, while `props | x` makes a plain dict.
    def __ior__(self, other: _SupportsKeysAndGetItem | Iterable[tuple[str, Any]], /) -> Self: ...  # type: ignore[misc]
    def __eq__(self, other: object, /) -> bool: ...
    __hash__: ClassVar[None]  # type: ignore[assignment]

@final
class Entries(Sequence[Entry]):
    """A node's arguments and properties as `Entry` objects, in the order of the text.

    A live, read-only list.
    """

    def index(self, entry: Entry, start: SupportsIndex = 0, stop: SupportsIndex = ...) -> int:
        """Returns the position of an entry.

        Raises:
            ValueError: The entry isn't one of these.
        """
    def count(self, entry: object) -> int:
        """Returns 1 if the entry is one of these, else 0."""
    @overload
    def __getitem__(self, index: SupportsIndex, /) -> Entry: ...
    @overload
    def __getitem__(self, index: slice, /) -> list[Entry]: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Entry]: ...
    def __reversed__(self) -> Iterator[Entry]: ...
    def __contains__(self, entry: object, /) -> bool: ...
    def __eq__(self, other: object, /) -> bool: ...
    __hash__: ClassVar[None]  # type: ignore[assignment]

@final
class Children(MutableSequence[Node]):
    """A document's top-level nodes or a node's children, as a live list.

    It works like a list, and every edit changes the document. Only detached nodes can go in. A node taken out
    (`pop`, `remove`, `del`, `clear`) becomes detached. An edit that would add several nodes adds all of them or
    none.
    """

    def insert(self, index: SupportsIndex, node: Node) -> None:
        """Inserts a detached node before the one at `index`.

        Raises:
            ValueError: The node is already in a document.
        """
    def append(self, node: Node) -> None:
        """Adds a detached node at the end.

        Raises:
            ValueError: The node is already in a document.
        """
    def extend(self, nodes: Iterable[Node]) -> None:
        """Adds detached nodes at the end. If any of them can't go in, none do.

        Raises:
            ValueError: A node is already in a document.
        """
    def pop(self, index: SupportsIndex = -1) -> Node:
        """Takes a node out and returns it, detached."""
    def remove(self, node: Node) -> None:
        """Takes a node out, leaving it detached.

        Raises:
            ValueError: The node isn't one of these.
        """
    def clear(self) -> None:
        """Takes every node out."""
    def reverse(self) -> None:
        """Reverses the nodes in place."""
    def index(self, node: Node, start: SupportsIndex = 0, stop: SupportsIndex = ...) -> int:
        """Returns the position of a node.

        Raises:
            ValueError: The node isn't one of these.
        """
    def count(self, node: object) -> int:
        """Returns 1 if the node is one of these, else 0."""
    @overload
    def __getitem__(self, index: SupportsIndex, /) -> Node: ...
    @overload
    def __getitem__(self, index: slice, /) -> list[Node]: ...
    @overload
    def __setitem__(self, index: SupportsIndex, value: Node, /) -> None: ...
    @overload
    def __setitem__(self, index: slice, value: Iterable[Node], /) -> None: ...
    def __delitem__(self, index: SupportsIndex | slice, /) -> None: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Node]: ...
    def __reversed__(self) -> Iterator[Node]: ...
    def __contains__(self, node: object, /) -> bool: ...
    def __iadd__(self, nodes: Iterable[Node], /) -> Self: ...
    def __eq__(self, other: object, /) -> bool: ...
    __hash__: ClassVar[None]  # type: ignore[assignment]
