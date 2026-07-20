import os
import json
import re
from datetime import datetime

WORKSPACE_DIR = r"d:\MyProject\LangChain"
TASK_FILE = "task.json"

def load_existing_specs(workspace_path: str = "") -> dict:
    """Reads specs from disk if they exist."""
    ws_dir = workspace_path or WORKSPACE_DIR
    req_path = os.path.join(ws_dir, "1_Requirements.txt")
    spec_path = os.path.join(ws_dir, "2_TechnicalSpec.txt")
    gherkin_path = os.path.join(ws_dir, "1_AcceptanceCriteria.feature")
    mermaid_path = os.path.join(ws_dir, "1_UserFlow.mermaid")
    
    requirements = ""
    if os.path.isfile(req_path):
        with open(req_path, "r", encoding="utf-8") as f:
            requirements = f.read()

    tech_spec = ""
    if os.path.isfile(spec_path):
        with open(spec_path, "r", encoding="utf-8") as f:
            tech_spec = f.read()

    gherkin = ""
    if os.path.isfile(gherkin_path):
        with open(gherkin_path, "r", encoding="utf-8") as f:
            gherkin = f.read()

    mermaid = ""
    if os.path.isfile(mermaid_path):
        with open(mermaid_path, "r", encoding="utf-8") as f:
            mermaid = f.read()
            
    return {
        "requirements": requirements,
        "tech_spec": tech_spec,
        "gherkin": gherkin,
        "mermaid": mermaid,
    }

def load_supervisor_memory(workspace_path: str = "") -> dict:
    """DEPRECATED — memory is in .agent-memory/ now. Returns empty dict."""
    return {}


# ═══════════════════════════════════════════════════════════════════════════════
# Task Tracking — persistent task.json for agent continuity across runs
# ═══════════════════════════════════════════════════════════════════════════════

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


def sync_planning_md_from_task_json(workspace_path: str, task_data: dict) -> None:
    """Synchronizes checkbox states in planning.md on disk with the task progress."""
    # Skip synchronization if we are in the planning phase
    is_planning = task_data.get("user_request", "").strip().startswith("/plan") or any(
        any(kw in s.get("description", "").lower() for kw in ["present plan", "design options", "design choices", "planning.md", "user approval"])
        for s in task_data.get("steps", [])
    )
    if is_planning:
        return

    planning_md_path = os.path.join(workspace_path, "planning.md")
    if not os.path.isfile(planning_md_path) or not task_data.get("steps"):
        return
    try:
        with open(planning_md_path, "r", encoding="utf-8") as f:
            content = f.read()
            
        steps = task_data["steps"]
        lines = content.split("\n")
        updated_lines = []
        step_idx = 0
        
        for line in lines:
            # Match "- [ ] Step description" or "- [x] Step description"
            match = re.match(r"^^(\s*-\s*\[)([ x])(\]\s*)(.*)", line)
            if match and step_idx < len(steps):
                prefix = match.group(1)
                suffix = match.group(3) + match.group(4)
                status = "x" if steps[step_idx].get("status") == "completed" else " "
                updated_lines.append(f"{prefix}{status}{suffix}")
                step_idx += 1
            else:
                updated_lines.append(line)
                
        with open(planning_md_path, "w", encoding="utf-8") as f:
            f.write("\n".join(updated_lines))
    except Exception as e:
        print(f"Error syncing planning.md: {e}")

def save_task_tracking(task_data: dict, workspace_path: str = "", chat_id: str = "") -> bool:
    """Save task.json to central .deep_agents/tasks/ (never inside project folders)."""
    cid = chat_id or task_data.get("chat_id", "")
    if cid and not re.match(r'^[\w\-.]+$', cid):
        raise ValueError(f"Invalid chat_id: {cid}")
    filename = f"task_{cid}.json" if cid else TASK_FILE
    task_path = os.path.join(_tasks_dir(), filename)
    try:
        task_data["updated_at"] = datetime.now().isoformat()
        with open(task_path, "w", encoding="utf-8") as f:
            json.dump(task_data, f, indent=2)

        # Programmatically sync planning.md on disk (still uses project path)
        ws_dir = workspace_path or WORKSPACE_DIR
        try:
            sync_planning_md_from_task_json(ws_dir, task_data)
        except Exception as e:
            print(f"Error syncing planning.md in save_task_tracking: {e}")

        return True
    except Exception as e:
        print(f"Error saving task tracking: {e}")
        return False


def init_task_tracking(user_request: str, steps: list[dict], workspace_path: str = "", chat_id: str = "") -> dict:
    """Create a new task.json entry from supervisor plan steps."""
    import hashlib
    task_id = hashlib.md5(user_request.strip().lower().encode()).hexdigest()[:12]
    task_data = {
        "task_id": task_id,
        "chat_id": chat_id,
        "user_request": user_request,
        "created_at": datetime.now().isoformat(),
        "updated_at": datetime.now().isoformat(),
        "status": "in_progress",
        "current_step": 0,
        "steps": steps,
        "artifacts": {
            "files_created": [],
            "files_modified": [],
            "tests_passing": False,
        },
        "error_history": [],
    }
    save_task_tracking(task_data, workspace_path, chat_id)
    return task_data


def parse_plan_to_steps(plan: str) -> list[dict]:
    """Parse a supervisor markdown checklist into structured step objects.
    Handles multiple checklist items on the same line (e.g., '- [ ] A - [ ] B')."""
    steps = []
    step_id = 0
    for line in plan.split("\n"):
        line_stripped = line.strip()
        if not line_stripped:
            continue
        # Find all checklist items on this line: "- [ ] ..." or "- [x] ..."
        matches = list(re.finditer(r"-\s*\[([ x])\]\s*(.*?)(?=-\s*\[|\s*$)", line_stripped))
        if not matches:
            # Simple match if the split pattern didn't work
            match = re.match(r"-\s*\[([ x])\]\s*(.*)", line_stripped)
            if match:
                matches = [match]
        for match in matches:
            step_id += 1
            status_marker = match.group(1).strip()
            description = match.group(2).strip()
            if description:
                steps.append({
                    "id": step_id,
                    "description": description,
                    "status": "completed" if status_marker == "x" else "pending",
                    "tool_calls": 0,
                    "notes": "",
                    "completed_at": None,
                })
    return steps


def build_task_progress_block(task_data: dict) -> str:
    """Format a task.json into a human-readable progress block for LLM context."""
    steps_data = task_data.get("steps", [])
    if not steps_data:
        return ""
    completed = sum(1 for s in steps_data if s.get("status") == "completed")
    total = len(steps_data)
    current_idx = task_data.get("current_step", 0)

    lines = [f"=== TASK PROGRESS ({completed}/{total} steps) ==="]
    for i, s in enumerate(steps_data):
        marker = {"completed": "[x]", "in_progress": "[/]", "pending": "[ ]", "failed": "[!]"}.get(
            s.get("status", "pending"), "[ ]"
        )
        prefix = ">>>" if i == current_idx and i < total else "   "
        line = f"  {prefix} {marker} Step {s['id']}: {s['description']}"
        if s.get("notes"):
            line += f" — {s['notes'][:150]}"
        if s.get("tool_calls"):
            line += f" [{s['tool_calls']} tool calls]"
        lines.append(line)
    lines.append("=== END TASK PROGRESS ===")
    return "\n".join(lines)
