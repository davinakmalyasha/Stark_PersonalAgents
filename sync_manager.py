"""
Sync Manager — Runs the single-agent graph and streams state updates.
Just plumbing: load workspace context → run agent → stream output.
"""
import os

from state_sync import shared_state, safe_update_state, safe_get_state, state_lock, safe_append_live_log


def run_and_sync_graph(client_request: str, workspace_path: str = "", chat_id: str = "", cancel_event=None):
    """Runs the agentic graph and yields shared_state after each step.
    The caller already resolves "continue" etc. — we just load context and run."""
    from sync_helpers import (
        load_existing_specs, load_supervisor_memory,
        load_task_tracking, build_task_progress_block,
    )

    # Load memory to determine project path
    memory = load_supervisor_memory(workspace_path)
    project_path = workspace_path or memory.get("project_path", "")

    # Probe environment once
    if project_path:
        try:
            from workspace_manager import probe_environment
            probe_environment(project_path)
        except Exception as e:
            print(f"[SYNC] Error probing environment for workspace {project_path}: {e}")

    # Inject task progress block if task.json exists
    resolved_request = client_request
    if project_path:
        try:
            task_data = load_task_tracking(project_path, chat_id)
            if task_data and task_data.get("steps"):
                progress_block = build_task_progress_block(task_data)
                if progress_block and "=== TASK PROGRESS" not in resolved_request:
                    resolved_request = resolved_request + "\n\n" + progress_block
                    print(f"[SYNC] Appended task progress ({task_data.get('current_step', 0)}/{len(task_data['steps'])} steps)")
        except Exception as e:
            print(f"[SYNC] Error appending task progress: {e}")



    # Load existing specs (requirements, tech_spec, etc.)
    specs = load_existing_specs(project_path)

    # Initialize state
    safe_update_state({
        "active_node": "agent",
        "next_agent": "",
        "completed_nodes": [],
        "chat_id": chat_id,
        "thoughts": {
            "agent": "Analyzing request...",
        },
        "client_request": resolved_request,
        "outputs": {
            "requirements": specs.get("requirements", ""),
            "tech_spec": specs.get("tech_spec", ""),
            "code": "",
            "agent_report": "",
            "test_report": "",
            "gherkin": specs.get("gherkin", ""),
            "mermaid": specs.get("mermaid", ""),
        },
        "project_path": project_path,
        "chat_id": chat_id,
        "agents_plan": "",
        "active_tasks": [],
        "deep_agents_log": [],
        "live_terminal_log": f"[>] User: {client_request}\n\n",
        "developer_tool_log": "",
        "code_updated": False,
        "requirements_updated": False,
        "tech_spec_updated": False,
    })
    yield safe_get_state()

    init_state = {
        "client_request": resolved_request,
        "requirements": specs.get("requirements", ""),
        "tech_spec": specs.get("tech_spec", ""),
        "code": "",
        "agent_report": "",
        "test_report": "",
        "error_count": 0,
        "next_agent": "",
        "project_path": project_path,
        "chat_id": chat_id,
        "agents_plan": "",
        "active_tasks": [],
        "requirements_updated": False,
        "tech_spec_updated": False,
        "code_updated": False,
    }

    output_keys = [
        "requirements", "tech_spec", "code", "agent_report", "test_report",
    ]

    try:
        from orchestrator_agent import orchestrator_node
        res = orchestrator_node(init_state)
        if res:
            with state_lock:
                shared_state["active_node"] = "agent"

                for key in output_keys:
                    if key in res:
                        shared_state["outputs"][key] = res[key]

                if "project_path" in res:
                    shared_state["project_path"] = res["project_path"]

                shared_state["failed_models"] = []

                if "code_updated" in res:
                    shared_state["code_updated"] = res["code_updated"]
                if "requirements_updated" in res:
                    shared_state["requirements_updated"] = res["requirements_updated"]
                if "tech_spec_updated" in res:
                    shared_state["tech_spec_updated"] = res["tech_spec_updated"]
                if "error" in res and res["error"]:
                    shared_state["error"] = res["error"]

                nxt = res.get("next_agent", "")
                if nxt:
                    shared_state["next_agent"] = nxt
                if nxt == "finish":
                    shared_state["thoughts"]["agent"] = "Work complete."

                if "agent" not in shared_state["completed_nodes"]:
                    shared_state["completed_nodes"].append("agent")

            yield safe_get_state()
    except Exception as e:
        import traceback
        tb = traceback.format_exc()
        print(f"[SYNC] Error running orchestrator: {e}\n{tb}")
        # Inject error into live_terminal_log so the frontend shows it
        safe_append_live_log(f"\n[SYNC CRASH] {e}\n[ASSISTANT] ERROR: {e}")
        raise e

    # ── Pillar 17: Memory consolidation moved to background daemon ──
    # The inline MemoryArchivist LLM call is REMOVED from the active execution loop.
    # Instead, the background memory_consolidation_daemon (started by api_server.py)
    # periodically reviews recent conversation logs and consolidates facts offline.
    # This saves $0.001/task and 5-15 seconds of user-facing latency per run.
    # Raw logs are still saved to /conversation_history/ for the daemon to process.
    #
    # To force an immediate consolidation (e.g., post-deploy), call:
    #   from memory_consolidation_daemon import consolidate_memories
    #   consolidate_memories()

    # Clear the lazy hydration store to prevent unbounded memory growth across runs
    try:
        from context_compaction import clear_lazy_store
        clear_lazy_store()
    except Exception:
        pass
