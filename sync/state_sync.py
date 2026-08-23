import threading
import json
import contextvars

active_store = contextvars.ContextVar("active_store", default=None)

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

def safe_get_state() -> dict:
    """Returns a thread-safe shallow-copied snapshot of shared_state."""
    with state_lock:
        copied = shared_state.copy()
        copied["outputs"] = shared_state["outputs"].copy()
        if "token_usage" in shared_state:
            copied["token_usage"] = {
                "total_input_tokens": shared_state["token_usage"]["total_input_tokens"],
                "total_output_tokens": shared_state["token_usage"]["total_output_tokens"],
                "total_cache_hit_tokens": shared_state["token_usage"].get("total_cache_hit_tokens", 0),
                "total_cache_miss_tokens": shared_state["token_usage"].get("total_cache_miss_tokens", 0),
                "total_cost": shared_state["token_usage"]["total_cost"],
                "calls": list(shared_state["token_usage"]["calls"])
            }
        if isinstance(shared_state.get("plan"), dict):
            copied["plan"] = {
                k: [dict(it) for it in v] if isinstance(v, list) else v
                for k, v in shared_state["plan"].items()
            }
        if isinstance(shared_state.get("mcp_servers"), list):
            copied["mcp_servers"] = list(shared_state["mcp_servers"])
        return copied

def safe_update_state(updates: dict):
    """Safely updates shared_state under lock."""
    with state_lock:
        for k, v in updates.items():
            if k == "outputs" and isinstance(v, dict):
                shared_state["outputs"].update(v)
            elif k == "token_usage" and isinstance(v, dict):
                shared_state["token_usage"] = {
                    "total_input_tokens": v.get("total_input_tokens", shared_state["token_usage"]["total_input_tokens"]),
                    "total_output_tokens": v.get("total_output_tokens", shared_state["token_usage"]["total_output_tokens"]),
                    "total_cache_hit_tokens": v.get("total_cache_hit_tokens", shared_state["token_usage"].get("total_cache_hit_tokens", 0)),
                    "total_cache_miss_tokens": v.get("total_cache_miss_tokens", shared_state["token_usage"].get("total_cache_miss_tokens", 0)),
                    "total_cost": v.get("total_cost", shared_state["token_usage"]["total_cost"]),
                    "calls": list(v.get("calls", shared_state["token_usage"]["calls"]))
                }
            else:
                shared_state[k] = v
        shared_state["state_version"] += 1

def safe_serialize_state() -> str:
    """Safely serializes shared_state to a JSON string under lock."""
    with state_lock:
        return json.dumps(shared_state)


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
        cur = shared_state.get("live_terminal_log", "")
        text = msg + ("\n" if newline else "")
        if LIVE_LOG_CAP and len(cur) + len(text) > LIVE_LOG_CAP:
            keep = LIVE_LOG_CAP - len(text)
            cur = cur[-keep:] if keep > 0 else ""
        shared_state["live_terminal_log"] = cur + text
        shared_state["state_version"] += 1



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
