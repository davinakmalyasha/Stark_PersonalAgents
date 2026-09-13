"""MCP per-scope separation tests: one global list, derived per-agent views."""

import unittest
from types import SimpleNamespace

from tools.mcp.mcp_bridge import (
    McpManager,
    _canonical_scope,
    _fq_tool_name,
    _parse_fq,
    get_manager,
)


def _client(name="srv", scopes=(), tools=None, **cfg_extra):
    return SimpleNamespace(
        name=name,
        status="connected",
        cfg={
            "scopes": list(scopes),
            "enabled_tools": [],
            "disabled_tools": [],
            **cfg_extra,
        },
        tools=tools or [],
    )


class TestScopeMatching(unittest.TestCase):
    def setUp(self):
        self.mgr = McpManager.__new__(McpManager)  # no config I/O

    def test_developer_scoped_server_reachable_from_dev_execution(self):
        """The Dev mismatch: schemas ask 'developer', execution runs as 'Dev'."""
        c = _client(scopes=["developer"])
        self.assertTrue(self.mgr._scope_matches(c, "developer"))  # schema side
        self.assertTrue(self.mgr._scope_matches(c, "Dev"))        # task('Dev') exec
        self.assertFalse(self.mgr._scope_matches(c, "General"))

    def test_general_scoped_server_matches_general_only(self):
        c = _client(scopes=["general"])
        self.assertTrue(self.mgr._scope_matches(c, "General"))
        self.assertTrue(self.mgr._scope_matches(c, "general"))
        self.assertFalse(self.mgr._scope_matches(c, "dev"))

    def test_server_side_tags_are_normalized_too(self):
        c = _client(scopes=["DEV"])  # sloppy tag still means developer
        self.assertTrue(self.mgr._scope_matches(c, "Dev"))

    def test_empty_scopes_is_global(self):
        c = _client()
        for scope in ("", "General", "Dev", "Orchestrator", "anything"):
            self.assertTrue(self.mgr._scope_matches(c, scope))

    def test_canonical_scope_aliases(self):
        self.assertEqual(_canonical_scope("Dev"), "developer")
        self.assertEqual(_canonical_scope(" DEV "), "developer")
        self.assertEqual(_canonical_scope("General"), "general")
        self.assertEqual(_canonical_scope(""), "")

    def test_ui_scoped_server_matches_ui_execution_only(self):
        c = _client(scopes=["ui"])
        self.assertTrue(self.mgr._scope_matches(c, "UI"))
        self.assertTrue(self.mgr._scope_matches(c, "ui"))
        self.assertFalse(self.mgr._scope_matches(c, "Dev"))
        self.assertFalse(self.mgr._scope_matches(c, "General"))


class TestPerScopeToolViews(unittest.TestCase):
    """One manager, several tagged servers -> distinct per-scope tool lists."""

    def setUp(self):
        self.mgr = get_manager()
        self.mgr._started = False  # keep queries side-effect free
        self.mgr._clients = {
            "notion": _client(
                name="notion",
                scopes=["general"],
                tools=[{"name": "search", "description": "Search pages",
                        "inputSchema": {"properties": {"q": {"type": "string"}},
                                        "required": ["q"]}}],
            ),
            "repo": _client(
                name="repo",
                scopes=["developer"],
                tools=[{"name": "grep", "description": "Grep code",
                        "inputSchema": {"properties": {}, "required": []}}],
            ),
            "global": _client(
                name="global",
                tools=[{"name": "ping", "description": "", "inputSchema": {}}],
            ),
            "shadcn": _client(
                name="shadcn",
                scopes=["ui"],
                tools=[{"name": "search_items", "description": "Search registry",
                        "inputSchema": {"properties": {"q": {"type": "string"}},
                                        "required": ["q"]}}],
            ),
        }

    def test_general_view_gets_general_and_global_only(self):
        names = [d["function"]["name"] for d in self.mgr.tool_defs_for_scope("General")]
        self.assertEqual(sorted(names), sorted([
            _fq_tool_name("notion", "search"),
            _fq_tool_name("global", "ping"),
        ]))

    def test_dev_view_gets_developer_and_global_only(self):
        names = [d["function"]["name"] for d in self.mgr.tool_defs_for_scope("Dev")]
        self.assertEqual(sorted(names), sorted([
            _fq_tool_name("repo", "grep"),
            _fq_tool_name("global", "ping"),
        ]))

    def test_disabled_tools_are_filtered_out(self):
        self.mgr._clients["notion"].cfg["disabled_tools"] = ["search"]
        names = [d["function"]["name"] for d in self.mgr.tool_defs_for_scope("general")]
        self.assertEqual(names, [_fq_tool_name("global", "ping")])

    def test_fq_name_round_trip(self):
        fq = _fq_tool_name("My Server", "do__thing")
        self.assertEqual(fq, "mcp__My_Server__do__thing")
        server, tool = _parse_fq(fq)
        self.assertEqual((server, tool), ("My_Server", "__".join(["do", "thing"])))

    def test_ui_view_gets_ui_scoped_and_global_only(self):
        names = [d["function"]["name"] for d in self.mgr.tool_defs_for_scope("UI")]
        self.assertEqual(sorted(names), sorted([
            _fq_tool_name("shadcn", "search_items"),
            _fq_tool_name("global", "ping"),
        ]))
        dev_names = [d["function"]["name"] for d in self.mgr.tool_defs_for_scope("Dev")]
        self.assertNotIn(_fq_tool_name("shadcn", "search_items"), dev_names)

    def test_schemas_native_names_follow_scope(self):
        from tools._schemas import get_native_tool_names

        ui_names = get_native_tool_names("UI")
        self.assertIn(_fq_tool_name("shadcn", "search_items"), ui_names)
        self.assertNotIn(
            _fq_tool_name("shadcn", "search_items"), get_native_tool_names("Dev")
        )


class TestMcpExecutionFallback(unittest.TestCase):
    def test_execute_tool_detailed_dispatches_mcp_names(self):
        from unittest.mock import patch

        from tools._registry import execute_tool_detailed

        with patch(
            "tools.mcp.mcp_bridge.mcp_execute", return_value="ok-from-bridge"
        ) as m:
            res = execute_tool_detailed(
                "mcp__shadcn__search_items", {"q": "button"}, scope="UI"
            )
        self.assertEqual(res.content, "ok-from-bridge")
        m.assert_called_once_with(
            "mcp__shadcn__search_items", {"q": "button"}, scope="UI"
        )


class TestSubagentToolScope(unittest.TestCase):
    def test_tool_in_scope_allows_mcp_names_and_spec_tools(self):
        from tools.AddOnTool.subagentTools import _tool_in_scope

        spec = ["browser_act", "edit_file"]
        self.assertTrue(_tool_in_scope("browser_act", spec))
        self.assertTrue(_tool_in_scope("mcp__shadcn__search_items", spec))
        self.assertFalse(_tool_in_scope("run_command", spec))
        self.assertTrue(_tool_in_scope("anything", []))


if __name__ == "__main__":
    unittest.main()
