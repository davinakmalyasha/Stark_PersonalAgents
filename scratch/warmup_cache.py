"""warmup_cache.py — prime the DeepSeek prefix cache before an eval run.

The first calls of a run pay FULL cache misses (no warm cache). Three tiny
calls with the exact static system prompts (Dev / SDET / Orchestrator) make
the huge shared prefixes cached, so every task's first call hits instead of
missing ~10k tokens. Run once before the parallel eval starts.

Optional arg: a project path — warms the Dev system message for that project
including its cached repo map (Dev's map is the LAST stable section, so the
full message is prefix-warmable). SDET/DevSecOps embed the map after their
tool lists, so their map warms on the project's first real call instead.
"""
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import SystemMessage, HumanMessage


def warm(role, system_text, reasoning_effort="disabled"):
    try:
        from llm import invoke_messages_with_fallback
        invoke_messages_with_fallback(
            role=role,
            messages=[SystemMessage(content=system_text),
                      HumanMessage(content="Reply with the single word: ok")],
            temp=0.1,
            reasoning_effort=reasoning_effort,
        )
        print(f"[WARM] {role} prefix cached (effort={reasoning_effort})")
    except Exception as e:
        print(f"[WARM] {role} failed: {str(e)[:120]}")


def warm_project(project_path):
    """Warm the Dev system message for a project, including its cached repo map."""
    from developer_prompt import _STATIC_SYSTEM_TEMPLATE as dev_prompt
    from memory_manager import load_cached_code_map
    cached_map = load_cached_code_map(project_path)
    if not cached_map:
        print(f"[WARM] project {project_path}: no cached map (refresh_code_map first)")
        return
    try:
        from workspace_manager import get_workspace_rules_and_profile
        from memory_manager import load_cached_project_identity
        stable = [dev_prompt]
        rules_context = get_workspace_rules_and_profile(project_path) or ""
        if rules_context:
            stable.append(f"## Workspace Rules & Profile\n{rules_context}")
        from memory_manager import load_cached_code_map
        cached_map = load_cached_code_map(project_path)
        if cached_map:
            stable.append(f"## Repository Map (cached, append-only)\n{cached_map}")
        project_card = load_cached_project_identity(project_path)
        if project_card:
            stable.append(f"## Project Overview\n{project_card}")
        warm("Developer(project)", "\n\n".join(stable))
    except Exception as e:
        print(f"[WARM] project warm failed: {str(e)[:120]}")


def main():
    from developer_prompt import _STATIC_SYSTEM_TEMPLATE as dev_prompt
    from sdet_prompt import _STATIC_SYSTEM_TEMPLATE as sdet_prompt
    from orchestrator_agent import _ORCHESTRATOR_SYSTEM_TEMPLATE as orch_prompt
    # Warm EVERY reasoning-config bucket the agents actually send: DeepSeek's
    # prefix cache is keyed per thinking config, so a base warmed under
    # "disabled" is invisible to a session starting with "high" — the first
    # wave of iter-1s would otherwise pay full misses.
    for effort in ("disabled", "high"):
        warm("Developer", dev_prompt, effort)
    for effort in ("disabled", "high"):
        warm("SDET", sdet_prompt, effort)
    for effort in ("disabled", "low"):
        warm("Orchestrator", orch_prompt, effort)
    if len(sys.argv) > 1:
        warm_project(sys.argv[1])
    print("[WARM] done")


if __name__ == "__main__":
    main()
