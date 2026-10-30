from rkdl._rkdl import _debug_assertions


def test_tests_run_on_a_debug_build():
    # The id-tree shape check after every edit only exists in debug builds; see `config-settings-package` in
    # pyproject.toml.
    assert _debug_assertions()
