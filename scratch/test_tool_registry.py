"""Self-test for P4 tool registry + pipeline. Run: venv312\\Scripts\\python.exe scratch/test_tool_registry.py"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tool_registry import ToolDefinition, active_tool_scope, get_registry

PASS = 0


def check(label, cond):
    global PASS
    if not cond:
        raise AssertionError(f"FAIL: {label}")
    PASS += 1
    print(f"  ok {PASS}: {label}")


def main():
    # ── Registry primitives ──
    reg = get_registry()

    def _fake_execute(**kw):
        return f"ran {kw.get('x')}"

    reg.register(ToolDefinition(
        name="demo_tool", description="demo_tool(x) â€” Demo.",
        parameters={"type": "object", "properties": {"x": {"type": "string"}}, "required": ["x"]},
        execute=_fake_execute,
    ))
    check("registered", reg.get("demo_tool") is not None)

    # scope visibility
    reg.restrict_scope("demo-scope", allow=["demo_tool"])
    check("names_for global", "demo_tool" in reg.names_for())
    check("names_for scoped", reg.names_for("demo-scope") == ["demo_tool"])
    reg.restrict_scope("deny-scope", deny=["demo_tool"])
    check("deny hides", "demo_tool" not in reg.names_for("deny-scope"))

    # schemas: structural, no description by default
    s = reg.schemas_for("demo-scope")
    check("schema structural", s[0]["function"]["name"] == "demo_tool"
          and "description" not in s[0]["function"])
    sd = reg.schemas_for("demo-scope", include_description=True)
    check("schema optional description", "description" in sd[0]["function"])

    # compact defs
    c = reg.compact_defs_for("demo-scope")
    check("compact doc", c["demo_tool"] == "demo_tool(x) â€” Demo.")

    # execution pipeline: allow / deny / unknown
    check("execute ok", reg.execute("demo_tool", {"x": "hi"}) == "ran hi")
    check("execute scoped", reg.execute("demo_tool", {"x": "hi"}, scope="demo-scope") == "ran hi")
    denied = reg.execute("demo_tool", {"x": "hi"}, scope="deny-scope")
    check("execute denied by scope", denied.startswith("Error: Unknown tool"))
    unk = reg.execute("nope", {})
    check("unknown tool error", "Unknown tool" in unk)

    # arg normalization (path -> file_path)
    def _fp(file_path=None, **kw):
        return f"fp={file_path}"
    reg.register(ToolDefinition(name="fp_tool", execute=_fp))
    check("normalize path->file_path", reg.execute("fp_tool", {"path": "a.py"}) == "fp=a.py")

    # pre hook: short-circuit (cache hit style)
    def _cache_hook(ctx):
        if ctx["name"] == "demo_tool" and ctx["args"].get("x") == "cached":
            return ("allow", "CACHED-VALUE")
        return None
    reg.register_pre_hook(_cache_hook)
    check("pre-hook short-circuit", reg.execute("demo_tool", {"x": "cached"}) == "CACHED-VALUE")
    reg._pre_hooks.remove(_cache_hook)

    # guard: monotonic deny
    def _guard(ctx):
        return "not allowed now" if ctx["name"] == "demo_tool" else None
    reg.register_guard(_guard)
    check("guard blocks", "[BLOCKED]" in reg.execute("demo_tool", {"x": "hi"}))
    reg._guards.remove(_guard)

    # post hook: replace
    def _post(ctx, result, is_error):
        return (result + "!", is_error)
    reg.register_post_hook(_post)
    check("post hook replaces", reg.execute("demo_tool", {"x": "hi"}) == "ran hi!")
    reg._post_hooks.remove(_post)

    # ContextVar scope resolution
    token = active_tool_scope.set("deny-scope")
    check("ContextVar scope applied", reg.execute("demo_tool", {"x": "hi"}).startswith("Error"))
    active_tool_scope.reset(token)

    # ── tools.py integration ──
    import tools
    tools.ensure_tool_registry()

    res = tools.execute_tool("search_code", {"pattern": "def ensure_tool_registry"})
    check("execute_tool routes through registry", "ensure_tool_registry" in str(res))

    # cache short-circuit (second identical call is served by the cache hook)
    res2 = tools.execute_tool("search_code", {"pattern": "def ensure_tool_registry"})
    check("execute_tool cache path", res == res2)

    # unknown tool
    check("unknown via execute_tool", "Unknown tool" in tools.execute_tool("no_such_tool", {}))

    # registry defs match the legacy docs for a known subagent surface
    tools._load_subagents()
    dev_defs = tools.ensure_tool_registry().compact_defs_for("Dev")
    check("Dev scope registered", len(dev_defs) > 5)
    check("doc line preserved", dev_defs.get("run_command", "").startswith("run_command(command,"))

    # orchestrator scope
    from tool_registry import active_tool_scope as ats
    t = ats.set("Orchestrator")
    tools.ensure_tool_registry().restrict_scope("Orchestrator", allow=["task", "update_feature", "ask_user"])
    check("orchestrator sees task", "task" in tools.ensure_tool_registry().names_for())
    check("orchestrator denied write_file", "write_file" not in tools.ensure_tool_registry().names_for())
    ats.reset(t)

    print(f"\nALL {PASS} CHECKS PASSED")


if __name__ == "__main__":
    main()
