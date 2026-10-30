"""The `.pyi` stub matches the built module: types through stubtest, docstrings and signatures directly."""

import ast
import inspect
import subprocess
import sys
from pathlib import Path

import pytest
import rkdl

STUB = Path(rkdl.__file__).with_suffix(".pyi")


def stub_docstrings():
    """`{"Node.copy": docstring}` for every class, function, method and property in the stub, None if it has none.
    Overloads share the first overload's docstring."""
    found = {}

    def add(name, node):
        if name not in found or found[name] is None:
            found[name] = ast.get_docstring(node)

    for node in ast.parse(STUB.read_text()).body:
        if isinstance(node, ast.FunctionDef) and not node.name.startswith("_"):
            add(node.name, node)
        if isinstance(node, ast.ClassDef) and not node.name.startswith("_"):
            add(node.name, node)
            for item in node.body:
                if isinstance(item, ast.FunctionDef) and not item.name.startswith("_"):
                    add(f"{node.name}.{item.name}", item)
    return found


DOCSTRINGS = stub_docstrings()


def runtime(path):
    owner, _, attr = path.partition(".")
    obj = getattr(rkdl, owner)
    return inspect.getattr_static(obj, attr) if attr else obj


def test_stubtest():
    # stubtest imports rkdl and compares every name, signature and property in the stub with it. It needs no
    # allowlist: PyO3 gives every function a signature, and everything else stubtest can see.
    result = subprocess.run(
        [sys.executable, "-m", "mypy.stubtest", "rkdl"],
        check=False,
        capture_output=True,
        text=True,
    )
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize("path", sorted(DOCSTRINGS))
def test_stub_docstring_matches_module(path):
    # Editors show the stub's docstring and help() shows the module's, so they're kept the same.
    stub = DOCSTRINGS[path]
    assert stub is not None, f"{path} has no docstring in the stub"
    assert inspect.cleandoc(runtime(path).__doc__ or "") == stub


@pytest.mark.parametrize("name", sorted(rkdl.__all__))
def test_signatures_show_their_defaults(name):
    # PyO3 shows a default it can't spell as `...`, which a `text_signature` fixes.
    obj = getattr(rkdl, name)
    callables = [obj] if callable(obj) else []
    if isinstance(obj, type):
        callables += [m for n, m in vars(obj).items() if callable(m) and not n.startswith("_")]
    for c in callables:
        try:
            signature = inspect.signature(c)
        except ValueError:
            continue
        assert all(p.default is not Ellipsis for p in signature.parameters.values()), f"{name}: {signature}"
