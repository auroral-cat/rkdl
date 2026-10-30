import sys
import threading

import pytest
import rkdl


@pytest.fixture(autouse=True)
def fast_switching():
    # Under the GIL, switch threads often so the races get a chance to happen.
    old = sys.getswitchinterval()
    sys.setswitchinterval(1e-6)
    yield
    sys.setswitchinterval(old)


def run(threads):
    errors = []

    def wrap(target):
        def go():
            try:
                target()
            except BaseException as e:  # noqa: BLE001
                errors.append(e)

        return go

    started = [threading.Thread(target=wrap(t)) for t in threads]
    for t in started:
        t.start()
    for t in started:
        t.join()
    if errors:
        raise errors[0]


def test_concurrent_lookups_share_one_handle():
    for _ in range(50):
        d = rkdl.parse("a\nx 1 {\n    y\n}\nb\n")
        barrier = threading.Barrier(8)
        found = []

        def look():
            barrier.wait()
            found.append((d["x"], d["x"]["y"]))

        run([look] * 8)
        xs, ys = [x for x, _ in found], [y for _, y in found]
        assert all(x is xs[0] for x in xs)
        assert all(y is ys[0] for y in ys)


def test_reads_while_a_node_moves_between_documents():
    first = rkdl.parse("a\nmover 1 {\n    inner\n}\nb\n")
    second = rkdl.parse("c {\n    d\n}\n")
    node = first["mover"]
    inner = node["inner"]
    done = threading.Event()

    def move():
        try:
            for _ in range(300):
                first.children.remove(node)
                second["c"].children.append(node)
                second["c"].children.remove(node)
                first.children.insert(1, node)
        finally:
            done.set()

    def read():
        while not done.is_set():
            assert node.name == "mover"
            assert node["inner"] is inner
            assert inner.name == "inner"
            assert len(node.children) == 1

    def look():
        while not done.is_set():
            for n in first.children:
                assert n.name in ("a", "mover", "b")
            found = first.get("mover")
            assert found is None or found is node

    run([move, read, read, read, look])
    assert str(first) == "a\nmover 1 {\n    inner\n}\nb\n"
    assert str(second) == "c {\n    d\n}\n"
    assert first["mover"] is node


def test_concurrent_entry_lookups_share_one_handle():
    for _ in range(50):
        d = rkdl.parse("x 1 a=2\n")
        n = d["x"]
        barrier = threading.Barrier(8)
        found = []

        def look():
            barrier.wait()
            found.append((n.args.entry(0), n.props.entry("a"), n.entries[1]))

        run([look] * 8)
        assert all(f[0] is found[0][0] for f in found)
        assert all(f[1] is found[0][1] and f[2] is found[0][1] for f in found)


def test_entry_reads_while_its_node_moves_between_documents():
    first = rkdl.parse("a\nmover 1 port=(u16)0x1F90\nb\n")
    second = rkdl.parse("c {\n    d\n}\n")
    node = first["mover"]
    arg, port = node.args.entry(0), node.props.entry("port")
    done = threading.Event()

    def move():
        try:
            for _ in range(300):
                first.children.remove(node)
                second["c"].children.append(node)
                second["c"].children.remove(node)
                first.children.insert(1, node)
        finally:
            done.set()

    def read():
        while not done.is_set():
            assert arg.value == 1
            assert port.value == 8080
            assert node.props.entry("port") is port
            assert list(node.args) == [1]

    def write():
        while not done.is_set():
            arg.value = 1
            node.props["port"] = 8080

    run([move, read, read, write])
    assert str(first) == "a\nmover 1 port=(u16)8080\nb\n"
    assert str(second) == "c {\n    d\n}\n"
