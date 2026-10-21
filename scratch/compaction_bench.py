"""compaction_bench.py — value measurement for the Codex-parity compaction.

Modes:
    --mock   (default) ideal-compactor transport test: every planted needle
             must reach the pass inputs it belongs to. Measures the pipeline
             (allocation, ledger, overlap), not LLM recall. Fast, offline.
    --live   real free-pool compactor over a scaled haystack: measures actual
             needle recall in the summary, probe answer accuracy, coverage,
             ambiguity, and writes a detailed markdown report.

Reports land in scratch/compaction_reports/ and the raw numbers in
<project>/.agent-memory/compaction_metrics.jsonl (emitted by the engine).

Run:
    venv312/Scripts/python.exe scratch/compaction_bench.py --mock
    venv312/Scripts/python.exe scratch/compaction_bench.py --live
"""
from __future__ import annotations

import argparse
import asyncio
import datetime as dt
import json
import os
import re
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import AsyncMock, patch

_REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _REPO)

from pydantic_ai.messages import (  # noqa: E402
    ModelRequest,
    ModelResponse,
    SystemPromptPart,
    TextPart,
    ToolCallPart,
    ToolReturnPart,
    UserPromptPart,
)

REPORT_DIR = os.path.join(_REPO, "scratch", "compaction_reports")


# ── Haystack ────────────────────────────────────────────────────────────────

def build_haystack(total_tokens: int = 3800, seed: int = 7) -> tuple[list, list[dict], dict]:
    """Synthetic conversation with planted needles at known regions.

    Regions mirror what the compaction pipeline must preserve:
      old_start / old_mid -> ledger chunks
      seam                -> boundary between old chunk and raw recent chunk
      recent / end        -> raw last chunk
      supersede pair      -> later fact must win in the summary
    """
    # `token` is a unique marker for the mock transport test; `expect` holds the
    # semantic substrings a live summary/ledger must contain to count as recall.
    needles = [
        {"id": "path", "region": "old_start", "token": "NEEDLE_PATH_a1b2c3",
         "expect": ["compaction_probe_a1b2c3"],
         "probe": "Which exact file path was edited early on?",
         "text": "Edited src/compaction_probe_a1b2c3.py to add with_retry() "
                 "(track as NEEDLE_PATH_a1b2c3). After edits run "
                 "`python -m py_compile src/compaction_probe_a1b2c3.py`."},
        {"id": "constraint", "region": "old_start", "token": "NEEDLE_CONSTRAINT_9f8e7d",
         "expect": ["pytest -q", "never commit"],
         "probe": "What exact testing constraint did the user state?",
         "text": "User constraint (verbatim): always run `pytest -q` before finishing; "
                 "never commit without explicit approval. NEEDLE_CONSTRAINT_9f8e7d"},
        {"id": "decision_old", "region": "old_mid", "token": "NEEDLE_DECISION_OLD_1111",
         "expect": ["SQLite"],
         "probe": "What storage decision was made early?",
         "text": "Decision: use SQLite for local cache; rejected Postgres. "
                 "NEEDLE_DECISION_OLD_1111"},
        {"id": "decision_new", "region": "recent", "token": "NEEDLE_DECISION_NEW_2222",
         "expect": ["Postgres"],
         "probe": "What storage decision is in force now?",
         "text": "Reversed the earlier storage decision: use Postgres (the SQLite "
                 "cache could not handle concurrent workers). NEEDLE_DECISION_NEW_2222"},
        {"id": "seam_error", "region": "seam", "token": "NEEDLE_ERROR_7c6b",
         "expect": ["planted_needle_42"],
         "probe": "What exact error string appeared right before the handoff?",
         "text": "Error: AssertionError: planted_needle_42 at tests/test_probe.py:13 "
                 "NEEDLE_ERROR_7c6b — fixed by adding a retry."},
        {"id": "end_state", "region": "end", "token": "NEEDLE_NEXT_5d4e",
         "expect": ["final_step_5d4e", "line 10"],
         "probe": "What is the exact next action at the end of the conversation?",
         "text": "NEXT ACTION: edit src/final_step_5d4e.py line 10 and re-run tests. "
                 "NEEDLE_NEXT_5d4e"},
    ]
    # Objective coverage facts (paths/commands/errors) beyond needle tokens.
    facts = {
        "paths": [
            "src/compaction_probe_a1b2c3.py", "src/final_step_5d4e.py",
            "tests/test_probe.py",
        ],
        "commands": ["pytest -q", "python -m py_compile"],
        "errors": ["AssertionError: planted_needle_42"],
    }

    body_chars = max(2000, int(total_tokens * 3.5 / 8))
    _unit = "work item progress note with details and findings "
    filler = (_unit * (body_chars // len(_unit) + 1))[:body_chars]

    body: list = []

    def user(text):
        return ModelRequest(parts=[UserPromptPart(content=text)])

    def assistant(text):
        return ModelResponse(parts=[TextPart(content=text)])

    def tool(name, result):
        return [
            ModelResponse(parts=[ToolCallPart(tool_name=name, args={"path": "x"}, tool_call_id=f"c-{name}-{len(body)}")]),
            ModelRequest(parts=[ToolReturnPart(tool_name=name, content=result, tool_call_id=f"c-{name}-{len(body)}")]),
        ]

    # old_start
    body.append(user("## Task\nBuild the feature. " + needles[0]["text"] + " " + needles[1]["text"]))
    body.append(assistant(filler))
    body += tool("read_file", filler)
    # old_mid (decision_old)
    body.append(user("continue. " + needles[2]["text"]))
    body.append(assistant(filler))
    body += tool("run_command", "PASS " + filler)
    # seam error
    body.append(user("tests failing. " + needles[4]["text"]))
    body.append(assistant(filler))
    # recent (decision_new)
    body.append(user(needles[3]["text"] + " " + filler))
    body.append(assistant(filler))
    body += tool("edit_file", "OK edited src/final_step_5d4e.py " + filler)
    # end (next action)
    body.append(ModelRequest(parts=[UserPromptPart(content=needles[5]["text"])]))

    history = [ModelRequest(parts=[SystemPromptPart(content="STABLE SYSTEM PROMPT")])] + body
    # Attach the objective facts to a metadata wrapper the caller can use.
    return history, needles, facts


# ── Helpers ─────────────────────────────────────────────────────────────────

def last_user_prompt(messages) -> str:
    for message in reversed(messages):
        for part in reversed(getattr(message, "parts", []) or []):
            if getattr(part, "part_kind", "") == "user-prompt":
                return str(getattr(part, "content", "") or "")
    return ""


def messages_text(messages) -> str:
    chunks = []
    for message in messages:
        for part in getattr(message, "parts", []) or []:
            content = getattr(part, "content", "")
            if isinstance(content, str):
                chunks.append(content)
            args = getattr(part, "args", None)
            if isinstance(args, dict):
                chunks.append(json.dumps(args))
    return "\n".join(chunks)


def coverage(needles: list[dict], text: str) -> dict:
    """Marker-token coverage (mock transport only)."""
    found = [n["id"] for n in needles if n["token"] in text]
    return {
        "total": len(needles),
        "found": len(found),
        "rate": round(len(found) / max(1, len(needles)), 3),
        "missing": [n["id"] for n in needles if n["token"] not in text],
    }


def semantic_coverage(needles: list[dict], text: str) -> dict:
    """Live recall: a needle counts when ANY expected semantic substring is
    present (markers are model noise; facts are what matter)."""
    low = (text or "").lower()
    found = [
        n["id"] for n in needles
        if any(str(exp).lower() in low for exp in n.get("expect", [n["token"]]))
    ]
    return {
        "total": len(needles),
        "found": len(found),
        "rate": round(len(found) / max(1, len(needles)), 3),
        "missing": [n["id"] for n in needles if n["id"] not in found],
    }


def objective_coverage(facts: dict, text: str) -> dict:
    out = {}
    for kind, items in facts.items():
        hit = [item for item in items if item in text]
        out[kind] = {"found": len(hit), "total": len(items),
                     "missing": [i for i in items if i not in hit]}
    return out


_AMBIGUOUS_RE = re.compile(
    r"\b(?:the|that|this)\s+(?:file|function|module|change|fix|thing)\b|"
    r"\b(?:it|they)\s+(?:was|were|is|are)\b",
    re.IGNORECASE,
)
_DANGLING_REF_RE = re.compile(r"\b[\w./-]+\.(?:py|ts|tsx|js|jsx|php|dart):\d+\b")


def ambiguity_report(summary: str, known_paths: list[str]) -> dict:
    vague = len(_AMBIGUOUS_RE.findall(summary or ""))
    refs = _DANGLING_REF_RE.findall(summary or "")
    dangling = [r for r in refs if not any(r.startswith(p) for p in known_paths)]
    uncertain = len(re.findall(r"\[uncertain\]", summary or "", re.IGNORECASE))
    tokens = max(1, len(summary or "") // 4)
    return {
        "summary_tokens_est": tokens,
        "vague_references": vague,
        "vague_per_1k": round(1000 * vague / tokens, 2),
        "dangling_refs": dangling,
        "uncertain_markers": uncertain,
    }


def improvement_hints(
    result: dict, cov: dict, by_region: dict, ledger_cov: dict | None = None
) -> list[str]:
    hints = []
    missing = cov.get("missing") or []
    ledger_missing = set((ledger_cov or {}).get("missing") or [])
    if not missing:
        hints.append("All planted facts survived; no structural loss detected.")
    for needle_id in missing:
        region = by_region.get(needle_id, "unknown")
        lost_in_ledger = ledger_cov is not None and needle_id in ledger_missing
        if region == "seam":
            hints.append(
                "Seam fact missing from summary: raise COMPACTION_SEAM_OVERLAP_TOKENS "
                "(or move the seam boundary earlier)."
            )
        elif region in ("old_start", "old_mid"):
            if lost_in_ledger:
                hints.append(
                    f"{needle_id}: lost in PASS 1 (never reached the ledger) — "
                    "ledger prompt not extracting it; consider stronger recall rules "
                    "or a larger COMPACTION_LEDGER_MAX_TOKENS."
                )
            else:
                hints.append(
                    f"{needle_id}: present in the ledger but dropped by PASS 2 — "
                    "the final prompt/model compressed it away; strengthen the "
                    "'preserve exact facts' instruction or raise summary detail."
                )
        elif region in ("recent", "end"):
            hints.append(
                f"{needle_id}: recent/end fact lost by the final pass — check that "
                "the raw last chunk actually reached pass 2 and the final prompt "
                "keeps current state."
            )
    if result.get("trimmed"):
        hints.append(f"Trim events ({result['trimmed']}): the window math under-estimated; raise reserve or lower chunk caps.")
    if result.get("failed_passes"):
        hints.append(f"Failed passes ({result['failed_passes']}): free-pool reliability; consider pinning a healthier provider/model.")
    if result.get("ledgers") and result.get("ledger_tokens", 0) < 200:
        hints.append("Ledger used very few tokens: prompt may be under-driving detail; inject a stronger budget line.")
    return hints


# ── Mock (transport) run ────────────────────────────────────────────────────

def mock_run(total_tokens: int = 3800) -> dict:
    from memory.compaction_engine import compact_history_with_summary

    history, needles, facts = build_haystack(total_tokens)
    by_region = {n["id"]: n["region"] for n in needles}
    d = tempfile.mkdtemp()
    os.makedirs(os.path.join(d, ".agent-memory"), exist_ok=True)
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("def run():\n    return 1\n")

    calls: list[dict] = []

    async def ideal_compactor(model, messages, **kwargs):
        prompt = last_user_prompt(messages)
        text = messages_text(messages)
        calls.append({"prompt": prompt, "text": text})
        if "For each source file below" in prompt:
            content = "app.py — Runs the probe."
        elif "LEDGER WRITER" in prompt:
            found = [n["token"] for n in needles if n["token"] in text]
            content = (
                "LEDGER:\n" + "\n".join(found)
                + "\n" + ("detail line for transport test " * 8)
            )
        else:
            found = [n["token"] for n in needles if n["token"] in text]
            content = "[SUMMARY]\n" + "\n".join(found) + "\n[PROJECT.MD]\nNO_CHANGE"
        return SimpleNamespace(parts=[SimpleNamespace(part_kind="text", content=content)])

    env = {
        "COMPACTION_CONTEXT_WINDOW": "10000",
        "COMPACTION_CHUNK_MAX_TOKENS": "2000",
        "COMPACTION_LAST_CHUNK_MAX_TOKENS": "2000",
        "COMPACTION_LEDGER_MAX_TOKENS": "2048",
        "COMPACTION_RESERVE_TOKENS": "1024",
        "COMPACTION_SEAM_OVERLAP_TOKENS": "200",
    }
    with patch.dict(os.environ, env, clear=False), \
         patch("llm.free_provider.build_free_fallback", return_value=object()), \
         patch("pydantic_ai.direct.model_request", new=AsyncMock(side_effect=ideal_compactor)):
        new_history, info = compact_history_with_summary(history, project_path=d)

    ledger_calls = [c for c in calls if "LEDGER WRITER" in c["prompt"]]
    final_calls = [c for c in calls if "CURRENT PROJECT.MD" in c["prompt"]]
    ledger_text = "\n".join(c["text"] for c in ledger_calls)
    final_text = "\n".join(c["text"] for c in final_calls)
    summary = str(info.get("summary") or "")

    return {
        "mode": "mock",
        "total_tokens": total_tokens,
        "project_path": d,
        "needles": needles,
        "by_region": by_region,
        "facts": facts,
        "result": info,
        "calls": len(calls),
        "ledger_transport": coverage(needles, ledger_text),
        "final_transport": coverage(needles, final_text),
        "summary_recall": coverage(needles, summary),
        "objective": objective_coverage(facts, summary),
        "ambiguity": ambiguity_report(summary, facts["paths"]),
        "compacted_history_parts": len(new_history),
    }


# ── Live run ────────────────────────────────────────────────────────────────

async def _ask_model(model_id: str, question: str, context: str) -> str:
    from pydantic_ai import direct as _direct
    from llm.free_provider import build_free_fallback

    model = build_free_fallback(capability="text", preferred_model=model_id)
    if model is None:
        return ""
    prompt = (
        "You are answering questions about a coding session using ONLY the "
        "context below. If the answer is not present, reply UNKNOWN. Answer in "
        "as few words as possible, keeping exact identifiers.\n\n"
        f"CONTEXT:\n{context}\n\nQUESTION: {question}\nANSWER:"
    )
    request = [ModelRequest(parts=[UserPromptPart(content=prompt)])]
    try:
        resp = await _direct.model_request(model, request, model_settings={"temperature": 0})
    except Exception:
        return ""
    return "".join(
        str(getattr(p, "content", "") or "")
        for p in getattr(resp, "parts", []) or []
        if getattr(p, "part_kind", "") == "text"
    ).strip()


def live_run(total_tokens: int = 40000, judge_model: str = "llama-3.3-70b-versatile") -> dict:
    from memory.compaction_engine import run_compaction_summary

    history, needles, facts = build_haystack(total_tokens)
    by_region = {n["id"]: n["region"] for n in needles}
    d = tempfile.mkdtemp()
    os.makedirs(os.path.join(d, ".agent-memory"), exist_ok=True)
    with open(os.path.join(d, "app.py"), "w", encoding="utf-8") as f:
        f.write("def run():\n    return 1\n")

    env = {
        "COMPACTION_CONTEXT_WINDOW": "26000",
        "COMPACTION_CHUNK_MAX_TOKENS": "21000",
        "COMPACTION_LAST_CHUNK_MAX_TOKENS": "20000",
        "COMPACTION_LEDGER_MAX_TOKENS": "4000",
        "COMPACTION_RESERVE_TOKENS": "1500",
        "COMPACTION_SEAM_OVERLAP_TOKENS": "500",
    }
    with patch.dict(os.environ, env, clear=False):
        info = asyncio.run(run_compaction_summary(history, d))

    summary = str(info.get("summary") or "")
    ledger_text = "\n\n---\n\n".join(info.get("ledger_texts") or [])
    recall = semantic_coverage(needles, summary)
    ledger_recall = semantic_coverage(needles, ledger_text)
    objective = objective_coverage(facts, summary)

    # Probe answers: give the model ONLY the compacted context (summary).
    probes = []
    for needle in needles:
        answer = asyncio.run(_ask_model(judge_model, needle["probe"], summary))
        answers = [str(exp).lower() for exp in needle.get("expect", [])]
        hit = any(exp in answer.lower() for exp in answers)
        probes.append({
            "id": needle["id"], "region": needle["region"],
            "question": needle["probe"], "expected": ", ".join(needle.get("expect", [])),
            "answer": answer[:200], "correct": bool(hit),
        })
    correct = sum(1 for p in probes if p["correct"])

    return {
        "mode": "live",
        "total_tokens": total_tokens,
        "project_path": d,
        "judge_model": judge_model,
        "needles": needles,
        "by_region": by_region,
        "facts": facts,
        "result": info,
        "summary_recall": recall,
        "ledger_recall": ledger_recall,
        "objective": objective,
        "ambiguity": ambiguity_report(summary, facts["paths"]),
        "probes": probes,
        "probe_accuracy": round(correct / max(1, len(probes)), 3),
        "ledger_preview": ledger_text[:3000],
        "summary_preview": summary[:4000],
    }


# ── Report ──────────────────────────────────────────────────────────────────

def _region_table(report: dict) -> str:
    rows = []
    by_region = report["by_region"]
    missing = set(report["summary_recall"].get("missing") or [])
    live = report["mode"] == "live"
    ledger_missing = set((report.get("ledger_recall") or {}).get("missing") or [])
    header = (
        "| needle | region | summary | ledger | expect |\n|---|---|---|---|---|"
        if live else
        "| needle | region | summary | token |\n|---|---|---|---|"
    )
    rows.append(header)
    for needle in report["needles"]:
        expect = ", ".join(needle.get("expect", [])) or needle["token"]
        if live:
            rows.append(
                f"| {needle['id']} | {by_region.get(needle['id'], '?')} | "
                f"{'MISSING' if needle['id'] in missing else 'ok'} | "
                f"{'MISSING' if needle['id'] in ledger_missing else 'ok'} | {expect} |"
            )
        else:
            rows.append(
                f"| {needle['id']} | {by_region.get(needle['id'], '?')} | "
                f"{'MISSING' if needle['id'] in missing else 'ok'} | {needle['token']} |"
            )
    return "\n".join(rows)


def generate_report(report: dict) -> str:
    r = report["result"]
    mos = "mock" if report["mode"] == "mock" else "live"
    lines = [
        f"# Compaction bench report ({mos})",
        "",
        f"- generated: {dt.datetime.now().isoformat(timespec='seconds')}",
        f"- haystack target: ~{report['total_tokens']:,} tokens",
        f"- compactor model: `{r.get('model') or '-'}`  window: {r.get('window') or 0:,}",
        f"- chunks: {r.get('chunks', 0)}  ledgers: {r.get('ledgers', 0)}  "
        f"overlap: {r.get('overlap_tokens', 0):,} tok  trimmed: {r.get('trimmed', 0)}  "
        f"failed passes: {r.get('failed_passes', 0)}  "
        f"served by: {', '.join(r.get('served_models') or []) or '-'}",
        f"- pre-compaction tokens (est): {r.get('pre_tokens', 0):,}",
        f"- summary tokens: {r.get('summary_tokens', 0):,}  "
        f"ledger tokens: {r.get('ledger_tokens', 0):,}  "
        f"retained user tokens: {r.get('retained_user_tokens', 0):,}",
        f"- post-compaction context (est): {r.get('post_context_tokens_est', 0):,}",
        f"- duration: {r.get('duration_ms', 0):,} ms",
        "",
        "## Recall by needle",
        "",
        _region_table(report),
        "",
        f"Summary recall: **{report['summary_recall']['found']}/"
        f"{report['summary_recall']['total']}** "
        f"({report['summary_recall']['rate'] * 100:.0f}%)",
    ]
    if report["mode"] == "mock":
        lines += [
            f"- ledger transport: {report['ledger_transport']['found']}/"
            f"{report['ledger_transport']['total']}",
            f"- final-pass transport: {report['final_transport']['found']}/"
            f"{report['final_transport']['total']}",
        ]
    else:
        lines += [
            f"- ledger recall (pass 1): **{report['ledger_recall']['found']}/"
            f"{report['ledger_recall']['total']}** "
            f"({report['ledger_recall']['rate'] * 100:.0f}%)",
            f"- probe accuracy: **{report['probe_accuracy'] * 100:.0f}%** "
            f"({sum(1 for p in report['probes'] if p['correct'])}/{len(report['probes'])})",
        ]
    lines += [
        "",
        "## Objective facts (paths / commands / errors)",
        "",
    ]
    for kind, data in report["objective"].items():
        lines.append(f"- {kind}: {data['found']}/{data['total']}  missing={data['missing']}")
    amb = report["ambiguity"]
    lines += [
        "",
        "## Ambiguity / precision guardrails",
        "",
        f"- vague references: {amb['vague_references']} ({amb['vague_per_1k']}/1k tokens)",
        f"- dangling refs: {amb['dangling_refs'] or 'none'}",
        f"- [uncertain] markers: {amb['uncertain_markers']}",
        "",
        "## Improvement hints",
        "",
    ]
    lines += [f"- {h}" for h in improvement_hints(
        r, report["summary_recall"], report["by_region"],
        ledger_cov=report.get("ledger_recall"),
    )]
    if report["mode"] == "live":
        lines += ["", "## Probe details", ""]
        for probe in report["probes"]:
            lines.append(
                f"- [{ 'ok' if probe['correct'] else 'MISS' }] {probe['id']} "
                f"({probe['region']}): expected `{probe['expected']}` got `{probe['answer'][:80]}`"
            )
        lines += ["", "## Ledger (pass 1) preview", "", "```",
                  report.get("ledger_preview", ""), "```"]
        lines += ["", "## Summary preview", "", "```", report.get("summary_preview", ""), "```"]
    return "\n".join(lines)


def write_report(report: dict) -> str:
    os.makedirs(REPORT_DIR, exist_ok=True)
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    path = os.path.join(REPORT_DIR, f"{stamp}-{report['mode']}.md")
    with open(path, "w", encoding="utf-8") as f:
        f.write(generate_report(report))
    return path


def main() -> int:
    parser = argparse.ArgumentParser(description="Compaction value bench")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--mock", action="store_true", help="offline transport test (default)")
    mode.add_argument("--live", action="store_true", help="real free-pool compactor")
    parser.add_argument("--tokens", type=int, default=None,
                        help="haystack size target (default 3800 mock / 40000 live)")
    parser.add_argument("--judge", default="llama-3.3-70b-versatile")
    args = parser.parse_args()

    if args.live:
        report = live_run(args.tokens or 40000, judge_model=args.judge)
    else:
        report = mock_run(args.tokens or 3800)
    path = write_report(report)
    print(f"[bench] report: {path}")
    print("=" * 70)
    print(generate_report(report))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
