"""warmup_cache.py — prime the DeepSeek prefix cache before an eval run.

The first calls of a run pay FULL cache misses (no warm cache). Three tiny
calls with the exact static system prompts (Dev / SDET / Orchestrator) make
the huge shared prefixes cached, so every task's first call hits instead of
missing ~10k tokens. Run once before the parallel eval starts.

Optional arg: a project path — warms the Dev system message for that project
using the same prompt builder that developer_agent uses at runtime, so the
cached prefix is byte-for-byte identical to what real tasks will send.
"""
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import SystemMessage, HumanMessage


def warm(role: str, system_text: str, reasoning_effort: str = "disabled") -> None:
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


def warm_project(project_path: str) -> None:
    """Warm the Dev system message for a project.

    Uses developer_agent._build_system_prompt() directly so the warmed prefix
    is byte-for-byte identical to what the real Developer agent sends on its
    first turn. Any layout drift in the agent is automatically reflected here.
    """
    try:
        from agents.developer_agent import _build_system_prompt
        static_system, _ = _build_system_prompt(
            task="",
            project_path=project_path,
            is_first_call=True,
        )
        if not static_system:
            print(f"[WARM] project {project_path}: empty system prompt (no stable identity yet?)")
            return
        warm("Developer(project)", static_system)
    except Exception as e:
        print(f"[WARM] project warm failed: {str(e)[:120]}")


def main() -> None:
    from agents.developer_prompt import _STATIC_SYSTEM_TEMPLATE as dev_prompt
    from agents.orchestrator_agent import _ORCHESTRATOR_SYSTEM_TEMPLATE as orch_prompt

    # Warm EVERY reasoning-config bucket the agents actually send: DeepSeek's
    # prefix cache is keyed per thinking config, so a base warmed under
    # "disabled" is invisible to a session starting with "high" — the first
    # wave of iter-1s would otherwise pay full misses.
    for effort in ("disabled", "high"):
        warm("Developer", dev_prompt, effort)
    for effort in ("disabled", "low"):
        warm("Orchestrator", orch_prompt, effort)

    if len(sys.argv) > 1:
        warm_project(sys.argv[1])

    print("[WARM] done")


if __name__ == "__main__":
    main()
