"""verify_helper.py — thin wrapper around the agent graph + GitHub gate handoff.

The local harness verification gate (adaptive_verify, repair rounds, smoke
checks) has been REMOVED. Dev verifies its own work naturally (feature tests),
and delivered code is pushed to GitHub by the harness, where the tiered
gate (fast-checks / staging / release-heavy workflows) runs. Failures are steered back to Dev
automatically by ci_gate — see ci_gate.py.
"""
import os


def run_graph_with_verify(prompt, workspace_path, chat_id, cancel_event=None,
                          max_fix_rounds=None):
    """Run the agent graph, then hand delivered code to the GitHub gate.

    Yields every graph state snapshot as before. After the graph completes,
    ci_gate.after_run pushes real changes (if any) and starts the background
    gate watch — fire-and-forget, never blocks the response. The last yielded
    snapshot is the final graph state (no verify_status anymore).
    """
    from sync.sync_manager import run_and_sync_graph
    for snap in run_and_sync_graph(prompt, workspace_path=workspace_path,
                                   chat_id=chat_id, cancel_event=cancel_event):
        yield snap
    try:
        from ci_gate import after_run
        after_run(workspace_path, prompt, chat_id)
    except Exception as e:
        print(f"[verify_helper] push/watch error (non-fatal): {e}")
