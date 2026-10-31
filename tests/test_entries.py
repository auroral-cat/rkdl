import collections.abc
import datetime
import gc
import weakref

import pytest
import rkdl


def node(text):
    d = rkdl.parse(text)
    return d, d.children[0]


# Reading


def test_read_args_and_props():
    _, n = node('n 1 "two" #"raw"# 0x1F90 port=8080 flag=#true nothing=#null\n')
    assert list(n.args) == [1, "two", "raw", 0x1F90]
    assert dict(n.props) == {"port": 8080, "flag": True, "nothing": None}
    assert n.args[-1] == 0x1F90
    assert n.args[1:3] == ["two", "raw"]
    assert len(n.args) == 4
    assert len(n.props) == 3
    assert "port" in n.props
    assert "missing" not in n.props
    assert 1 in n.props.values()


def test_views_are_the_abcs():
    _, n = node("n 1 a=2\n")
    assert isinstance(n.args, collections.abc.MutableSequence)
    assert isinstance(n.props, collections.abc.MutableMapping)
    assert isinstance(n.entries, collections.abc.Sequence)


def test_out_of_range():
    _, n = node("n 1 a=2\n")
    with pytest.raises(IndexError):
        n.args[1]
    with pytest.raises(IndexError):
        n.args[1] = 5
    with pytest.raises(IndexError):
        del n.args[-2]
    with pytest.raises(IndexError):
        n.args.entry(3)
    with pytest.raises(KeyError):
        n.props["b"]
    with pytest.raises(IndexError):
        n.entries[2]


def test_entries_in_file_order():
    _, n = node("n 1 a=2 3\n")
    entries = list(n.entries)
    assert [(e.name, e.value) for e in entries] == [(None, 1), ("a", 2), (None, 3)]
    assert n.entries[1] is n.props.entry("a")
    assert n.entries[-1] is n.args.entry(1)
    assert n.entries.index(n.args.entry(1)) == 2
    assert n.args.entry(0) in n.entries
    assert list(reversed(n.entries)) == entries[::-1]
    assert n.entries[1:] == entries[1:]
    assert n.entries[::-2] == [entries[2], entries[0]]
    assert type(n.entries[:1]) is list


def test_entries_hold_only_entries():
    _, n = node("n 1 a=2 3\n")
    one = n.args.entry(0)
    # The values the entries hold aren't entries.
    assert 1 not in n.entries
    assert ("a", 2) not in n.entries
    assert n.entries.count(one) == 1
    assert n.entries.count(1) == 0
    del n.args[0]
    assert n.entries.count(one) == 0


def test_entries_index_takes_a_range_like_a_list():
    _, n = node("n 1 a=2 3\n")
    last = n.args.entry(1)
    assert n.entries.index(last, 1) == 2
    assert n.entries.index(last, -1) == 2
    with pytest.raises(ValueError):
        n.entries.index(last, 0, 2)
    with pytest.raises(ValueError):
        n.entries.index("not an entry")


# Writing


def test_untouched_entries_stay_byte_identical():
    d, n = node('n 0x1F90 /-0o7 #"raw"#   "x"  port=0x1F90 /-gone=1 b=#"r"#\n')
    n.args[2] = "y"
    assert str(d) == 'n 0x1F90 /-0o7 #"raw"#   "y"  port=0x1F90 /-gone=1 b=#"r"#\n'
    n.props["port"] = 8081
    assert str(d) == 'n 0x1F90 /-0o7 #"raw"#   "y"  port=0x1F91 /-gone=1 b=#"r"#\n'


def test_writing_keeps_the_spelling_where_it_can():
    d, n = node("n 0x1F90 port=0x1F90\n")
    n.args[0] = 8081
    n.props.entry("port").value = 1.5
    assert str(d) == "n 0x1F91 port=1.5\n"


def test_slashdash_before_an_edited_entry_stays():
    d, n = node("n /-1 2 /-x=1 x=2\n")
    n.args[0] = 3
    n.props["x"] = 4
    assert str(d) == "n /-1 3 /-x=1 x=4\n"


def test_new_props_append_with_canonical_keys():
    d, n = node("n 1\n")
    n.props["port"] = 80
    n.props["quoted key"] = "v"
    assert str(d) == 'n 1 port=80 "quoted key"=v\n'


def test_props_mapping_methods():
    d, n = node("n a=1 b=2\n")
    assert n.props.get("a") == 1
    assert n.props.get("z", 9) == 9
    assert n.props.setdefault("a", 5) == 1
    assert n.props.setdefault("c", 3) == 3
    n.props.update({"a": 10}, b=20)
    assert list(n.props.items()) == [("a", 10), ("b", 20), ("c", 3)]
    assert str(d) == "n a=10 b=20 c=3\n"
    with pytest.raises(TypeError):
        n.props[1] = 2


def test_props_keys_are_str():
    d, n = node("n a=1 b=2\n")
    # Like a dict's missing key, a key that isn't a str is simply not there.
    with pytest.raises(KeyError) as missing:
        n.props[1]
    assert missing.value.args == (1,)
    with pytest.raises(KeyError) as missing:
        del n.props[1]
    assert missing.value.args == (1,)
    with pytest.raises(KeyError) as missing:
        n.props.pop(1)
    assert missing.value.args == (1,)
    assert 1 not in n.props
    assert n.props.get(1) is None
    assert n.props.get(1, "default") == "default"
    assert n.props.pop(1, "default") == "default"
    assert str(d) == "n a=1 b=2\n"


def test_props_pop_takes_one_default():
    d, n = node("n a=1\n")
    with pytest.raises(TypeError, match="pop expected at most 2 arguments"):
        n.props.pop("a", 1, 2)
    assert str(d) == "n a=1\n"


def test_views_take_int_indexes_and_iterables():
    d, n = node("n 1 2 a=3\n")
    # The messages are CPython's and PyO3's, so only the exception types are rkdl's to promise.
    with pytest.raises(TypeError):
        n.args["0"]
    with pytest.raises(TypeError):
        n.args["0"] = 5
    with pytest.raises(TypeError):
        del n.args["0"]
    with pytest.raises(TypeError):
        n.entries["0"]
    with pytest.raises(TypeError):
        n.args = 5
    with pytest.raises(TypeError):
        n.args[0:1] = 5
    with pytest.raises(TypeError):
        n.args.extend(5)
    with pytest.raises(TypeError):
        n.props = 5
    with pytest.raises(TypeError):
        n.props.update(5)
    # As for a dict, a pair has two items.
    with pytest.raises(ValueError):
        n.props.update([("b",)])
    with pytest.raises(TypeError, match="property keys must be str"):
        n.props.setdefault(1, 2)
    assert str(d) == "n 1 2 a=3\n"


@pytest.mark.xfail(strict=True, reason="props take pairs only as tuples; a dict takes any two-item iterable")
def test_props_update_takes_pairs_as_lists():
    d, n = node("n a=1\n")
    n.props.update([["b", 2]])
    assert str(d) == "n a=1 b=2\n"


def test_props_clear():
    d, n = node("n 1 a=1 2 b=2\n")
    n.props.clear()
    assert str(d) == "n 1 2\n"


def test_args_clear_keeps_props():
    d, n = node("n 1 a=1 2 b=2\n")
    n.args.clear()
    assert str(d) == "n a=1 b=2\n"


def test_props_equal_mappings():
    _, n = node("n 1 a=1 b=#null\n")
    assert n.props == {"b": None, "a": 1}
    assert {"a": 1, "b": None} == n.props
    assert n.props != {"a": 1}
    assert n.props != {"a": 2, "b": None}
    assert n.props == rkdl.Node("m", a=1, b=None).props
    assert n.props != [("a", 1), ("b", None)]
    with pytest.raises(TypeError):
        hash(n.props)


def test_props_popitem_takes_the_last():
    d, n = node("n a=1 2 b=2\n")
    assert n.props.popitem() == ("b", 2)
    assert n.props.popitem() == ("a", 1)
    assert str(d) == "n 2\n"
    with pytest.raises(KeyError):
        n.props.popitem()


def test_args_reverse_keeps_layout():
    d, n = node("n 1  (u8)2 a=3 0x10 /-x 4\n")
    n.args.reverse()
    assert str(d) == "n 4  0x10 a=3 (u8)2 /-x 1\n"
    assert list(n.args) == [4, 16, 2, 1]
    assert n.args.entry(2).type == "u8"


def test_args_reverse_none_or_one():
    d, n = node("n a=1\n")
    n.args.reverse()
    n.args.append(5)
    n.args.reverse()
    assert str(d) == "n 5 a=1\n"


def test_args_iadd():
    d, n = node("n 1 a=1\n")
    args = n.args
    args += (2, 3)
    assert isinstance(args, rkdl.Args)
    assert str(d) == "n 1 2 3 a=1\n"
    args += args
    assert list(n.args) == [1, 2, 3, 1, 2, 3]
    with pytest.raises(TypeError):
        args += 5


def test_args_extend_is_all_or_nothing():
    d, n = node("n 1 a=1\n")
    with pytest.raises(TypeError):
        n.args.extend([5, object()])
    args = n.args
    with pytest.raises(OverflowError):
        args += [5, 2**127]
    assert str(d) == "n 1 a=1\n"


# Arguments and properties interleaved


def test_insert_goes_where_the_ith_argument_is():
    d, n = node("n 1 a=1 2 b=2 3\n")
    n.args.insert(1, "x")
    assert str(d) == "n 1 a=1 x 2 b=2 3\n"
    n.args.insert(-1, "y")
    assert str(d) == "n 1 a=1 x 2 b=2 y 3\n"
    n.args.insert(100, "end")
    assert str(d) == "n 1 a=1 x 2 b=2 y 3 end\n"
    assert list(n.args) == [1, "x", 2, "y", 3, "end"]


def test_append_goes_after_the_last_argument():
    d, n = node("n 1 a=1 2 b=2\n")
    n.args.append(3)
    assert str(d) == "n 1 a=1 2 3 b=2\n"


def test_append_with_no_arguments_goes_first():
    d, n = node("n a=1\n")
    n.args.append(3)
    n.args.extend([4, 5])
    assert str(d) == "n 3 4 5 a=1\n"


def test_delete_and_pop_arguments():
    d, n = node("n 1 a=1 2 b=2 3 4\n")
    assert n.args.pop() == 4
    del n.args[0]
    assert str(d) == "n a=1 2 b=2 3\n"
    n.args.remove(3)
    assert str(d) == "n a=1 2 b=2\n"
    with pytest.raises(ValueError):
        n.args.remove(99)


def test_delete_argument_slice():
    d, n = node("n 1 2 x=0 3 4\n")
    del n.args[::2]
    assert str(d) == "n 2 x=0 4\n"


def test_args_search_methods():
    _, n = node("n 1 2 1\n")
    assert n.args.index(2) == 1
    assert n.args.count(1) == 2
    assert 2 in n.args
    assert 9 not in n.args
    assert list(reversed(n.args)) == [1, 2, 1]


def test_iteration_snapshots():
    d, n = node("n 1 2 3 a=1 b=2\n")
    seen = []
    for v in n.args:
        seen.append(v)
        n.args.append(v)
    assert seen == [1, 2, 3]
    keys = []
    for k in n.props:
        keys.append(k)
        del n.props[k]
    assert keys == ["a", "b"]
    values = []
    for e in n.entries:
        values.append(e.value)
        n.args.remove(e.value)
    assert values == [1, 2, 3, 1, 2, 3]
    assert str(d) == "n\n"


# Type annotations


def test_annotation_kept_when_the_value_fits():
    d, n = node("n (u8)1 port=(u16)80\n")
    n.args[0] = 200
    n.props["port"] = 8080
    assert str(d) == "n (u8)200 port=(u16)8080\n"


def test_a_value_that_does_not_fit_raises():
    d, n = node("n (u8)1 when=(u8)2\n")
    with pytest.raises(ValueError, match="doesn't fit \\(u8\\): expected an integer"):
        n.args[0] = "x"
    with pytest.raises(ValueError, match="doesn't fit \\(u8\\): expected an integer"):
        n.props["when"] = datetime.date(2024, 1, 2)
    assert str(d) == "n (u8)1 when=(u8)2\n"


def test_annotation_inferred_after_removing_it():
    d, n = node("n when=(u8)2\n")
    n.props.entry("when").type = None
    n.props["when"] = datetime.date(2024, 1, 2)
    assert str(d) == 'n when=(date)"2024-01-02"\n'


def test_unknown_annotation_is_kept():
    d, n = node("n (celsius)20\n")
    n.args[0] = 21.5
    assert str(d) == "n (celsius)21.5\n"


def test_set_and_remove_annotation():
    d, n = node("n 1 x=( u8 ) 2\n")
    e = n.args.entry(0)
    e.type = "u8"
    assert e.type == "u8"
    assert str(d) == "n (u8)1 x=( u8 ) 2\n"
    x = n.props.entry("x")
    x.type = None
    assert x.type is None
    assert str(d) == "n (u8)1 x=2\n"
    assert x.value == 2


def test_setting_a_type_the_value_does_not_fit_raises():
    d, n = node("n 5 300\n")
    e = n.args.entry(0)
    with pytest.raises(ValueError, match="date"):
        e.type = "date"
    with pytest.raises(ValueError, match="u8"):
        n.args.entry(1).type = "u8"
    assert str(d) == "n 5 300\n"
    assert n.args == [5, 300]
    e.type = "u8"
    assert str(d) == "n (u8)5 300\n"


def test_a_value_that_does_not_fit_its_type_still_has_a_repr():
    _, n = node("n (date)5\n")
    e = n.args.entry(0)
    with pytest.raises(ValueError):
        _ = e.value
    with pytest.raises(ValueError):
        n.args[0]
    assert repr(e) == "<Entry <kdl: (date)5>>"
    assert repr(n.args) == "Args([<kdl: (date)5>])"


def test_known_type_that_does_not_fit_raises_on_read():
    _, n = node('n (u8)300 d=(date)"nope"\n')
    with pytest.raises(ValueError):
        n.args[0]
    with pytest.raises(ValueError):
        n.props["d"]


def test_annotation_converts_on_read():
    _, n = node('n (date)"2024-01-02"\n')
    assert n.args[0] == datetime.date(2024, 1, 2)


# Duplicate keys: these follow kdl-rs exactly. If kdl-rs changes, these tests fail on purpose.


def test_duplicate_keys_read_and_write_the_last():
    d, n = node("n a=1 a=2\n")
    assert n.props["a"] == 2
    assert len(n.props) == 1
    assert list(n.props) == ["a"]
    n.props["a"] = 3
    assert str(d) == "n a=1 a=3\n"
    assert n.props.entry("a") is n.entries[1]


def test_duplicate_keys_equality_and_popitem():
    d, n = node("n a=1 b=2 a=3\n")
    assert n.props == {"a": 3, "b": 2}
    assert n.props.popitem() == ("a", 3)
    assert n.props == {"a": 1, "b": 2}
    assert str(d) == "n a=1 b=2\n"


def test_del_removes_the_duplicate_props_reads():
    _, n = node("n a=1 a=2\n")
    assert n.props["a"] == 2
    del n.props["a"]
    assert [(e.name, e.value) for e in n.entries] == [("a", 1)]
    assert n.props["a"] == 1


def test_del_finds_a_quoted_key():
    d, n = node('n 1 "quoted key"=1\n')
    del n.props["quoted key"]
    assert str(d) == "n 1\n"


def test_del_finds_parsed_keys():
    d, n = node("n port=1 x=2\n")
    del n.props["port"]
    assert n.props.pop("x") == 2
    assert str(d) == "n\n"


def test_del_finds_keys_added_through_the_api():
    d, n = node("n 1\n")
    n.props["port"] = 80
    del n.props["port"]
    assert str(d) == "n 1\n"
    n.props["x"] = 2
    assert n.props.pop("x") == 2
    assert n.props.pop("x", None) is None
    with pytest.raises(KeyError):
        del n.props["x"]


# Entry handles


def test_entry_identity():
    _, n = node("n 1 a=2\n")
    assert n.args.entry(0) is n.args.entry(0)
    assert n.props.entry("a") is n.entries[1]
    assert n.args.entry(0) != n.props.entry("a")
    assert hash(n.args.entry(0)) == hash(n.entries[0])


def test_entry_handles_are_not_kept_alive():
    _, n = node("n 1\n")
    ref = weakref.ref(n.args.entry(0))
    gc.collect()
    assert ref() is None
    assert n.args.entry(0).value == 1


def test_entry_attributes():
    _, n = node('n "web" port=(u16)0x1F90\n')
    arg, prop = n.entries
    assert arg.name is None
    assert prop.name == "port"
    assert prop.type == "u16"
    assert str(arg) == '"web"'
    assert str(prop) == "port=(u16)0x1F90"
    assert repr(arg) == "<Entry 'web'>"
    assert repr(prop) == "<Entry port=(u16)8080>"
    name, value = prop
    assert (name, value) == ("port", 8080)


def test_entry_repr_quotes_keys_and_types():
    _, n = node('n "a b"=("my type")1 "+.7"=2\n')
    assert repr(n.props.entry("a b")) == '<Entry "a b"=("my type")1>'
    assert repr(n.props.entry("+.7")) == '<Entry "+.7"=2>'


def test_entries_repr_lists_the_entries():
    _, n = node('n "web" port=(u16)0x1F90\n')
    assert repr(n.entries) == "Entries([<Entry 'web'>, <Entry port=(u16)8080>])"
    assert repr(rkdl.Node("n").entries) == "Entries([])"


def test_entries_equality_is_by_identity():
    _, n = node("n 1 a=1\n")
    _, m = node("n 1 a=1\n")
    assert n.entries == list(n.entries)
    assert n.entries == n.entries
    assert n.entries != m.entries
    assert n.entries != list(n.entries)[::-1]
    assert n.entries != [1, 1]
    assert n.entries != list(n.entries)[:1]
    assert n.entries != [*n.entries, n.entries[0]]
    assert n.entries != tuple(n.entries)
    with pytest.raises(TypeError):
        hash(n.entries)


def test_removed_entry_is_dead():
    _, n = node("n 1 2 a=3\n")
    first = n.args.entry(0)
    a = n.props.entry("a")
    del n.args[0]
    n.props.clear()
    for e in (first, a):
        with pytest.raises(RuntimeError, match="entry was removed"):
            _ = e.value
        with pytest.raises(RuntimeError, match="entry was removed"):
            e.value = 5
        with pytest.raises(RuntimeError, match="entry was removed"):
            e.type = "u8"
        with pytest.raises(RuntimeError, match="entry was removed"):
            str(e)
    assert first not in n.entries
    assert n.args.entry(0).value == 2


def test_entry_follows_its_node():
    d = rkdl.parse("a 1 x=2\nb {\n    c\n}\n")
    a = d["a"]
    one, x = a.args.entry(0), a.props.entry("x")
    d.children.remove(a)
    one.value = 10
    assert a.args[0] == 10
    d["b"].children.append(a)
    x.value = 20
    assert str(d) == "b {\n    c\n    a 10 x=20\n}\n"
    assert a.args.entry(0) is one
    assert d["b"]["a"].props.entry("x") is x


def test_entry_value_write_on_handle():
    d, n = node("n (u8)1\n")
    e = n.args.entry(0)
    e.value = 2
    assert str(d) == "n (u8)2\n"
    with pytest.raises(ValueError, match="'s' doesn't fit"):
        e.value = "s"
    e.type = None
    e.value = "s"
    assert str(d) == "n s\n"
    assert e.type is None


def test_detached_node_entries():
    n = rkdl.Node("n", 1, port=80)
    n.args.append(2)
    n.props["port"] = 81
    assert list(n.args) == [1, 2]
    assert dict(n.props) == {"port": 81}
    d = rkdl.parse("")
    d.children.append(n)
    assert str(d) == "n 1 2 port=81\n"
