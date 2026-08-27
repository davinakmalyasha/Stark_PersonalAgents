"""STARK LIVE SMOKE — run LATER when API credits are available.

Exercises the full reworked orchestrator with a REAL LLM (OpenCode Go route)
against the boss's literal multi-intent sentence:

    "i want you to create 2 project randomly, 1 game and 1 web, also can you
     turn off the lamp in bedroom, and note today i eat fried rice"

Expected behavior under the Stark doctrine:
  * 2 projects created via create_project (background first turns)
  * House delegated ("lamp off"), General delegated ("note lunch")
  * 4 visible chats created across workspaces
  * Park-and-wait collects [DELEGATION RESULT] events
  * Final reply: short, plain-language, boss-style

USAGE (when ready):
    venv312\\Scripts\\python.exe scratch\\bench_stark_live.py

Requires OPENCODE_API_KEY in .env (or environment). Cost: flash-tier,
roughly a few cents. Everything writes ONLY inside scratch/_stark_live/.
"""

import io
import json
import os
import sys
import time

_ROOT = r"D:\MyProject\LangChain"
sys.path.insert(0, _ROOT)
sys.path.insert(0, os.path.join(_ROOT, "Desktop"))

# dotenv FIRST so OPENCODE_API_KEY is visible before any provider resolution.
from dotenv import load_dotenv  # noqa: E402
load_dotenv(os.path.join(_ROOT, ".env"))

LIVE_WS = os.path.join(_ROOT, "scratch", "_stark_live")

PROMPT = (
    "i want you to create 2 project randomly, 1 game and 1 web, also can you "
    "turn off the lamp in bedroom, and note today i eat fried rice"
)


def _isolate() -> None:
    """Point every persistent surface into scratch/_stark_live."""
    import agents.project_router as pr
    pr.PROJECTS_ROOT = LIVE_WS
    pr.INDEX_PATH = os.path.join(LIVE_WS, "project_agents.json")
    os.makedirs(LIVE_WS, exist_ok=True)

    # Redirect the General workspace chat dir so delegation chats don't touch
    # the real _general history.
    import workspace_manager as wm
    gen_dir = os.path.join(LIVE_WS, "chats", "_general")
    os.makedirs(gen_dir, exist_ok=True)
    wm.GENERAL_CHAT_DIR = gen_dir


def _check(label: str, ok: bool, detail: str = "") -> bool:
    icon = "[PASS]" if ok else "[FAIL]"
    print(f"{icon} {label}" + (f" — {detail}" if detail else ""))
    return ok


def main() -> int:
    if not os.environ.get("OPENCODE_API_KEY"):
        print("[ABORT] OPENCODE_API_KEY not set — top up / configure first.")
        return 1

    _isolate()

    from agents import delegation
    from agents.orchestrator_agent import orchestrator_node
    from sync.state_sync import shared_state

    # Route ALL LLM traffic through the OpenCode Go provider (DeepSeek direct
    # is out of budget). Must be set BEFORE the first invoke resolves a model.
    shared_state["llm_provider"] = "opencode"
    shared_state["chat_id"] = "stark-live"
    shared_state.setdefault("thoughts", {})
    shared_state.setdefault("outputs", {})
    delegation.reset_delegations()

    t0 = time.time()
    res = orchestrator_node({
        "client_request": PROMPT,
        "project_path": LIVE_WS,
        "chat_id": "stark-live",
    })
    elapsed = time.time() - t0

    print("\n" + "=" * 70)
    print(f"STARK REPLY ({elapsed:.0f}s):\n{res.get('agent_report', '')}")
    print("=" * 70)

    print(f"\n{delegation.status_table()}")

    try:
        with open(pr_index := os.path.join(LIVE_WS, "project_agents.json"), "r", encoding="utf-8") as f:
            projects = json.load(f).get("projects", [])
    except Exception:
        projects = []
    print(f"\nProjects registered: {[p.get('name') for p in projects]}")

    usage = shared_state.get("token_usage", {}) or {}
    print(
        f"Tokens: in={usage.get('total_input_tokens', 0):,} "
        f"out={usage.get('total_output_tokens', 0):,} "
        f"cache_hit={usage.get('total_cache_hit_tokens', 0):,} "
        f"cost=${usage.get('total_cost', 0.0):.4f}"
    )

    ok = True
    ok &= _check(">=2 projects registered", len(projects) >= 2)
    kinds = {e["agent_key"] for e in delegation._DELEGATIONS.values()}
    ok &= _check("House delegated", "house" in kinds)
    ok &= _check("General delegated", "general" in kinds)
    n_chats = sum(1 for e in delegation._DELEGATIONS.values()) + len(projects)
    ok &= _check(">=4 job chats total", n_chats >= 4, f"count={n_chats}")
    failed = [e for e in delegation._DELEGATIONS.values() if e["status"] == "failed"]
    ok &= _check("no crashed delegations", not failed,
                 ", ".join(e["id"] for e in failed))
    report = res.get("agent_report", "")
    ok &= _check("reply is compact (<1200 chars)", 0 < len(report) < 1200,
                 f"len={len(report)}")

    print("\nRESULT:", "SMOKE PASSED" if ok else "SMOKE HAD FAILURES (see above)")
    return 0 if ok else 2


if __name__ == "__main__":
    raise SystemExit(main())
