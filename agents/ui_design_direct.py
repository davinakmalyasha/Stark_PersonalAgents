"""UI design direct runner — OpenDesign module backend (Phase 1).

Runs the existing UI specialist (agents/specialists/ui.json) DIRECTLY on a
design brief: no orchestrator, no Dev in the loop. Every prompt in a design
module session goes straight to this UI loop.

Session flow:
  1. Build the opening brief: skill pointer + lean context (user brief,
     workspace path, existing project memory if present)
     + explicit deliverables contract.
  2. start_continuable(chat_id, workspace, "UI", brief) → first turn.
  3. Continuation guard: while deliverables are missing and rounds remain,
     send_message() a pointed nudge (never rely on a single turn to finish).
  4. Yield state snapshots in the same shape as the other direct runners
     (run_house_direct / run_general_direct / run_dfd_direct).

Read-only w.r.t. existing flows: this is a NEW file. Dev, orchestrator, and
UI-verification paths are untouched. The UI spec's design-mode lines
(agents/specialists/ui.json) only trigger when the TASK is design work.
"""

from __future__ import annotations

import glob
import json
import os
import time

_AGENT_NAME = "UI-Design"

_MAX_MEMORY_CHARS = 4000


def _ui_skill_names() -> list[str]:
    """Skill names from the UI specialist spec (single source of truth)."""
    try:
        spec_path = os.path.join(
            os.path.dirname(os.path.abspath(__file__)), "specialists", "ui.json"
        )
        with open(spec_path, "r", encoding="utf-8") as f:
            names = json.load(f).get("skills") or []
        return [str(n) for n in names if str(n).strip()]
    except Exception:
        return []


def _harness_root() -> str:
    return os.environ.get("HARNESS_ROOT") or os.path.dirname(
        os.path.dirname(os.path.abspath(__file__))
    )


def _read_text_capped(path: str, cap: int) -> str:
    try:
        with open(path, "r", encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return ""
    if len(text) > cap:
        text = text[:cap] + f"\n[... truncated {len(text) - cap} chars ...]"
    return text


def _session_evidence(workspace: str, since_ts: float) -> dict:
    """Fresh-session deliverables: files with mtime >= session start.

    mtime gating is deliberate: stale artifacts from earlier sessions must
    NOT satisfy the contract (they confused prior drill runs).
    """
    html, shots, designmd = [], [], []
    try:
        for p in glob.glob(os.path.join(workspace, "**", "*.html"), recursive=True):
            try:
                if os.path.isfile(p) and os.path.getmtime(p) >= since_ts:
                    html.append(os.path.relpath(p, workspace))
            except OSError:
                continue
    except Exception:
        pass
    try:
        for ext in ("*.png", "*.jpg", "*.jpeg", "*.webp"):
            for p in glob.glob(os.path.join(workspace, "**", ext), recursive=True):
                try:
                    if os.path.isfile(p) and os.path.getmtime(p) >= since_ts:
                        shots.append(os.path.relpath(p, workspace))
                except OSError:
                    continue
    except Exception:
        pass
    try:
        pm = os.path.join(workspace, ".agent-memory", "project.md")
        if os.path.isfile(pm) and os.path.getmtime(pm) >= since_ts:
            designmd.append(".agent-memory/project.md")
    except OSError:
        pass
    return {"html": sorted(html)[:20], "screenshots": sorted(shots)[:20],
            "project_md": designmd}


def _missing(evidence: dict) -> list[str]:
    missing = []
    if not evidence["html"]:
        missing.append("artifact HTML (created/modified this session)")
    if not evidence["screenshots"]:
        missing.append("screenshot evidence (saved in the project folder)")
    if not evidence["project_md"]:
        missing.append("`## Design` section in .agent-memory/project.md (updated this session)")
    return missing


def _build_brief(prompt: str, workspace: str, images: list | None) -> str:
    root = _harness_root()
    # Progressive disclosure: the prompt carries the skill's description and
    # absolute path; the agent reads the full SKILL.md itself when the loop
    # applies.
    skills_block = ""
    try:
        from tools.skills import skill_pointer_block
        skills_block = skill_pointer_block(
            _ui_skill_names() or ["ui-ux-pro-max"],
            workspace_path=workspace,
            harness_root=root,
        )
    except Exception:
        skills_block = ""
    prior_design = _read_text_capped(
        os.path.join(workspace, ".agent-memory", "project.md"), _MAX_MEMORY_CHARS
    )
    lines = [
        "# UI/UX DESIGN SESSION (OpenDesign module)",
        "",
        "You are working a design session. Every prompt in this session comes",
        "directly to you — no orchestrator, no developer agent in the loop.",
        "",
        f"Harness root (absolute paths for shared files): {root}",
        f"Project folder — do ALL work inside it, touch nothing outside: {workspace}",
        "",
    ]
    if skills_block:
        lines += [skills_block, "",
                  "Design work MUST follow that skill file end-to-end.", ""]
    else:
        lines += ["## Design skill file missing — proceed with best practice:",
                  "generate the design system with ui-ux-pro-max's search script,",
                  "build real files, self-critique, screenshot-verify, finalize",
                  "the `## Design` section in .agent-memory/project.md.", ""]
    if prior_design:
        lines += ["## Standing project memory (honor the `## Design` section; never edit other sections)",
                  "", prior_design, ""]
    if images:
        lines += ["## Attached reference images",
                  "\n".join(f"- {p}" for p in images),
                  "Read them with read_file (vision) and honor what they show.", ""]
    lines += [
        "## Deliverables (ALL required before finishing)",
        "1. Artifact HTML created/modified during THIS session, inside the project folder.",
        "2. Screenshot evidence saved inside the project folder.",
        "3. The `## Design` section in .agent-memory/project.md updated this session "
        "(direction, theme, palette, typography, concept — compact bullets; create "
        "`.agent-memory/` if needed). Never modify `## Identity` or other lines.",
        "Keep acting with tools until all three exist. Never end a turn with",
        "text-only narration mid-loop — a text-only reply ends the session.",
        "",
        "## USER BRIEF",
        prompt.strip(),
    ]
    return "\n".join(lines)


def _nudge(missing: list[str]) -> str:
    return (
        "Continue the design session NOW with tools (do not reply text-only — "
        "a text-only reply ends the session). Still missing: "
        + "; ".join(missing)
        + ". Pick up exactly where the loop left off."
    )


def run_ui_design_direct(prompt: str, workspace_path: str = "",
                         chat_id: str = "", images: list | None = None,
                         max_rounds: int = 4):
    """Generator entry point for the API server (design workspace_kind).

    Yields serialized state snapshots, same contract as run_house_direct.
    """
    from sync.state_sync import safe_serialize_state, safe_update_state
    from agents.subagent_manager import start_continuable, send_message

    images = list(images or [])
    t0 = time.time()

    def _snap(log: str, extra: dict | None = None):
        state = {"live_terminal_log": f"[{_AGENT_NAME}] {log}\n"}
        if extra:
            state["outputs"] = extra
        safe_update_state(state)
        yield json.loads(safe_serialize_state())

    yield from _snap("Processing…")

    ws = (workspace_path or "").strip()
    if not ws or not os.path.isdir(ws):
        yield from _snap(
            "ERROR: the design module needs a project folder.",
            {"agent_report": "ERROR: no project folder bound to this design session."},
        )
        return

    brief = _build_brief(prompt, os.path.abspath(ws), images)
    label = f"Design: {' '.join(prompt.split())[:40]}"

    child_id = ""
    last_result = ""
    try:
        yield from _snap("Starting UI design session…")
        first = start_continuable(chat_id or "design", os.path.abspath(ws),
                                  "UI", brief, label=label)
        child_id = str(first.get("child_id", ""))
        last_result = str(first.get("result", "") or "")
        yield from _snap(f"Design turn 1 done (session {child_id}).",
                         {"design_child_id": child_id})

        rounds = 1
        while rounds < max_rounds:
            missing = _missing(_session_evidence(os.path.abspath(ws), t0))
            if not missing:
                break
            rounds += 1
            yield from _snap(
                f"Continuing design (round {rounds}/{max_rounds}): "
                f"still missing {', '.join(missing)}.")
            try:
                nxt = send_message(child_id, _nudge(missing),
                                   os.path.abspath(ws))
                last_result = str(nxt.get("result", "") or last_result)
            except Exception as e:
                yield from _snap(f"Continue turn failed: {e}")
                break

        evidence = _session_evidence(os.path.abspath(ws), t0)
        still_missing = _missing(evidence)
        report_lines = [
            "## UI Design session",
            f"- Session: {child_id}",
            f"- Artifact HTML: {', '.join(evidence['html']) or '—'}",
            f"- Screenshots: {', '.join(evidence['screenshots']) or '—'}",
            f"- project.md `## Design`: {'updated this session' if evidence['project_md'] else 'no'}",
        ]
        if still_missing:
            report_lines.append(
                "- INCOMPLETE (max rounds reached), still missing: "
                + ", ".join(still_missing))
        if last_result:
            report_lines += ["", "### Agent's last message", last_result[-2000:]]
        report = "\n".join(report_lines)
        yield from _snap("Done",
                         {"agent_report": report,
                          "design_child_id": child_id})
    except Exception as e:
        import traceback
        print(f"[{_AGENT_NAME} Error] {e}\n{traceback.format_exc()}", flush=True)
        yield from _snap(
            f"[{_AGENT_NAME} Error] {e}",
            {"agent_report": f"ERROR: UI design session failed: {e}",
             "design_child_id": child_id},
        )
