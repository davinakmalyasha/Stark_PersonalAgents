import threading
import json
import contextvars

active_store = contextvars.ContextVar("active_store", default=None)
# Optional per-thread state used by delegated workspace agents.  The root run
# continues to use ``shared_state``; a child gets a private snapshot so its
# log/output/token updates cannot overwrite the parent's live chat.
active_state = contextvars.ContextVar("active_state", default=None)

state_lock = threading.Lock()

shared_state = {
    "state_version": 0,
    "active_node": "",
    "client_request": "",
    "outputs": {
        "requirements": "", "tech_spec": "", "code": "", "agent_report": "",
        "test_report": ""
    },
    "project_path": "",
    "plan": {},
    "mcp_servers": [],
    "live_terminal_log": "",
    "selected_model": "Automatic",
    "selected_temp": 0.7,
    "token_usage": {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cache_hit_tokens": 0,
        "total_cache_miss_tokens": 0,
        "total_cost": 0.0,
        "calls": []
    }
}

def _target_state() -> dict:
    """Return the current context's state target."""
    return active_state.get() or shared_state


def safe_get_state() -> dict:
    """Return a thread-safe snapshot of the current run's state."""
    with state_lock:
        current = _target_state()
        copied = current.copy()
        copied["outputs"] = (current.get("outputs") or {}).copy()
        if "token_usage" in current:
            copied["token_usage"] = {
                "total_input_tokens": current["token_usage"].get("total_input_tokens", 0),
                "total_output_tokens": current["token_usage"].get("total_output_tokens", 0),
                "total_cache_hit_tokens": current["token_usage"].get("total_cache_hit_tokens", 0),
                "total_cache_miss_tokens": current["token_usage"].get("total_cache_miss_tokens", 0),
                "total_cost": current["token_usage"].get("total_cost", 0),
                "calls": list(current["token_usage"].get("calls", []))
            }
        if isinstance(current.get("plan"), dict):
            copied["plan"] = {
                k: [dict(it) for it in v] if isinstance(v, list) else v
                for k, v in current["plan"].items()
            }
        if isinstance(current.get("mcp_servers"), list):
            copied["mcp_servers"] = list(current["mcp_servers"])
        if isinstance(current.get("thoughts"), dict):
            copied["thoughts"] = dict(current["thoughts"])
        return copied

def safe_update_state(updates: dict):
    """Safely update the current run's state under the shared lock."""
    with state_lock:
        target = _target_state()
        for k, v in updates.items():
            if k == "outputs" and isinstance(v, dict):
                target.setdefault("outputs", {}).update(v)
            elif k == "token_usage" and isinstance(v, dict):
                previous = target.setdefault("token_usage", {})
                target["token_usage"] = {
                    "total_input_tokens": v.get("total_input_tokens", previous.get("total_input_tokens", 0)),
                    "total_output_tokens": v.get("total_output_tokens", previous.get("total_output_tokens", 0)),
                    "total_cache_hit_tokens": v.get("total_cache_hit_tokens", previous.get("total_cache_hit_tokens", 0)),
                    "total_cache_miss_tokens": v.get("total_cache_miss_tokens", previous.get("total_cache_miss_tokens", 0)),
                    "total_cost": v.get("total_cost", previous.get("total_cost", 0)),
                    "calls": list(v.get("calls", previous.get("calls", [])))
                }
            else:
                target[k] = v
        target["state_version"] = int(target.get("state_version", 0)) + 1

def safe_serialize_state() -> str:
    """Safely serialize the current run's state under the lock."""
    with state_lock:
        return json.dumps(_target_state())


# Cap for live_terminal_log — keeps the SSE snapshots and frontend re-parses bounded
LIVE_LOG_CAP = 20000


def safe_append_live_log(msg: str, newline: bool = True) -> None:
    """Safely appends a message to the live terminal log under lock without copying state.

    Bumps state_version so the SSE streamer in api_server.py emits intermediate
    snapshots while a run is in progress (progressive thinking in the frontend).
    ``newline=False`` appends the message verbatim — used for terminal output
    lines that already carry their own trailing newline.
    """
    with state_lock:
        target = _target_state()
        cur = target.get("live_terminal_log", "")
        text = msg + ("\n" if newline else "")
        if LIVE_LOG_CAP and len(cur) + len(text) > LIVE_LOG_CAP:
            keep = LIVE_LOG_CAP - len(text)
            cur = cur[-keep:] if keep > 0 else ""
        target["live_terminal_log"] = cur + text
        target["state_version"] = int(target.get("state_version", 0)) + 1


def new_scoped_state(overrides: dict | None = None) -> dict:
    """Create an isolated child-run state from the current root snapshot.

    Provider/model settings are retained, while output/log/plan/token fields
    are reset so a child can use the existing agent code without publishing
    its transcript into the parent run.
    """
    state = safe_get_state()
    state.update({
        "state_version": 0,
        "live_terminal_log": "",
        "outputs": {
            "requirements": "", "tech_spec": "", "code": "",
            "agent_report": "", "test_report": "",
        },
        "thoughts": {},
        "plan": {},
        "token_usage": {
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cache_hit_tokens": 0,
            "total_cache_miss_tokens": 0,
            "total_cost": 0.0,
            "calls": [],
        },
    })
    if overrides:
        state.update(overrides)
    return state



def reset_shared_state():
    """Fully reset shared_state to defaults. Use between independent task runs."""
    with state_lock:
        shared_state["active_node"] = ""
        shared_state["client_request"] = ""
        shared_state["outputs"] = {
            "requirements": "", "tech_spec": "", "code": "", "agent_report": "",
            "test_report": ""
        }
        shared_state["project_path"] = ""
        shared_state["plan"] = {}
        shared_state["live_terminal_log"] = ""
        shared_state["token_usage"] = {
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cache_hit_tokens": 0,
            "total_cache_miss_tokens": 0,
            "total_cost": 0.0,
            "calls": []
        }
        shared_state["state_version"] = 0
