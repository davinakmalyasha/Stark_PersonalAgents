"""verify_helper.py — thin wrapper around the agent graph."""
import os


def run_graph_with_verify(prompt, workspace_path, chat_id, cancel_event=None,
                          max_fix_rounds=None, force_orchestrator=False):
    """Run the agent graph and yield snapshots."""
    from sync.sync_manager import run_and_sync_graph
    for snap in run_and_sync_graph(prompt, workspace_path=workspace_path,
                                   chat_id=chat_id, cancel_event=cancel_event,
                                   force_orchestrator=force_orchestrator):
        yield snap
