"""
Sync Manager — Runs the single-agent graph and streams state updates.
Just plumbing: load workspace context → run agent → stream output.
"""
import os

from sync.state_sync import shared_state, safe_update_state, safe_get_state, state_lock, safe_append_live_log


def run_and_sync_graph(client_request: str, workspace_path: str = "", chat_id: str = "", cancel_event=None):
    """Runs the agentic graph and yields shared_state after each step.
    The caller already resolves "continue" etc. — we just load context and run."""
    from sync.sync_helpers import (
        load_existing_specs,
        load_task_tracking, build_plan_progress_block,
    )

    # ── P9: direct-to-Dev routing — messages bound for an existing project's
    # Dev agent never touch the orchestrator (zero relay tokens). New project
    # creation is NOT decided here — that stays with the orchestrator LLM.
    try:
        from agents.project_router import route_request, stream_direct
        route_kind, project = route_request(client_request, workspace_path, chat_id)
        if route_kind == "project_dev" and project:
            yield from stream_direct(project, client_request, workspace_path, chat_id)
            return
    except Exception as e:
        print(f"[SYNC] Project routing error (falling back to orchestrator): {e}")

    # Project path comes from the caller (workspace) — the legacy supervisor-memory
    # loader was removed (it always returned {}).
    project_path = workspace_path

    # Probe environment once
    if project_path:
        try:
            from workspace_manager import probe_environment
            probe_environment(project_path)
        except Exception as e:
            print(f"[SYNC] Error probing environment for workspace {project_path}: {e}")

    # Inject the current plan block if task.json has one (LLM-driven update_plan)
    resolved_request = client_request
    if project_path:
        try:
            task_data = load_task_tracking(project_path, chat_id)
            # A "completed" task file is stale for a NEW request (the orchestrator must
            # re-evaluate it, not trust the old completion) — skip the plan block
            # or the orchestrator may skip delegation entirely.
            if task_data and task_data.get("plan") and task_data.get("status") != "completed":
                plan_block = build_plan_progress_block(task_data)
                if plan_block and "=== CURRENT PLAN" not in resolved_request:
                    resolved_request = resolved_request + "\n\n" + plan_block
                    print(f"[SYNC] Appended plan block ({len(task_data.get('plan', {}))} scope(s))")
        except Exception as e:
            print(f"[SYNC] Error appending plan block: {e}")



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
        from agents.orchestrator_agent import orchestrator_node
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

    # Clear the lazy hydration store to prevent unbounded memory growth across runs
    try:
        from memory.context_compaction import clear_lazy_store
        clear_lazy_store()
    except Exception:
        pass
