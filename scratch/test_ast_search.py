"""ast_search warehouse-tool checks: rules, output, registry path, scope."""
import os
import shutil
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

FIXTURE_REL = os.path.join("scratch", "_ast_test_tmp")
FIXTURE = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), FIXTURE_REL)

_MOD = '''import os
import json as js


class Base:
    pass


class Child(Base):
    def method(self):
        pass


@decorator_marker
def decorated():
    pass


def helper(a, b, c, d, e, f):
    try:
        do_thing()
    except:
        pass
    os.path.join("a", "b")


def caller():
    helper(1, 2, 3, 4, 5, 6)
'''


def _ast(**kw):
    from tools.MainTool.astSearch import ast_search
    return ast_search(**kw)


class AstSearchTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        shutil.rmtree(FIXTURE, ignore_errors=True)
        os.makedirs(os.path.join(FIXTURE, "pkg"))
        with open(os.path.join(FIXTURE, "pkg", "mod.py"), "w", encoding="utf-8") as f:
            f.write(_MOD)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(FIXTURE, ignore_errors=True)

    def test_call_dotted(self):
        out = _ast(rule="call", name="os.path.join", path=FIXTURE_REL)
        self.assertIn("os.path.join()", out)
        self.assertIn("[def helper]", out)

    def test_call_last_segment(self):
        out = _ast(rule="call", name="join", path=FIXTURE_REL)
        self.assertIn("os.path.join()", out)

    def test_call_site(self):
        out = _ast(rule="call", name="helper", path=FIXTURE_REL)
        self.assertIn("[def caller]", out)

    def test_import(self):
        out = _ast(rule="import", name="json", path=FIXTURE_REL)
        self.assertIn("import json", out)

    def test_class_bases(self):
        out = _ast(rule="class_bases", name="Base", path=FIXTURE_REL)
        self.assertIn("class Child(Base)", out)

    def test_decorator(self):
        out = _ast(rule="decorator", name="decorator_marker", path=FIXTURE_REL)
        self.assertIn("@decorator_marker on def decorated", out)

    def test_bare_except(self):
        out = _ast(rule="bare_except", path=FIXTURE_REL)
        self.assertIn("except:", out)

    def test_many_params(self):
        out = _ast(rule="many_params", max_params=5, path=FIXTURE_REL)
        self.assertIn("def helper (6 params)", out)

    def test_no_docstring(self):
        out = _ast(rule="no_docstring", path=FIXTURE_REL)
        self.assertIn("def helper (no docstring)", out)

    def test_long_function(self):
        out = _ast(rule="long_function", min_lines=2, path=FIXTURE_REL)
        self.assertIn("def helper", out)

    def test_invalid_rule(self):
        out = _ast(rule="nope", path=FIXTURE_REL)
        self.assertTrue(out.startswith("Error: rule must be one of"), out)

    def test_name_required(self):
        out = _ast(rule="call", path=FIXTURE_REL)
        self.assertIn("requires name", out)

    def test_registry_path_executes(self):
        from tools._registry import execute_tool_detailed
        res = execute_tool_detailed("ast_search", {"rule": "bare_except", "path": FIXTURE_REL})
        self.assertIn("except:", res.content)
        self.assertFalse(res.is_error)

    def test_warehouse_not_in_developer_scope(self):
        from agents.workspace_registry import WORKSPACE_REGISTRY
        self.assertNotIn("ast_search", WORKSPACE_REGISTRY["developer"].tools)
        self.assertIn("search_code", WORKSPACE_REGISTRY["developer"].tools)

    def test_relative_paths(self):
        out = _ast(rule="bare_except", path=FIXTURE_REL)
        self.assertIn("scratch/_ast_test_tmp/pkg/mod.py", out)
        self.assertNotIn(os.getcwd().replace("\\", "/"), out)


if __name__ == "__main__":
    unittest.main(verbosity=2)
