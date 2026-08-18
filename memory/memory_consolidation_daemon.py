import os
import json
import glob
import re
import gzip
from memory.memory_io import load_memory, save_memory
from llm import invoke_with_fallback
from pydantic import BaseModel, Field

_HARNESS_ROOT = os.path.dirname(os.path.abspath(__file__))
_HISTORY_DIR = os.path.join(_HARNESS_ROOT, ".deep_agents", "conversation_history")


class ConsolidatedMemory(BaseModel):
    lessons_learned: list[str] = Field(default_factory=list, description="New coding/debugging lessons learned or pitfalls to avoid.")
    past_requests: list[str] = Field(default_factory=list, description="Client feature requests completed in this iteration.")
    new_technologies: list[str] = Field(default_factory=list, description="Specific frameworks, tools, or libraries used.")


def _recent_history_text(limit_bytes=60000, max_files=30):
    """Read the most recent plain-text conversation-history logs (the real
    session content, unlike the msgpack checkpoint blobs)."""
    if not os.path.isdir(_HISTORY_DIR):
        return []
    files = sorted(glob.glob(os.path.join(_HISTORY_DIR, "*.txt")),
                   key=os.path.getmtime, reverse=True)[:max_files]
    texts = []
    total = 0
    for f in files:
        try:
            with open(f, encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except Exception:
            continue
        if not text.strip():
            continue
        if total + len(text) > limit_bytes:
            break
        texts.append(text)
        total += len(text)
    return texts


def _extract_workspaces(texts):
    """Discover workspace roots: prefer the app's registered workspaces whose
    paths appear in the session logs; fall back to absolute dirs found in the
    logs that look like project roots (contain source/config files)."""
    registered = []
    try:
        from workspace_manager import list_workspaces
        for w in list_workspaces() or []:
            p = (w.get("path") or "").strip()
            if p and os.path.isdir(p) and not p.lower().endswith("_general"):
                registered.append(os.path.normpath(p))
    except Exception:
        pass
    if registered:
        found = []
        for t in texts:
            low = t.lower()
            for r in registered:
                if r.lower() in low and r not in found:
                    found.append(r)
        if found:
            return found
    # fallback heuristic: dirs from absolute file paths that look like projects
    ws = set()
    for t in texts:
        for m in re.finditer(r"[A-Za-z]:[\\/][^\s\"']+", t):
            p = m.group(0).replace("/", os.sep)
            if not (os.path.isabs(p) and os.path.isfile(p)):
                continue
            d = os.path.dirname(p)
            if os.path.isdir(d) and any(
                os.path.isfile(os.path.join(d, c)) for c in
                ("package.json", "pyproject.toml", "requirements.txt", "go.mod")):
                ws.add(os.path.normpath(d))
    return list(ws)


def _write_project_memory(workspace, lessons, requests, techs):
    """Write consolidated memory into the project's .agent-memory so the
    agents actually READ it (this was the missing half of the pipeline)."""
    mem_dir = os.path.join(workspace, ".agent-memory")
    try:
        os.makedirs(mem_dir, exist_ok=True)
        # MEMORY.md is RETIRED — LLM-written "lessons learned" from chat logs
        # were the larping-risky kind of memory. Its role is replaced by the
        # verified failure→fix pattern library (error_patterns.py), which only
        # stores evidence-backed lessons (hidden-test-verified diffs).
        # features.md entry via the standard writer (merged, not overwritten)
        try:
            from memory.memory_manager import update_feature
            from datetime import datetime
            stamp = datetime.now().strftime("%Y-%m-%d %H:%M")
            summary = "; ".join(lessons[:3]) if lessons else "session consolidated"
            update_feature(workspace, f"Consolidated {stamp}", summary[:300])
        except Exception:
            pass
        return True
    except Exception as e:
        print(f"[MEMORY] project write failed for {workspace}: {e}")
        return False


def consolidate_memories():
    print("Starting background memory consolidation...")

    # 1. Read the REAL session content (plain-text history logs)
    texts = _recent_history_text()
    if not texts:
        print("No conversation history found to consolidate.")
        return
    workspaces = _extract_workspaces(texts)
    print(f"[MEMORY] {len(texts)} history logs, {len(workspaces)} workspace(s) detected")

    # 2. LLM consolidation
    sys_inst = (
        "You are the Background Memory Consolidation Agent. Audit the recent "
        "conversation histories and extract new lessons learned, completed "
        "requests, and new technologies introduced. Be specific and concise; "
        "skip trivial or repetitive entries."
    )
    prompt = "Recent Conversations:\n" + "\n---\n".join(texts[:15])

    try:
        res: ConsolidatedMemory = invoke_with_fallback(
            role="MemoryConsolidation",
            sys_inst=sys_inst,
            prompt=prompt,
            schema=ConsolidatedMemory,
            temp=0.1,
        )
    except Exception as e:
        print(f"Memory consolidation LLM call failed: {e}")
        return

    # 3. Global store (backward compatible)
    try:
        current_mem = load_memory()
        lessons = current_mem.setdefault("lessons_learned", [])
        for l in res.lessons_learned:
            if l not in lessons:
                lessons.append(l)
        requests = current_mem.setdefault("past_requests", [])
        for r in res.past_requests:
            if r not in requests:
                requests.append(r)
        it = current_mem.setdefault("it_department", {})
        techs = it.setdefault("technologies", [])
        for t in res.new_technologies:
            if t not in techs:
                techs.append(t)
        save_memory(current_mem)
    except Exception as e:
        print(f"[MEMORY] global store write failed: {e}")

    # 4. WRITE to each detected workspace's .agent-memory — the missing half
    written = 0
    for ws in workspaces:
        if _write_project_memory(ws, res.lessons_learned, res.past_requests,
                                 res.new_technologies):
            written += 1
    print(f"[MEMORY] consolidation complete: {len(res.lessons_learned)} lessons, "
          f"{len(res.past_requests)} requests, {len(res.new_technologies)} techs, "
          f"written to {written} workspace(s).")


if __name__ == "__main__":
    consolidate_memories()
