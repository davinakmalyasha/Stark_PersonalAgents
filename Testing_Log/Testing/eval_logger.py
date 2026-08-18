import os
import json
import time
from typing import Optional, Dict, Any

# ── File paths ──────────────────────────────────────────────────────────────

def _project_dir() -> str:
    return os.path.dirname(os.path.abspath(__file__))

def _calls_log_path() -> str:
    """Append-only JSONL file for per-LLM-call records. No read-modify-write races."""
    log_dir = os.path.join(_project_dir(), ".deep_agents")
    return os.path.join(log_dir, "gen5_eval_calls.jsonl")

def _tasks_log_path() -> str:
    """Summary JSON for per-task results. Written once per task, no race with calls."""
    log_dir = os.path.join(_project_dir(), ".deep_agents")
    return os.path.join(log_dir, "gen5_eval_tasks.json")

# ── Initialization ──────────────────────────────────────────────────────────

def initialize_eval_logs() -> None:
    """Create log directory if needed."""
    for p in [_calls_log_path(), _tasks_log_path()]:
        os.makedirs(os.path.dirname(p), exist_ok=True)
    # Initialize tasks file if missing
    tasks_path = _tasks_log_path()
    if not os.path.exists(tasks_path):
        _write_tasks_file({"tasks": [], "overall_success_rate": "0/0 (0%)",
                           "total_time_seconds": 0.0, "total_cost_usd": 0.0})

def _write_tasks_file(data: dict) -> None:
    """Atomic write for tasks file (no concurrent writers)."""
    tmp = _tasks_log_path() + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2)
    os.replace(tmp, _tasks_log_path())  # Atomic on Windows/Linux

# ── Append-Only Call Logger (Fixes race with task results) ──────────────────

def save_to_gen5_log(role: str, model_used: str, input_tokens: int,
                     output_tokens: int, cache_hit_tokens: int,
                     where: str, reasoning_chars: int = 0,
                     reasoning_effort: Optional[str] = None,
                     reasoning_budget: Optional[int] = None) -> None:
    """
    Pillar 45: Append a single LLM call record to the eval log.
    Uses JSONL (one JSON object per line, append-only).

    This eliminates the read-modify-write race that caused 90% of calls
    (Supervisor, Developer, Tester) to be lost when save_task_result_incremental
    overwrote the shared JSON file.
    """
    from llm.llm_stats import calculate_cost

    calls_path = _calls_log_path()
    os.makedirs(os.path.dirname(calls_path), exist_ok=True)

    cost = calculate_cost(model_used, input_tokens, output_tokens, cache_hit_tokens)
    cache_miss_tokens = max(0, input_tokens - cache_hit_tokens)

    # Truncate long 'where' descriptions
    if len(where) > 120:
        where = where[:117] + "..."

    record = {
        "agent": role,
        "model": model_used,
        "input": input_tokens,
        "output": output_tokens,
        "cache_hits": cache_hit_tokens,
        "cache_misses": cache_miss_tokens,
        "cost": round(cost, 6),
        "timestamp": time.strftime("%H:%M:%S"),
        "where": where,
        "reasoning_chars": reasoning_chars if reasoning_chars is not None else 0,
        "reasoning_effort": reasoning_effort,
        "reasoning_budget": reasoning_budget,
    }

    # Append-only: open, write one line, close. No read step. No races.
    for attempt in range(3):
        try:
            with open(calls_path, "a", encoding="utf-8") as f:
                f.write(json.dumps(record, ensure_ascii=False) + "\n")
            break
        except IOError:
            time.sleep(0.05)

# ── Task Result Logger (Separate file, no contention with calls) ────────────

def save_task_result_incremental(res: Dict[str, Any]) -> None:
    """
    Save per-task result to the tasks summary file.
    Since this writes to a DIFFERENT file than save_to_gen5_log,
    there's no race condition between call logging and task logging.
    """
    tasks_path = _tasks_log_path()
    os.makedirs(os.path.dirname(tasks_path), exist_ok=True)

    if not os.path.exists(tasks_path):
        initialize_eval_logs()

    # Read current tasks (only this function writes to this file)
    for attempt in range(3):
        try:
            with open(tasks_path, "r", encoding="utf-8") as f:
                data = json.load(f)
            if not isinstance(data, dict):
                data = {}
            break
        except (IOError, json.JSONDecodeError):
            time.sleep(0.05)
    else:
        data = {}

    if "tasks" not in data or not isinstance(data["tasks"], list):
        data["tasks"] = []

    # Build clean task result
    item = res.copy()
    stdout = res.get("verify_stdout", "")
    if stdout:
        item["verify_stdout"] = stdout[-1500:] if len(stdout) > 1500 else stdout
    else:
        item.pop("verify_stdout", None)

    stderr = res.get("verify_stderr", "")
    if stderr:
        item["verify_stderr"] = stderr[-1500:] if len(stderr) > 1500 else stderr
    else:
        item.pop("verify_stderr", None)

    # Upsert by task ID
    existing_idx = next((i for i, t in enumerate(data["tasks"]) if t.get("id") == item.get("id")), -1)
    if existing_idx >= 0:
        data["tasks"][existing_idx] = item
    else:
        data["tasks"].append(item)

    # Compute aggregates
    completed = data["tasks"]
    passed = sum(1 for t in completed if t.get("status") == "PASS")
    total = len(completed)
    total_time = sum(t.get("time_elapsed", 0.0) for t in completed)
    total_cost = sum(t.get("cost", 0.0) for t in completed)

    data["overall_success_rate"] = f"{passed}/{total} ({passed/total*100:.1f}%)" if total > 0 else "0/0 (0%)"
    data["total_time_seconds"] = round(total_time, 2)
    data["total_cost_usd"] = round(total_cost, 6)

    _write_tasks_file(data)

# ── Read / Aggregate (for post-run analysis) ────────────────────────────────

def read_all_calls() -> list[dict]:
    """Read all LLM call records from the append-only JSONL file."""
    calls_path = _calls_log_path()
    if not os.path.exists(calls_path):
        return []
    calls = []
    with open(calls_path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    calls.append(json.loads(line))
                except json.JSONDecodeError:
                    pass
    return calls

def aggregate_calls(calls: list[dict]) -> dict:
    """Compute totals from a list of call records."""
    totals = {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cache_hit_tokens": 0,
        "total_cache_miss_tokens": 0,
        "total_cost": 0.0,
        "calls": calls,
    }
    for c in calls:
        totals["total_input_tokens"] += c.get("input", 0)
        totals["total_output_tokens"] += c.get("output", 0)
        totals["total_cache_hit_tokens"] += c.get("cache_hits", 0)
        totals["total_cache_miss_tokens"] += c.get("cache_misses", 0)
        totals["total_cost"] = round(totals["total_cost"] + c.get("cost", 0), 6)
    return totals

def read_eval_summary() -> dict:
    """Read the complete eval state: aggregated calls + task results."""
    calls = read_all_calls()
    call_totals = aggregate_calls(calls)

    tasks_path = _tasks_log_path()
    tasks_data = {}
    if os.path.exists(tasks_path):
        try:
            with open(tasks_path, "r", encoding="utf-8") as f:
                tasks_data = json.load(f)
        except (IOError, json.JSONDecodeError):
            pass

    return {
        **call_totals,
        "tasks": tasks_data.get("tasks", []),
        "overall_success_rate": tasks_data.get("overall_success_rate", "0/0 (0%)"),
        "total_time_seconds": tasks_data.get("total_time_seconds", 0.0),
        "total_cost_usd": tasks_data.get("total_cost_usd", 0.0),
    }

# ── Legacy compatibility ────────────────────────────────────────────────────

def get_log_path() -> str:
    """Legacy path — returns calls log for backward compat."""
    return _calls_log_path()

def initialize_gen5_log() -> None:
    """Legacy init — delegates to new initialization."""
    initialize_eval_logs()
