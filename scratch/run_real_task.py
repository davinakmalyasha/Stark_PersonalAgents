"""Drive a real orchestrator task through the harness end-to-end (free BSA +
effort-ladder Developer) and save the resulting token usage for before/after
analysis. Safe: runs against a throwaway temp project, never the real repo.

Usage:
    BSA_MAX_SECTIONS=4 python scratch/run_real_task.py "your task here"
Output: scratch/measure_run_usage.json  (token_usage dict from shared_state)
"""
import json
import os
import shutil
import sys
import tempfile

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

_OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "measure_run_usage.json")


def _make_app(root: str) -> None:
    """A tiny existing Express app so BSA runs in FEATURE mode (research reads files)."""
    app = os.path.join(root, "app")
    os.makedirs(app, exist_ok=True)
    with open(os.path.join(app, "server.js"), "w", encoding="utf-8") as f:
        f.write(
            "const express = require('express');\n"
            "const app = express();\n"
            "app.use(express.json());\n"
            "// GET /api/users returns a hardcoded list\n"
            "app.get('/api/users', (req, res) => res.json([{ id: 1, name: 'alice' }]));\n"
            "module.exports = app;\n"
        )
    with open(os.path.join(app, "users.js"), "w", encoding="utf-8") as f:
        f.write("// User repository — currently in-memory array.\n"
                "const users = [{ id: 1, name: 'alice', email: 'alice@example.com' }];\n"
                "module.exports = { users };\n")
    with open(os.path.join(app, "package.json"), "w", encoding="utf-8") as f:
        f.write('{ "name": "demo-app", "dependencies": { "express": "^4.19.0" } }\n')


def main():
    task = (
        sys.argv[1]
        if len(sys.argv) > 1
        else "Design and implement a new feature: a user profile page with an edit form "
             "so a user can update their name and email. Write the specification first, "
             "then implement it in the existing app."
    )
    tmp = tempfile.mkdtemp(prefix="bsa_measure_")
    try:
        _make_app(tmp)
        print(f"[DRIVER] task={task[:80]!r} workspace={tmp}", flush=True)

        from sync_manager import run_and_sync_graph

        step = 0
        for state in run_and_sync_graph(task, workspace_path=tmp, chat_id="measure-run"):
            step += 1
            active = state.get("active_node", "?")
            if step == 1 or step % 5 == 0:
                print(f"[DRIVER] step {step}: {active}", flush=True)

        from state_sync import safe_get_state
        usage = safe_get_state().get("token_usage", {})
        with open(_OUT, "w", encoding="utf-8") as f:
            json.dump(usage, f, indent=1, default=str)
        calls = usage.get("calls", [])
        print(f"\n[DRIVER] token_usage -> {_OUT}")
        print(f"[DRIVER] {len(calls)} calls | "
              f"output {usage.get('total_output_tokens', 0):,} | "
              f"input {usage.get('total_input_tokens', 0):,} | "
              f"cost ${float(usage.get('total_cost', 0)):.4f}")

        specs = os.path.join(tmp, "specs")
        if os.path.isdir(specs):
            print("[DRIVER] specs:", os.listdir(specs))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        print(f"[DRIVER] cleaned up {tmp}")


if __name__ == "__main__":
    main()
