import os
import json
import re
from datetime import datetime

WORKSPACE_DIR = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
TASK_FILE = "task.json"

def load_existing_specs(workspace_path: str = "") -> dict:
    """Reads current pipeline specs from disk: specs/SPEC-*.md and DECISION-*.md."""
    import glob
    ws_dir = workspace_path or WORKSPACE_DIR
    result = {"requirements": "", "tech_spec": "", "gherkin": "", "mermaid": ""}
    specs_dir = os.path.join(ws_dir, "specs")
    if os.path.isdir(specs_dir):
        for p in sorted(glob.glob(os.path.join(specs_dir, "SPEC-*.md"))):
            try:
                with open(p, "r", encoding="utf-8") as f:
                    result["tech_spec"] += f"\n\n### {os.path.basename(p)}\n" + f.read()
            except Exception:
                pass
        for p in sorted(glob.glob(os.path.join(specs_dir, "DECISION-*.md"))):
            try:
                with open(p, "r", encoding="utf-8") as f:
                    result["requirements"] += f"\n\n### {os.path.basename(p)}\n" + f.read()
            except Exception:
                pass
    return result


# ═══════════════════════════════════════════════════════════════════════════════
# Task Tracking — persistent task.json for agent continuity across runs
#
# The plan is LLM-driven (codex concept): the model writes the entire plan array
# on every update_plan call (full replacement). The harness only persists it,
# re-injects it on resume, and mirrors it into UI state. No harness-side step
# machine, no parsing of planning.md.
# ═══════════════════════════════════════════════════════════════════════════════

PLAN_STATUSES = ("pending", "in_progress", "completed")
MAX_STEP_CHARS = 200


def _tasks_dir() -> str:
    """Centralized tasks directory — never inside a project folder."""
    d = os.path.join(WORKSPACE_DIR, ".deep_agents", "tasks")
    os.makedirs(d, exist_ok=True)
    return d


def load_task_tracking(workspace_path: str = "", chat_id: str = "") -> dict | None:
    """Load the task.json from central .deep_agents/tasks/. Returns None if not found."""
    if chat_id and not re.match(r'^[\w\-.]+$', chat_id):
        raise ValueError(f"Invalid chat_id: {chat_id}")
    filename = f"task_{chat_id}.json" if chat_id else TASK_FILE
    task_path = os.path.join(_tasks_dir(), filename)
    if os.path.isfile(task_path):
        try:
            with open(task_path, "r", encoding="utf-8") as f:
                return json.load(f)
        except Exception:
            pass
    return None


def save_task_tracking(task_data: dict, workspace_path: str = "", chat_id: str = "") -> bool:
    """Save task.json to central .deep_agents/tasks/ (never inside project folders)."""
    cid = chat_id or task_data.get("chat_id", "")
    if cid and not re.match(r'^[\w\-.]+$', cid):
        raise ValueError(f"Invalid chat_id: {cid}")
    filename = f"task_{cid}.json" if cid else TASK_FILE
    task_path = os.path.join(_tasks_dir(), filename)
    try:
        task_data["updated_at"] = datetime.now().isoformat()
        # Atomic write: readers (plan dock, agent re-injection, parallel job
        # tracker syncs) must never observe a torn/truncated JSON file.
        tmp_path = task_path + ".tmp"
        with open(tmp_path, "w", encoding="utf-8") as f:
            json.dump(task_data, f, indent=2)
        os.replace(tmp_path, task_path)
        return True
    except Exception as e:
        print(f"Error saving task tracking: {e}")
        return False


# ── Codex-style plan store ────────────────────────────────────────────────────

def plan_state(workspace_path: str = "", chat_id: str = "") -> dict:
    """Plans per scope, e.g. {"developer": [{step, status}, ...]}."""
    task_data = load_task_tracking(workspace_path, chat_id)
    if not task_data:
        return {}
    plans = task_data.get("plan")
    return plans if isinstance(plans, dict) else {}


def load_plan(workspace_path: str = "", chat_id: str = "", scope: str = "") -> list[dict]:
    """Load the plan items for a scope. Returns [] when absent.

    With no scope, returns the first non-empty plan found (display fallback).
    """
    plans = plan_state(workspace_path, chat_id)
    if not plans:
        return []
    scope_key = (scope or "").lower()
    items = plans.get(scope_key, []) if scope_key else (next(iter(plans.values()), []) or [])
    return [{"step": str(it.get("step", "")), "status": it.get("status", "pending")}
            for it in items if isinstance(it, dict) and it.get("step")]


def save_plan(plan: list[dict], workspace_path: str = "", chat_id: str = "", scope: str = "") -> dict:
    """Persist a codex-style plan for a scope into task.json (full replacement)."""
    task_data = load_task_tracking(workspace_path, chat_id) or {
        "task_id": f"auto_{chat_id}",
        "chat_id": chat_id,
        "user_request": "",
        "created_at": datetime.now().isoformat(),
        "status": "in_progress",
        "artifacts": {"files_created": [], "files_modified": [], "tests_passing": False},
    }
    plans = task_data.setdefault("plan", {})
    plans[(scope or "agent").lower()] = list(plan)
    save_task_tracking(task_data, workspace_path, chat_id)
    return task_data


def build_plan_progress_block(task_data: dict, scope: str = "") -> str:
    """Format the codex-style plan into a compact progress block for LLM context."""
    plans = task_data.get("plan")
    if not isinstance(plans, dict) or not plans:
        return ""
    scope_key = (scope or "").lower()
    items = plans.get(scope_key, []) if scope_key else (next(iter(plans.values()), []) or [])
    if not items:
        return ""
    completed = sum(1 for it in items if isinstance(it, dict) and it.get("status") == "completed")
    total = len(items)
    lines = [f"=== CURRENT PLAN ({completed}/{total} completed) ==="]
    for i, it in enumerate(items, 1):
        status = it.get("status", "pending")
        marker = {"completed": "[x]", "in_progress": "[/]", "pending": "[ ]"}.get(status, "[ ]")
        lines.append(f"  {i}. {marker} {str(it.get('step', ''))[:MAX_STEP_CHARS]}")
    lines.append("=== END PLAN ===")
    return "\n".join(lines)
