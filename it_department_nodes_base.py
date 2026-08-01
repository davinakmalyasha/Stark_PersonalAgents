"""
IT Department Node Definitions — Base agents: BA, SA, DevOps, Analytics.
BA/SA use Word automation. DevOps uses agy CLI. Analytics uses Word.

Covers:
  Pillar 118: Hierarchical Swarm Message Compression
"""
import os
import json
from typing import TypedDict
from state_sync import shared_state, safe_append_live_log

# ── Pillar 118: Hierarchical Swarm Message Compression ──

def compress_swarm_output(agent_name: str, raw_output: str) -> str:
    """
    Compress a subagent's raw output into a structured record.
    Prevents internal agent-to-agent chatter from bloating the supervisor's context.

    Returns a compressed markdown block with summary, files_written, key_decisions, errors.
    """
    if not raw_output or len(raw_output) < 500:
        return raw_output  # Keep short outputs intact

    # Build structured summary
    lines = raw_output.splitlines()
    preview = "\n".join(lines[:15])

    # Detect errors
    errors: list[str] = []
    error_keywords = ["error", "failed", "traceback", "exception", "STATUS: FAIL", "exit code: 1"]
    for line in lines:
        if any(kw in line.lower() for kw in error_keywords):
            errors.append(line.strip()[:200])
            if len(errors) >= 5:
                break

    # Detect file paths written
    files_written: list[str] = []
    import re
    for line in lines:
        m = re.search(r"(?:wrote|created|saved|written|generated)\s+(?:to\s+)?([/\w.\\-]+(?:\.md|\.txt|\.py|\.json|\.yml|\.yaml|\.toml|\.cfg|\.ini|\.html|\.css|\.js))", line, re.IGNORECASE)
        if m:
            files_written.append(m.group(1))
            if len(files_written) >= 10:
                break

    # Build compressed record
    record = {
        "agent": agent_name,
        "summary": preview[:300],
        "total_chars": len(raw_output),
        "files_written": files_written[:10],
        "errors": errors[:5],
    }

    compressed = (
        f"[SWARM COMPRESSED] {agent_name} output compacted (Pillar 118)\n"
        f"```json\n{json.dumps(record, indent=2, ensure_ascii=False)}\n```\n"
        f"--- Full output ({len(raw_output)} chars) available in scratch ---"
    )
    return compressed

WORKSPACE_DIR = r"d:\MyProject\LangChain"
PROJECT_ROOT = r"d:\MyProject"


def load_state_field(value: str) -> str:
    """If value looks like a VFS path, load its content from the filesystem. Otherwise return value."""
    if isinstance(value, str) and (value.startswith("/workspace/") or value.startswith("/scratch/") or value.startswith("/memories/")):
        try:
            from tools import _sanitize_path
            real_path = _sanitize_path(value)
            if os.path.isfile(real_path):
                with open(real_path, "r", encoding="utf-8", errors="replace") as f:
                    return f.read()
        except Exception:
            pass
    return value


def save_state_field(field_name: str, content: str) -> str:
    """Saves the content to a VFS path and returns the VFS path reference.

    Pillar 118: For agent_report fields >2000 chars, compress to structured record
    before saving, keeping the full log in scratch for reference.
    """
    FIELD_PATHS = {
        "requirements": "/workspace/1_Requirements.txt",
        "tech_spec": "/workspace/2_TechnicalSpec.txt",
        "code": "/workspace/3_SourceCode.py",
        "agent_report": "/scratch/agent_report.txt",
        "test_report": "/scratch/test_report.txt",
        "devops_config": "/scratch/devops_config.txt",
        "analytics_report": "/scratch/analytics_report.txt",
    }
    if field_name in FIELD_PATHS and content:
        if isinstance(content, str) and (content.startswith("/workspace/") or content.startswith("/scratch/") or content.startswith("/memories/")):
            return content

        # ── Pillar 118: Compress large subagent outputs ──
        if field_name == "agent_report" and len(content) > 2000:
            # Save full raw output to scratch for debugging
            try:
                import uuid as _uuid
                from tools import _sanitize_path
                raw_path = f"/scratch/agent_report_raw_{_uuid.uuid4().hex[:8]}.txt"
                real_raw = _sanitize_path(raw_path)
                os.makedirs(os.path.dirname(real_raw), exist_ok=True)
                with open(real_raw, "w", encoding="utf-8") as f:
                    f.write(content)
            except Exception:
                raw_path = None

            # Compress to structured record
            compressed = compress_swarm_output("subagent", content)
            if raw_path:
                compressed += f"\n(Full raw output: {raw_path})"
            content = compressed

        try:
            from tools import _sanitize_path
            vfs_path = FIELD_PATHS[field_name]
            real_path = _sanitize_path(vfs_path)
            os.makedirs(os.path.dirname(real_path), exist_ok=True)
            with open(real_path, "w", encoding="utf-8") as f:
                f.write(content)
            return vfs_path
        except Exception:
            pass
    return content


# vfs_state_wrapper removed — single-node graph doesn't need inter-node state offloading.
# save_state_field and load_state_field are still available below for direct use.


class ITState(TypedDict, total=False):
    client_request: str
    requirements: str
    tech_spec: str
    code: str
    agent_report: str
    test_report: str
    devops_config: str
    analytics_report: str
    error_count: int
    next_agent: str
    project_path: str
    chat_id: str
    agents_plan: str
    active_tasks: list[str]
    requirements_updated: bool
    tech_spec_updated: bool
    code_updated: bool
    remaining_steps: int



def _extract_chunk_text(chunk) -> str:
    """Safely extracts text from a LangChain stream chunk."""
    text = chunk.content
    if isinstance(text, list):
        parts: list[str] = []
        for p in text:
            if isinstance(p, str):
                parts.append(p)
            elif isinstance(p, dict):
                parts.append(p.get("text", str(p)))
            else:
                parts.append(str(p))
        return "".join(parts)
    return str(text)


def trace(msg):
    try:
        with open(r"d:\MyProject\LangChain\scratch\trace.log", "a", encoding="utf-8") as f:
            f.write(msg + "\n")
            f.flush()
    except Exception:
        pass


def invoke_llm(role: str, system_instruction: str, prompt: str) -> str:
    """Invokes LLM with centralized fallback strategy.
    Streams output to shared_state live_terminal_log."""
    shared_state["thoughts"][role.lower()] = f"Spawning {role} Agent..."
    safe_append_live_log(f"\n--- [{role.upper()} AGENT] ---")
    from llm import invoke_with_fallback
    return invoke_with_fallback(role, system_instruction, prompt)



# Department nodes (ba_node, sa_node, devops_node, analytics_node) removed.
# The agent handles all work internally. The underlying agent modules
# (ba_agent.py, sa_agent.py, etc.) are kept for reference.


def save_autonomous_document(s: dict, filename: str, content: str, is_docx: bool = False, title: str = ""):
    """Saves a document to both the developer workspace and the autonomous archive folder."""
    from word_automation import write_docx_with_word

    # 1. Save to original workspace folder
    os.makedirs(WORKSPACE_DIR, exist_ok=True)
    original_path = os.path.join(WORKSPACE_DIR, filename)
    if is_docx:
        write_docx_with_word(original_path, title, content)
    else:
        with open(original_path, "w", encoding="utf-8") as f:
            f.write(content)

    # 2. Save to D:\MyAutonomousDocuments\<ProjectName>\
    project_path = s.get("project_path", "")
    if not project_path:
        project_path = WORKSPACE_DIR
    project_name = os.path.basename(os.path.normpath(project_path))
    if not project_name:
        project_name = "LangChain"

    auto_dir = os.path.join(r"D:\MyAutonomousDocuments", project_name)
    os.makedirs(auto_dir, exist_ok=True)

    auto_path = os.path.join(auto_dir, filename)
    if is_docx:
        write_docx_with_word(auto_path, title, content)
    else:
        with open(auto_path, "w", encoding="utf-8") as f:
            f.write(content)
