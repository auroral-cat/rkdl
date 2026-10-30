<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/auroral-cat/rkdl/main/.github/assets/banner-dark.png">
    <img alt="rkdl: KDL for Python, built on kdl-rs" src="https://raw.githubusercontent.com/auroral-cat/rkdl/main/.github/assets/banner-light.png" width="100%">
  </picture>
</p>

rkdl reads and edits [KDL](https://kdl.dev) v2 documents from Python, using [kdl-rs](https://github.com/kdl-org/kdl-rs)
underneath. When you edit a file and save it, the parts you didn't touch keep their comments, hex numbers and quoting.
Only the whitespace right next to an edit can move.

A KDL document is a list of nodes. Each node has a name, then arguments (plain values like `"web"` or `4`) and
properties (`key=value`), and optionally a block of child nodes in `{ }`. A node or a value can carry a type
annotation in parentheses, like `(date)"2026-10-30"`.

## Install

rkdl isn't on PyPI yet, so install it from git:

```sh
uv add "rkdl @ git+https://github.com/auroral-cat/rkdl@v0.1.0"
# or
pip install "rkdl @ git+https://github.com/auroral-cat/rkdl@v0.1.0"
```

This builds it from source, so you need Rust. The build uses the toolchain version rkdl pins in `rust-toolchain.toml`;
with [rustup](https://rustup.rs) that version is fetched automatically. rkdl needs Python 3.11 or newer.

## Editing a file

Say `server.kdl` looks like this:

```kdl
// Where the app listens
server "web" port=0x1F90 {
    host "localhost"
    workers 4
}
logging level="info"
```

Load it and read some values:

```python
import rkdl

doc = rkdl.load("server.kdl")

server = doc["server"]
print(server.args[0])  # web
print(server.props["port"])  # 8080
print(server["workers"].args[0])  # 4
```

Change two values, add a node, remove another, and save:

```python
server.props["port"] = 9090
server["workers"].args[0] = 8
timeout = rkdl.Node("timeout", 30)
server.children.append(timeout)
logging = doc["logging"]
logging.detach()

rkdl.dump(doc, "server.kdl")
```

`server.kdl` is now:

```kdl
// Where the app listens
server "web" port=0x2382 {
    host "localhost"
    workers 8
    timeout 30
}
```

The comment stayed, the port is still hex, and `timeout` is indented like its siblings. `load` and `dump` take a path
or an open file, text or binary. `rkdl.parse(text)` and `str(doc)` do the same with strings.

## Finding nodes

`doc["name"]` gives the first node with that name, and raises `KeyError` if there's none. A `(type, name)` pair also
matches the type annotation. `get(key, default)` returns the default instead of raising, `get_all(key)` returns every
match, and `key in doc` checks whether there is one. Nodes work the same way for their children.

```python
text = """\
package "rkdl"
(dev)dependency "pytest"
dependency "hypothesis"
scripts {
    test "pytest"
}
"""
doc = rkdl.parse(text)

first_dependency = doc["dependency"]  # matches any type: this is (dev)dependency
dev_dependency = doc[("dev", "dependency")]
all_dependencies = doc.get_all("dependency")
license_node = doc.get("license")  # None
has_scripts = "scripts" in doc  # True
```

Iterating over a document or a node gives its children. `iter()` walks all the descendants depth first, and
`iter(name)` keeps only the ones with that name:

```python
top_level_names = [node.name for node in doc]
# ['package', 'dependency', 'dependency', 'scripts']
every_name = [node.name for node in doc.iter()]
# ['package', 'dependency', 'dependency', 'scripts', 'test']
test_parents = [node.parent.name for node in doc.iter("test")]
# ['scripts']
```

A node's `name` and `type` can be set too.

## Arguments and properties

`node.args` is a live list of the node's arguments, and `node.props` a live dict of its properties, both as plain
Python values. Changing them changes the document.

```python
doc = rkdl.parse('dependency (dev)"pytest" "hypothesis" version=">=8" optional=#true\n')
dependency = doc["dependency"]

print(dependency.args)  # Args(['pytest', 'hypothesis'])
print(dependency.props)  # Props({'version': '>=8', 'optional': True})

dependency.args.append("ruff")
dependency.props["version"] = ">=9"
del dependency.props["optional"]
print(doc)
# dependency (dev)"pytest" "hypothesis" ruff version=">=9"
```

A new string is written without quotes when KDL allows it, like `ruff` here.

`node.entries` lists every argument and property in file order, as `Entry` objects. An entry has a `name` (`None`
for an argument), a `type` and a `value`. `args.entry(index)` and `props.entry(key)` get a single one. You can set an
entry's `type` and `value`, and the `name` of a property; an argument can't become a property or the other way round.

```python
for entry in dependency.entries:
    print(entry.name, entry.type, entry.value)
# None dev pytest
# None None hypothesis
# None None ruff
# version None >=9

version_entry = dependency.props.entry("version")
version_entry.name = "min-version"
print(doc)
# dependency (dev)"pytest" "hypothesis" ruff min-version=">=9"
```

## Children and moving nodes

`node.children` is a live list of nodes. It's `None` when the node has no `{ }` block. Assigning `[]` adds an empty
block and assigning `None` removes it.

A node can only be in one place at a time. To move it, take it out first with `node.detach()`, or with
`parent.children.pop(index)`, `parent.children.remove(node)` or `del parent.children[index]`, then insert it
somewhere else. The comments on the lines above it and at the end of its line go with it. To put the same node in
twice, insert `node.copy()`.

```python
text = """\
todo {
    task "write the README"
    task "tag v0.1.0"
}
done
"""
doc = rkdl.parse(text)
todo = doc["todo"]
done = doc["done"]

done.children = []
finished_task = todo.children[0]
finished_task.detach()
done.children.append(finished_task)
new_task = rkdl.Node("task", "push to GitHub")
todo.children.insert(0, new_task)
print(doc)
```

```kdl
todo {
    task "push to GitHub"
    task "tag v0.1.0"
}
done {
    task "write the README"
}
```

Appending `finished_task` without detaching it raises `ValueError`. `rkdl.Node(name, *args, type=None, props=None,
children=None, **props)` builds a new node that isn't in any document yet; `props={...}` takes keys that
aren't Python identifiers.

## Type annotations

The spec's reserved type annotations come back as Python types:

| Annotation | Python type |
| --- | --- |
| `i8` `i16` `i32` `i64` `i128` `isize` `u8` `u16` `u32` `u64` `u128` `usize` | `int`, range checked |
| `f32` `f64` | `float` |
| `decimal64` `decimal128` `decimal` | `decimal.Decimal` |
| `date-time` | `datetime.datetime` |
| `date` | `datetime.date` |
| `time` | `datetime.time` |
| `ipv4` | `ipaddress.IPv4Address` |
| `ipv6` | `ipaddress.IPv6Address` |
| `uuid` | `uuid.UUID` |
| `regex` | `re.Pattern` |
| `base64` | `bytes` |
| `duration` `currency` `country-2` `country-3` `country-subdivision` `email` `idn-email` `hostname` `idn-hostname` `url` `url-reference` `irl` `irl-reference` `url-template` `base85` | `str` |

A value that doesn't fit its annotation, like `(date)"2026-13-01"`, still parses. Reading it raises `ValueError`.

When you write a value to an entry that has an annotation, the annotation stays if the value reads as that type:
`"2029-01-01"` written over a `(date)` stays `(date)`. A number out of range for a numeric type raises `ValueError`.
Anything else gets an annotation picked from its Python type, or none for a plain value. To annotate a value
yourself, set its entry's `type`.

```python
import datetime

doc = rkdl.parse('backup started=(date-time)"2026-10-30T09:00:00Z" keep=(u8)7 retries=3\n')
backup = doc["backup"]

started = backup.props["started"]
# datetime.datetime(2026, 10, 30, 9, 0, tzinfo=datetime.timezone.utc)

backup.props["keep"] = 14
backup.props["finished"] = datetime.date(2026, 10, 31)
retries_entry = backup.props.entry("retries")
retries_entry.type = "u8"
print(doc)
# backup started=(date-time)"2026-10-30T09:00:00Z" keep=(u8)14 retries=(u8)3 finished=(date)"2026-10-31"

backup.props["keep"] = 300
# ValueError: 300 doesn't fit type u8: out of range 0..=255
```

For your own annotations, give a `Parser` some `Type`s. `decode` turns the plain value into your object and `encode`
turns it back:

```python
import dataclasses


@dataclasses.dataclass
class Celsius:
    degrees: float


def celsius_to_number(temperature):
    return temperature.degrees


celsius_type = rkdl.Type(
    "celsius",
    Celsius,
    decode=Celsius,
    encode=celsius_to_number,
)
parser = rkdl.Parser(types=[celsius_type])

doc = parser.parse("oven (celsius)180.0\n")
oven = doc["oven"]
print(oven.args[0])  # Celsius(degrees=180.0)

oven.args[0] = Celsius(200.0)
fridge = doc.node("fridge", Celsius(4.0))
doc.children.append(fridge)
print(doc)
# oven (celsius)200.0
# fridge (celsius)4.0
```

`rkdl.Node` only knows the built-in types, so build nodes that hold your own values with `doc.node(...)` or
`parser.node(...)`.

## Formatting

`doc.fmt()` reformats the whole document with kdl-rs's formatter. It changes more than whitespace: here the quotes on
`"web"` go too.

```python
messy_text = """\
server   "web"    port=8080 {
      // the public name
  host "example.com"
        }
"""
doc = rkdl.parse(messy_text)
doc.fmt()
print(doc)
```

```kdl
server web port=8080 {
    // the public name
    host example.com
}
```

`rkdl.Formatter` takes `indent`, `indent_level`, `no_comments` and `entry_autoformat_keep`. Pass one to `fmt`, or to
`Parser(formatter=...)` to format every document as it's parsed:

```python
tab_formatter = rkdl.Formatter(indent="\t", no_comments=True)
doc.fmt(tab_formatter)
```

## Parse errors

Text that isn't valid KDL raises `rkdl.ParseError`, a subclass of `ValueError`. Its message shows where each problem
is. `line` and `column` point at the first one, counting characters from 1. `diagnostics` has a `Diagnostic` for each
problem, with `message`, `label`, `help`, `line`, `column`, `offset` and `length`. When `load` read the text from a
file, `path` names it and so does the message; otherwise `path` is `None`.

```python
try:
    rkdl.parse("port 0x1G\n")
except rkdl.ParseError as error:
    print(error.line, error.column)  # 1 6
    print(error)
```

```text
  × Failed to parse KDL document

Error: 
  × Expected hexadecimal number
   ╭────
 1 │ port 0x1G
   ·      ──┬─
   ·        ╰── not hexadecimal number
   ╰────
  help: A valid value was partially parsed, but was not followed by a value terminator. Did you want a space here?
```

## Stability

rkdl is 0.x, so anything may change. Some things are likely to change, in particular: the text of `ParseError` and its
diagnostics, what `fmt()` produces and which options `Formatter` takes, and where whitespace lands next to an edit.

## Known limits

- KDL v2 only.
- Documents nested deeper than 1000 levels are refused with a `ParseError`, and an edit that would nest deeper raises
  `ValueError`.
- A few kdl-rs bugs remain; see the tests marked `xfail` in [`tests/`](https://github.com/auroral-cat/rkdl/tree/main/tests).

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
