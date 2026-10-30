<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/auroral-cat/rkdl/main/.github/assets/banner-dark.png">
    <img alt="rkdl: KDL for Python, built on kdl-rs" src="https://raw.githubusercontent.com/auroral-cat/rkdl/main/.github/assets/banner-light.png" width="100%">
  </picture>
</p>

rkdl is a Python library for reading and editing [KDL](https://kdl.dev) v2 files, built on
[kdl-rs](https://github.com/kdl-org/kdl-rs), a Rust crate. When you save a file you edited, everything you didn't
change is written back as it was, comments included.

Say `server.kdl` looks like this:

```kdl
// Where the app listens
server "web" port=0x1F90 {
    host "localhost"
    workers 4
}
logging level="info"
```

Load it, read a few values, change some, add a node, remove one, and save:

```python
import rkdl

doc = rkdl.load("server.kdl")

server = doc["server"]
print(server.args[0])  # web
print(server.props["port"])  # 8080, written 0x1F90 in the file
print(server["workers"].args[0])  # 4

# Values are plain Python values. 9090 replaces a value written in hex, so it's written in hex too.
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

The comment is still there, and `timeout` got the same indentation as its siblings.

In KDL terms, `server`, `host`, `workers` and `logging` are nodes. On the `server` line, `server` is the node's name,
`"web"` is an argument, `port=0x1F90` is a property, and the nodes inside `{ }` are its children. `doc["server"]`
gives the first top-level node named `server`, and `server["workers"]` the first child named `workers`. `args` works
like a Python list and `props` like a dict, and changing them changes the document.

`load` and `dump` take a path or an open file. To work with strings, use `rkdl.parse(text)` and `str(doc)`. The rest
of this README works with strings.

## Install

```sh
uv add rkdl
# or
pip install rkdl
```

rkdl needs Python 3.11 or newer. The source is on GitHub at
[auroral-cat/rkdl](https://github.com/auroral-cat/rkdl).

## Contents

- [Finding nodes, and nodes that are missing or repeated](#finding-nodes-and-nodes-that-are-missing-or-repeated)
- [When the file isn't valid KDL](#when-the-file-isnt-valid-kdl)
- [When a setting in the file is wrong](#when-a-setting-in-the-file-is-wrong)
- [KDL v1 files](#kdl-v1-files)
- [Adding a node](#adding-a-node)
- [Removing and moving nodes](#removing-and-moving-nodes)
- [Renaming a node or changing its annotation](#renaming-a-node-or-changing-its-annotation)
- [Comments](#comments)
- [Arguments and properties](#arguments-and-properties)
- [Annotations on arguments and properties](#annotations-on-arguments-and-properties)
- [Which Python classes values come back as](#which-python-classes-values-come-back-as)
- [Writing values](#writing-values)
- [Your own annotations](#your-own-annotations)
- [Reformatting the whole file](#reformatting-the-whole-file)
- [Details: held nodes, comparing, copies, slashdash, encoding, saving](#details-held-nodes-comparing-copies-slashdash-encoding-saving)
- [Reference](#reference)
- [Stability](#stability)
- [Known limits](#known-limits)
- [License](#license)

## Finding nodes, and nodes that are missing or repeated

A hand-edited file may not have the node you look for, or may have it twice. Here's a file with both cases. It also
has a node with an annotation, the `(dev)` in parentheses before the name:

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

first_dependency = doc["dependency"]  # (dev)dependency, the first one in the file
dev_dependency = doc[("dev", "dependency")]  # the first dependency annotated (dev)
all_dependencies = doc.get_all("dependency")  # both, in file order; an empty list if there's none
license_node = doc.get("license")  # None
has_scripts = "scripts" in doc  # True
```

- `doc[name]` raises `KeyError` when no node has that name. `doc.get(name)` returns `None` instead, or the default
  you pass as a second argument, and `name in doc` tells you whether there is one.
- When the name appears more than once, `doc[name]` and `get` give the first. KDL doesn't say which of two nodes with
  the same name counts; that's up to your file format. `get_all(name)` gives all of them in file order, so you can
  pick, or report the duplicate to your user with the line each one is on, from `node.line` (counting from 1).
- A name matches nodes with any annotation. To match the annotation too, look up an `(annotation, name)` pair.
  `(None, name)` matches only nodes without one.

A node's annotation is in its `type` attribute, since KDL's spec calls annotations types: `first_dependency.type` is
`"dev"`, without the parentheses, and it's `None` on a node without one.

Nodes have the same lookups for their children. A node without a `{ }` block has no children: its `children` is
`None`, `node[name]` raises `KeyError`, `get` returns `None`, and iterating over it gives nothing.

Iterating over a document or a node gives its children. `doc.iter()` (and `node.iter()`) goes through every node below
it, depth first in file order, and `doc.iter(name)` keeps only the ones with that name:

```python
top_level_names = [node.name for node in doc]
# ['package', 'dependency', 'dependency', 'scripts']
every_name = [node.name for node in doc.iter()]
# ['package', 'dependency', 'dependency', 'scripts', 'test']
test_parents = [node.parent.name for node in doc.iter("test")]
# ['scripts']
```

`node.parent` is `None` for a top-level node.

## When the file isn't valid KDL

If someone broke the file, `load` and `parse` raise `rkdl.ParseError`, a subclass of `ValueError`. Say
`settings.kdl` has this:

```kdl
server "web" {
    port 8080
    tls true
    log-file null
}
```

```python
try:
    settings = rkdl.load("settings.kdl")
except rkdl.ParseError as error:
    print(error.path, error.line, error.column)  # settings.kdl 3 9
    print(error)
```

```text
settings.kdl:3:9: Failed to parse KDL document

  × Expected identifier string
   ╭─[settings.kdl:3:9]
 2 │     port 8080
 3 │     tls true
   ·         ──┬─
   ·           ╰── not identifier string
 4 │     log-file null
   ╰────
  help: this looks like KDL v1: in KDL v2 it's #true

  × Expected identifier string
   ╭─[settings.kdl:4:14]
 3 │     tls true
 4 │     log-file null
   ·              ──┬─
   ·                ╰── not identifier string
 5 │ }
   ╰────
  help: this looks like KDL v1: in KDL v2 it's #null
```

The parser keeps going after a problem, so one `ParseError` can list several, each marked in the text with a note
under it. The message is written to be shown to your user as it is. `line` and `column` give the first problem's
position, counting characters from 1, and `path` names the file, or is `None` when the text didn't come from a named
file.

A file that isn't UTF-8 makes `load` raise `UnicodeDecodeError` instead. That's a `ValueError` too, so
`except ValueError` catches both.

To report problems your own way, such as in an editor, `error.diagnostics` has one `Diagnostic` per problem, with
`message`, `label` (the note under the marked text), `help`, `line`, `column`, `offset` (characters from the start
of the text, counting from 0) and `length` (in characters).

## When a setting in the file is wrong

A file can be valid KDL and still be wrong for your tool, such as a port given as a string. Nodes and their arguments
and properties know where they were in the file, so your message can point at the spot the way a `ParseError` does.
Say `app.kdl` has this:

```kdl
server "web" {
    port "8080"
    retries (u8)300
}
```

```python
config = rkdl.load("app.kdl")
server = config["server"]
port_entry = server["port"].args.entry(0)  # the argument as an Entry, with its own position
if not isinstance(port_entry.value, int):
    print(f"{config.path}:{port_entry.line}:{port_entry.column}: port should be a number, not {port_entry}")
# app.kdl:2:10: port should be a number, not "8080"

if server.get("host") is None:  # a missing node: point at its parent
    print(f"{config.path}:{server.line}:{server.column}: server needs a host")
# app.kdl:1:1: server needs a host

try:
    retries = server["retries"].args[0]  # 300 doesn't fit (u8), so reading it raises
except rkdl.ReadError as error:
    print(error.path, error.line, error.column, error.key, error.text)
    print(error)
# app.kdl 3 13 0 (u8)300
# app.kdl:3:13: retries arg 0: 300 doesn't fit (u8): out of range 0..=255
```

- An `Entry` is one argument or property, with its value, annotation and position. "Annotations on arguments and
  properties" covers it in full.
- `node.get(name)` returns `None` for a missing node, as under "Finding nodes". `server["host"]` would raise a
  `KeyError` that names only `host`, with no position, so check with `get` and report at the parent.
- `node.line` and `node.column` give where a node starts, counted as a `ParseError` counts them: from 1, in
  characters. An argument or property has its own: `port_entry.line` here for the first argument of `port`, and
  `server.props.entry(key).line` for a property of `server`, each with a `column` too. They describe the text as it
  was parsed: edits and moves within the document don't change them. A node, argument or property that didn't come
  from parsing (built with `rkdl.Node`, added after parsing, a copy) has `None`, and so does a node moved into
  another document.
- `str(entry)` gives an argument or property as the file has it, annotation and quotes included: `"8080"` above,
  `(u8)300` for the `retries` argument.
- `config.path` is the file `load` read, and `None` for a document from `parse`.
- Reading a value that doesn't fit its annotation raises `rkdl.ReadError`, a subclass of `ValueError`. Only annotations
  rkdl knows are checked: the ones listed under "Which Python classes values come back as", and your own (see "Your own
  annotations"). `path`, `line` and `column` are the entry's, as above, `key` is the argument's index in `args` or the
  property's key, and `text` is the argument or property as the file has it, like `str(entry)`. The message names the
  node and the argument or property, not the nodes around it, and it's written to be shown to your users as it is. For a
  document from `parse`, which has no path, it starts with the line and column alone, as in `3:13: retries arg 0: ...`.

To report every value in a file that doesn't fit, rather than stopping at the first, go through every entry and read
its value. Going through `node.entries` doesn't raise, since it reads only names and annotations; reading
`entry.value` does. Your own checks, like the port one above, can go in the same loop:

```python
problems = []
for node in config.iter():
    for entry in node.entries:
        try:
            entry.value  # raises if the value doesn't fit its annotation
        except rkdl.ReadError as error:
            problems.append(str(error))
print(problems)
# ["app.kdl:3:13: retries arg 0: 300 doesn't fit (u8): out of range 0..=255"]
```

## KDL v1 files

rkdl reads KDL v2 only, and doesn't convert v1 files. Most v1 files fail to parse on bare `true`, `false` or `null`,
or on raw strings written `r"..."`. Your user then sees a `ParseError` like the one for `settings.kdl` above: where
the problem looks like KDL v1, its help line says so and gives the v2 form, such as `#true` or `#"..."#`.

## Adding a node

`rkdl.Node(name, *args, **props)` builds a node that isn't in any document yet. Keyword arguments become properties.
For a property name that isn't a Python identifier, like `log-level`, pass a dict as `props=`. `children=` takes a
list of nodes to put in a `{ }` block, and `type=` gives the node an annotation, as in `rkdl.Node("dependency",
"pytest", type="dev")` for `(dev)dependency "pytest"`; a property named `type` goes in `props=`.

To put the node at a given spot, insert it into `children` at an index. `doc.children` holds the top-level nodes, and
`children.index(node)` finds where a sibling is. On a node without a `{ }` block, `children` is `None` and there's
nothing to insert into: assign a list to give it a block, as in `node.children = [child]` (`[]` makes an empty one).

```python
text = """\
server "web" {
  host "localhost"  // dev only
  workers 4
}
"""
doc = rkdl.parse(text)
server = doc["server"]

if "tls" not in server:
    tls = rkdl.Node("tls", "/etc/web.pem", verify=True)
    host_position = server.children.index(server["host"])
    server.children.insert(host_position + 1, tls)

upstream = rkdl.Node("upstream", "127.0.0.1:9000")
proxy = rkdl.Node("proxy", props={"log-level": "warn"}, children=[upstream])
doc.children.insert(0, proxy)
print(doc)
```

```kdl
proxy log-level=warn {
  upstream "127.0.0.1:9000"
}
server "web" {
  host "localhost"  // dev only
  tls "/etc/web.pem" verify=#true
  workers 4
}
```

A new node goes on its own line and takes the file's indentation (two spaces here) and line endings. The lines around
it don't change. Its values are written the plain v2 way: `True` becomes `#true`, and a string gets quotes only when it
needs them (`warn` doesn't, while `/etc/web.pem` and `127.0.0.1:9000` do). A value whose class has an annotation of
its own gets it, so a `datetime.date` is written `(date)"2026-10-31"`; "Writing values" has the rules.

## Removing and moving nodes

`node.detach()` takes a node out of its document and returns it. `pop`, `remove` and `del` on its parent's
`children` do the same. The comments that belong to the node leave with it; "Comments" below says which those are.

A node can be in one place at a time. To move one, take it out, then insert it somewhere else:

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

done.children = []  # done had no { } block; an empty list gives it one
finished_task = todo.children[0]
finished_task.detach()
done.children.append(finished_task)
print(doc)
```

```kdl
todo {
    task "tag v0.1.0"
}
done {
    task "write the README"
}
```

Appending `finished_task` without detaching it first raises `ValueError`. To put the same node in two places, insert
`node.copy()` for the second one: a new node with the same values, children and owned comments, sharing nothing with
the original.

## Renaming a node or changing its annotation

Assign to a node's `name` and `type`. Setting `type` to `None` removes the annotation. A node's annotation isn't
checked against anything, so any name works:

```python
doc = rkdl.parse('(dev)dependency "pytest"\n')
dependency = doc["dependency"]
dependency.name = "test-dependency"
dependency.type = None
print(doc)
# test-dependency "pytest"
```

## Comments

rkdl can't read, edit or add comments directly. It keeps them where they are, and a comment that belongs to a node
moves, copies or disappears with it.

A node owns the comment at the end of its line and the comments right above it, up to the nearest blank line. That
holds for the first node too: a header comment with no blank line under it belongs to the first node and leaves with
it, so give your header a blank line. A commented-out node (`/-`) on the line above a node belongs to that node in the
same way. Comments that belong to no node stay where they are:

- comments above a blank line, so a header comment set off by a blank line stays when you remove the first node, and
  so does a comment over a group of nodes;
- comments before a `}` or at the end of the file.

Inserting follows the same rule: `insert(0, node)` puts the node below a header comment set off by a blank line, not
above it. `fmt()` removes blank lines, which changes who owns what.

To add a node with a comment, parse a snippet that has one and move the node in:

```python
doc = rkdl.parse('server "web" {\n    workers 4\n}\n')
snippet = rkdl.parse("// Added by deploy.py\ntimeout 30 // seconds\n")
timeout = snippet["timeout"].detach()  # both comments belong to timeout, so they come along
doc["server"].children.append(timeout)
print(doc)
```

```kdl
server "web" {
    workers 4
    // Added by deploy.py
    timeout 30 // seconds
}
```

## Arguments and properties

`node.args` works like a list of the node's arguments, and `node.props` like a dict of its properties:

```python
doc = rkdl.parse('deploy "web" "api" region="eu-west" dry-run=#true\n')
deploy = doc["deploy"]

print(deploy.args)  # Args(['web', 'api'])
print(deploy.props)  # Props({'region': 'eu-west', 'dry-run': True})

deploy.args.append("worker")  # written without quotes, since it doesn't need them
deploy.props["region"] = "us-east"  # keeps its quotes: a replaced value keeps its form
del deploy.props["dry-run"]
print(doc)
# deploy "web" "api" worker region="us-east"
```

The new argument goes after the last argument, and a new property after the last property. If a file has the same
property twice on one node, the last one counts, as KDL says: reading and writing act on it, and `del` removes it, so
the earlier one counts after that.

## Annotations on arguments and properties

Values can have annotations too. `args` and `props` show only the values, so to see or change an annotation, use the
value's `Entry`. `node.entries` lists every argument and property in file order, each with a `name` (`None` for an
argument), a `type` (the annotation) and a `value`:

```python
doc = rkdl.parse('cache (path)"/var/cache" ttl=(seconds)300 size=512\n')
cache = doc["cache"]
print(cache.args[0])  # /var/cache, without its (path) annotation

for entry in cache.entries:
    print(entry.name, entry.type, entry.value)
# None path /var/cache
# ttl seconds 300
# size None 512
```

`args.entry(index)` and `props.entry(key)` get a single entry. You can set its `type` and `value`, and rename a
property through its `name`. An argument can't become a property, or the other way round: setting an argument's
`name`, or a property's `name` to `None`, raises `ValueError`.

```python
size_entry = cache.props.entry("size")
size_entry.type = "mb"
size_entry.name = "max-size"
print(doc)
# cache (path)"/var/cache" ttl=(seconds)300 max-size=(mb)512
```

When you set `type` to an annotation rkdl checks (one from the table in the next section, or your own), the value
already there has to fit it. If it doesn't, setting `type` raises a plain `ValueError`, not `rkdl.ReadError`, and
changes nothing. The message shows the value as the file has it:

```python
ttl_entry = cache.props.entry("ttl")
ttl_entry.type = "u8"
# ValueError: 300 doesn't fit (u8): out of range 0..=255
```

## Which Python classes values come back as

Plain KDL values come back as `str`, `int`, `float`, `bool` or `None`. The KDL spec reserves some annotation names,
and rkdl reads values with those annotations as these Python classes:

| Annotation | Python class |
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

Any other annotation, like `(path)` above, leaves the value plain. A value that doesn't fit its annotation still
parses, since a person may have typed it. Reading it raises `rkdl.ReadError`, as under "When a setting in the file is
wrong":

```python
doc = rkdl.parse('backup expires=(date)"2026-13-01"\n')
backup = doc["backup"]
expires = backup.props["expires"]
# rkdl.ReadError: 1:8: backup prop expires: "2026-13-01" doesn't fit (date): month must be in 1..12, not 13
```

## Writing values

You can write values of the classes that `args` and `props` read back: `str`, `int`, `float`, `bool`, `None`, the
classes in the table above, and in a document that knows your own `Type`s, their classes. A value of any other class
raises `TypeError`, whatever it's written over. A value written over another keeps the old one's form where the new
value allows it: hex stays hex, a string written quoted or raw stays that way, and one written bare stays bare unless
the new string needs quotes.

The annotation a written value gets:

- **Over a value with an annotation**, the annotation stays. If rkdl checks that annotation (one from the table, or
  one of your own `Type`s), the new value has to fit it, or the write raises `ValueError` and
  changes nothing. Any other annotation isn't checked.
- **Over a value without one, or as a new value**, it gets its Python class's own annotation: `(date)` for
  `datetime.date`, `(decimal)` for `decimal.Decimal`, `(base64)` for `bytes`, and so on through the table, and none
  for `str`, `int`, `float`, `bool` and `None`.
- **`rkdl.Value(value, type=...)`** sets the annotation outright, whatever was there. `type=None`, the default, means
  no annotation. The value has to fit `type` if rkdl checks it.

A write that doesn't fit raises a plain `ValueError`, not `rkdl.ReadError`, since the value came from your program and
not from the file. Its message shows the value as Python's `repr` does.

```python
import datetime

doc = rkdl.parse('backup expires=(date)"2027-01-01" keep=(u8)7 dir=(path)"/srv"\n')
backup = doc["backup"]

backup.props["keep"] = 14.0  # checked annotation, fits: written 14
backup.props["expires"] = "2028-06-30"  # checked annotation, fits
backup.props["dir"] = "/backup"  # annotation rkdl doesn't check
backup.props["finished"] = datetime.date(2026, 10, 31)  # new value: its class's annotation
print(doc)
# backup expires=(date)"2028-06-30" keep=(u8)14 dir=(path)"/backup" finished=(date)"2026-10-31"

backup.props["keep"] = 300  # checked annotation, doesn't fit
# ValueError: 300 doesn't fit (u8): out of range 0..=255
backup.props["keep"] = "weekly"  # checked annotation, doesn't fit
# ValueError: 'weekly' doesn't fit (u8): expected an integer
backup.props["expires"] = datetime.datetime(2029, 1, 1, 9, 0)  # checked annotation, doesn't fit
# ValueError: datetime.datetime(2029, 1, 1, 9, 0) doesn't fit (date): would lose its time

backup.props["expires"] = rkdl.Value(datetime.datetime(2029, 1, 1, 9, 0), type="date-time")  # rkdl.Value
backup.props["finished"] = rkdl.Value(datetime.date(2026, 10, 31))  # no (date): reads back as a str
print(repr(backup.props["expires"]))  # reading gives the plain value back, not a Value
# datetime.datetime(2029, 1, 1, 9, 0)
print(doc)
# backup expires=(date-time)"2029-01-01T09:00:00" keep=(u8)14 dir=(path)"/backup" finished="2026-10-31"

backup.props["tags"] = ["daily"]  # a class rkdl can't write
# TypeError: can't convert list to a KDL value
```

`rkdl.Value` goes anywhere a value is written: `args`, `props`, `entry.value`, `append`, and the values you pass to
`rkdl.Node`.

## Your own annotations

To read values with an annotation of your own as Python objects, describe the annotation with an `rkdl.Type` and pass
it to an `rkdl.Parser`. A `Type` takes the annotation name, your Python class, a `decode` function that turns the
plain KDL value into your object and an `encode` function that turns it back. Say `kitchen.kdl` has this:

```kdl
oven (celsius)180.0
```

```python
import dataclasses


@dataclasses.dataclass
class Celsius:
    degrees: float


def number_to_celsius(number):
    return Celsius(float(number))  # float() raises ValueError for a value that isn't a number


def celsius_to_number(temperature):
    return temperature.degrees


celsius_type = rkdl.Type("celsius", Celsius, decode=number_to_celsius, encode=celsius_to_number)
parser = rkdl.Parser(types=[celsius_type])

doc = parser.load("kitchen.kdl")  # rkdl.load("kitchen.kdl") is the same as rkdl.Parser().load("kitchen.kdl")

oven = doc["oven"]
print(oven.args[0])  # Celsius(degrees=180.0)

oven.args[0] = Celsius(200.0)
fridge = rkdl.Node("fridge", Celsius(4.0))  # rkdl.Node doesn't know your Types
# TypeError: can't convert Celsius to a KDL value
fridge = doc.node("fridge", Celsius(4.0))  # doc.node builds a node that knows this document's Types
doc.children.append(fridge)
print(doc)
# oven (celsius)200.0
# fridge (celsius)4.0
```

Values with your annotation follow the rules in "Writing values", with your `Type` in place of the table: a write
over a `(celsius)` value has to be a `Celsius` or a plain value `decode` accepts. A plain value is written in your
`Type`'s form, `encode(decode(value))`. When `decode` raises any exception, the value doesn't fit: reading it raises
`rkdl.ReadError` and writing it a plain `ValueError`, as for the table's annotations, with your exception as the
`__cause__`.

```python
oven.args[0] = "21.5"  # a plain value decode accepts
print(oven)
# oven (celsius)21.5
oven.args[0] = "warm"  # decode raises, so the write raises
# ValueError: 'warm' doesn't fit (celsius): its decode raised ValueError("could not convert string to float: 'warm'")

warm_doc = parser.parse('oven (celsius)"warm"\n')
warm_temperature = warm_doc["oven"].args[0]  # decode raises, so the read raises
# rkdl.ReadError: 1:6: oven arg 0: "warm" doesn't fit (celsius): its decode raised ValueError("could not convert string to float: 'warm'")
```

## Reformatting the whole file

Edits leave the rest of the file alone. `doc.fmt()` does the opposite: it rewrites the whole document with kdl-rs's
formatter, which changes more than whitespace. That suits a file your tool generates and nobody edits by hand, or a
user who asks your tool to format their file. On a hand-written file it undoes the formatting people chose.

```python
messy_text = """\
server   "web"    port=0x1F90 {
      // the public name
  host "example.com"   // staging uses example.net
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

The formatter dropped the quotes that `"web"` and `"example.com"` don't need and wrote the port in decimal. It also
dropped `// staging uses example.net`: `fmt()` loses a `//` comment at the end of a node's line, and `/* */`
comments between a node's arguments and properties. Comments on their own line stay.

The formatter also drops blank lines. A comment that a blank line kept apart from the node below it, such as a file's
header comment, then belongs to that node, and leaves with it if you remove the node.

After `fmt()`, `line` and `column` still describe the text as it was parsed, before formatting, so report problems
to your user before you format.

`fmt` takes an `rkdl.Formatter` to change how it formats. `indent` is the text for one level, four spaces by default.
`entry_autoformat_keep=True` leaves each argument and property as written, with the spaces before it, so the only
changes are indentation, line breaks and the comments the formatter loses:

```python
doc = rkdl.parse(messy_text)
tab_formatter = rkdl.Formatter(indent="\t", entry_autoformat_keep=True)
doc.fmt(tab_formatter)
print(doc)
```

```kdl
server   "web"    port=0x1F90 {
	// the public name
	host "example.com"
}
```

`// staging uses example.net` is gone here too.

`no_comments=True` removes every comment. Use it only when the comments don't matter, such as a copy made for a
program to read; on a file people edit, it deletes their notes.

## Details: held nodes, comparing, copies, slashdash, encoding, saving

- **A node you hold after `detach()`** is the same Python object, now outside any document. It keeps its values,
  children and comments, you can still edit it, and nodes you hold inside it stay inside it. You can insert it again,
  in the same document or another one. An `Entry` whose argument or property was removed raises `RuntimeError` when
  you use it.
- **Nodes you hold after `fmt()`** still point at the same nodes and keep working.
- **Comparing nodes.** `==` on two nodes means they are the same node in the document, not that they look alike: two
  `dependency "pytest"` lines are two different nodes.
- **Copies.** `node.copy()` brings the comments the node owns, as described under "Comments".
  `doc.copy()` copies the whole text, comments included. Both share nothing with the original, and nothing in a copy
  has a position: `line`, `column` and `path` are `None`.
- **Slashdash (`/-`)**, which comments out a node, an argument, a property or a `{ }` block, counts as a comment:
  the parts it comments out don't show up in `children`, `args`, `props` or `entries`, and they stay in the text.
  Which node a commented-out node belongs to is under "Comments".
- **Encoding and line endings.** `load` reads UTF-8 only, and `dump` writes UTF-8; "When the file isn't valid KDL"
  says what `load` raises for other encodings. Given a path or a file opened in binary mode, both keep line endings as
  they are. A file opened in text mode without `newline=""` lets Python translate them: CRLF becomes LF on reading,
  and LF becomes the platform's line ending on writing.
- **Saving isn't atomic.** `dump` opens the path and writes over the file, so a crash partway through can leave it
  half-written. If your tool needs an atomic save, dump to a temporary file in the same folder and move it over the
  original with `os.replace`.

## Reference

Every class and function has a docstring, which `help()` shows, and the type stub
[`python/rkdl/__init__.pyi`](python/rkdl/__init__.pyi) has them all with full signatures. `Args`, `Props` and
`children` support the rest of the list and dict methods, such as `extend`, `update` and `setdefault`.

## Stability

Text you didn't edit is written back as it was. Between versions, the whitespace where an edit meets the old text may
change.

rkdl is 0.x, so anything may change between versions. The likeliest changes, besides that whitespace, are the text of
error messages, `ParseError`'s diagnostics, and what `fmt()` produces.

## Known limits

- KDL v2 only.
- Documents nested deeper than 1000 levels are refused with a `ParseError`, since deeper ones can overflow the parser
  and crash the process. An edit that would nest deeper raises `ValueError`.
- Known issues:
  - A commented-out top-level node followed by `;`, as in `/-node 1;`, fails to parse, though it's valid KDL.
  - A few invalid files parse anyway, such as ones with the control character U+007F in a string or name.
  - The integer -2<sup>127</sup> is written but can't be read back.
  - `fmt()` loses some comments, as described under "Reformatting the whole file".

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE).
