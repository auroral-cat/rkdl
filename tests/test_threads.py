import sys
import threading

import pytest
import rkdl
from conftest import run_threads as run

ROUNDS = 300


@pytest.fixture(autouse=True)
def fast_switching():
    # Under the GIL, switch threads often so the races get a chance to happen.
    old = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    yield
    sys.setswitchinterval(old)


def mover(node, home, away, done=None, at=None):
    """A thread target that moves `node` from `home` into `away` and back, ROUNDS times.

    `home` and `away` are documents or nodes. The node goes back to index `at` of home, or the end. Sets `done`, if
    given, when it stops.
    """

    def move():
        try:
            for _ in range(ROUNDS):
                home.children.remove(node)
                away.children.append(node)
                away.children.remove(node)
                if at is None:
                    home.children.append(node)
                else:
                    home.children.insert(at, node)
            return ROUNDS
        finally:
            if done is not None:
                done.set()

    return move


def until(done, check):
    """A thread target that runs `check` until `done` is set, and returns how many times it ran."""

    def loop():
        count = 0
        while not done.is_set():
            check()
            count += 1
        return count

    return loop


def test_concurrent_lookups_share_one_handle():
    for _ in range(50):
        d = rkdl.parse("a\nx 1 {\n    y\n}\nb\n")
        found = run(*[lambda d=d: (d["x"], d["x"]["y"])] * 8)
        xs, ys = [x for x, _ in found], [y for _, y in found]
        assert all(x is xs[0] for x in xs)
        assert all(y is ys[0] for y in ys)


def test_reads_while_a_node_moves_between_documents():
    first = rkdl.parse("a\nmover 1 {\n    inner\n}\nb\n")
    second = rkdl.parse("c {\n    d\n}\n")
    node = first["mover"]
    inner = node["inner"]
    done = threading.Event()

    def read():
        assert node.name == "mover"
        assert node["inner"] is inner
        assert inner.name == "inner"
        assert len(node.children) == 1

    def look():
        for n in first.children:
            assert n.name in ("a", "mover", "b")
        found = first.get("mover")
        assert found is None or found is node

    counts = run(mover(node, first, second["c"], done, at=1), *[until(done, read)] * 3, until(done, look))
    assert all(counts), counts
    assert str(first) == "a\nmover 1 {\n    inner\n}\nb\n"
    assert str(second) == "c {\n    d\n}\n"
    assert first["mover"] is node


def test_concurrent_entry_lookups_share_one_handle():
    for _ in range(50):
        n = rkdl.parse("x 1 a=2\n")["x"]
        found = run(*[lambda n=n: (n.args.entry(0), n.props.entry("a"), n.entries[1])] * 8)
        assert all(f[0] is found[0][0] for f in found)
        assert all(f[1] is found[0][1] and f[2] is found[0][1] for f in found)


def test_entry_reads_while_its_node_moves_between_documents():
    first = rkdl.parse("a\nmover 1 port=(u16)0x1F90\nb\n")
    second = rkdl.parse("c {\n    d\n}\n")
    node = first["mover"]
    arg, port = node.args.entry(0), node.props.entry("port")
    done = threading.Event()

    def read():
        assert arg.value == 1
        assert port.value == 8080
        assert node.props.entry("port") is port
        assert list(node.args) == [1]

    def write():
        arg.value = 1
        node.props["port"] = 8080

    counts = run(mover(node, first, second["c"], done, at=1), *[until(done, read)] * 2, until(done, write))
    assert all(counts), counts
    # Every write was of the value already there.
    assert str(first) == "a\nmover 1 port=(u16)0x1F90\nb\n"
    assert str(second) == "c {\n    d\n}\n"


def test_nodes_moving_both_ways_between_two_documents():
    first = rkdl.parse("x 1 {\n    deep\n}\nstay\n")
    second = rkdl.parse("y 2\nother {\n    z\n}\n")
    x, y = first["x"], second["y"]
    other = second["other"]
    # x goes first → second while y goes second → first. Meanwhile deep goes from x's block to other's while z goes
    # from other's to x's, with x itself moving between the documents.
    counts = run(
        mover(x, first, second), mover(y, second, first), mover(x["deep"], x, other), mover(other["z"], other, x)
    )
    assert counts == [ROUNDS] * 4
    assert sorted(n.name for n in first.children) == ["stay", "x"]
    assert sorted(n.name for n in second.children) == ["other", "y"]
    assert first["x"] is x and second["y"] is y
    assert [n.name for n in x.children] == ["deep"]
    assert [n.name for n in other.children] == ["z"]
    for doc in (first, second):
        assert str(rkdl.parse(str(doc))) == str(doc)


def test_pop_and_a_concurrent_write_keep_both():
    # Either order is fine: popping first returns 1 and the write lands on 2; writing first makes the pop return 99.
    # Before pop checked the value it read, the write could land on the entry being popped and be lost.
    for _ in range(ROUNDS):
        node = rkdl.parse("n 1 2 k=1\n")["n"]

        def write(node=node):
            node.args[0] = 99
            node.props["k"] = 99

        def pop(node=node):
            return node.args.pop(0), node.props.pop("k")

        (arg, prop), _ = run(pop, write)
        assert (arg, list(node.args)) in [(1, [99]), (99, [2])]
        assert (prop, dict(node.props)) in [(1, {"k": 99}), (99, {})]


def test_edits_keep_retrying_while_other_threads_move_the_node():
    # An edit gives up only when its own callbacks keep changing the document. Other threads' changes, however many,
    # just make it try again.
    first = rkdl.parse("a 1 k=1\n")
    second = rkdl.parse("")
    node = first["a"]
    done = threading.Event()
    editors = ["first", "second"]  # each one still running

    def bounce():
        src, dst = (first, second) if len(first.children) else (second, first)
        try:
            dst.children.append(src.children.pop())
        except IndexError:
            pass  # the other bouncer took it first

    def edit():
        try:
            for _ in range(ROUNDS):
                node.args.append(2)
                node.args.pop()
                node.props["k"] = 2
                node.props.pop("k", None)  # the other editor may have popped it
            return ROUNDS
        finally:
            editors.pop()
            if not editors:
                done.set()

    counts = run(until(done, bounce), until(done, bounce), edit, edit)
    assert counts[2:] == [ROUNDS, ROUNDS]
    assert str(node) == "a 1"


def test_detached_nodes_inserted_into_each_other():
    # Each node's block lives in that node's own private document, so the two inserts lock the same two documents
    # in opposite roles. One wins, and the other finds it would put a node inside itself.
    for _ in range(ROUNDS):
        a, b = rkdl.Node("a"), rkdl.Node("b")
        a.children = []
        b.children = []

        def put(node, into):
            def go():
                try:
                    into.children.append(node)
                except ValueError:
                    return False
                return True

            return go

        won = run(put(a, b), put(b, a))
        assert sorted(won) == [False, True]
        outer, inner = (b, a) if won[0] else (a, b)
        assert list(outer.children) == [inner]
        assert list(inner.children) == []
