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


if __name__ == "__main__":
    unittest.main()
