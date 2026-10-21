"""compaction_ab.py — A/B task eval: control (no compaction) vs compacted.

Runs `scratch/eval_runner.py --only <task>` twice in fresh subprocesses:
    control:   COMPACTION_CHECKPOINT_TOKENS=999999999 (never compacts)
    compacted: COMPACTION_CHECKPOINT_TOKENS=<trigger> (compacts mid-task)

Both arms keep their temp workspace so the compaction metrics JSONL and the
final transcript artifacts can be inspected. Writes a markdown comparison to
scratch/compaction_reports/ab-<task>-<stamp>.md.

Run:
    venv312/Scripts/python.exe scratch/compaction_ab.py --task bugfix-offbyone
    venv312/Scripts/python.exe scratch/compaction_ab.py --task bugfix-offbyone --compacted-trigger 8000
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shutil
import subprocess
import sys

_REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_PY = sys.executable
_RUNNER = os.path.join(_REPO, "scratch", "eval_runner.py")
_EVAL_OUT = os.path.join(_REPO, "scratch", "eval_runs")
_REPORT_DIR = os.path.join(_REPO, "scratch", "compaction_reports")

_FIELDS = [
    "status", "pass_at", "pass_at_1", "attempts", "repairs", "hollow_retries",
    "duration_s", "cost", "input_tokens", "output_tokens",
    "cache_hit", "cache_miss", "paid_calls", "model_requests",
    "natural_fix_cycles", "self_checks",
]


def _run_arm(task: str, arm: str, env_overrides: dict, timeout: int) -> dict:
    env = os.environ.copy()
    env.update(env_overrides)
    env["DEEP_AGENTS_EVAL_RUN"] = "1"
    cmd = [_PY, _RUNNER, "--only", task, "--keep"]
    print(f"\n[ab] running {arm} arm: {' '.join(cmd)}", flush=True)
    started = dt.datetime.now()
    try:
        proc = subprocess.run(cmd, cwd=_REPO, env=env, timeout=timeout,
                              capture_output=True, text=True, errors="replace")
        returncode = proc.returncode
        tail = "\n".join((proc.stdout or "").splitlines()[-25:])
        err = "\n".join((proc.stderr or "").splitlines()[-15:])
    except subprocess.TimeoutExpired:
        returncode = -1
        tail = f"TIMEOUT after {timeout}s"
        err = ""
    duration = (dt.datetime.now() - started).total_seconds()

    report_path = os.path.join(_EVAL_OUT, f"{task}.json")
    report: dict = {}
    if os.path.isfile(report_path):
        try:
            with open(report_path, encoding="utf-8") as f:
                report = json.load(f)
            shutil.copyfile(
                report_path,
                os.path.join(_REPORT_DIR, f"ab-{task}-{arm}.json"),
            )
        except Exception:
            report = {}
    return {
        "arm": arm, "returncode": returncode, "wall_s": round(duration, 1),
        "report": report, "stdout_tail": tail, "stderr_tail": err,
    }


def _compaction_records(report: dict) -> list[dict]:
    tmp = report.get("tmp_dir") or ""
    path = os.path.join(tmp, ".agent-memory", "compaction_metrics.jsonl")
    if not tmp or not os.path.isfile(path):
        return []
    out = []
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line:
                    out.append(json.loads(line))
    except Exception:
        return []
    return out


def _fmt(value) -> str:
    if isinstance(value, float):
        return f"{value:,.4f}"
    if isinstance(value, int):
        return f"{value:,}"
    return str(value if value is not None else "-")


def build_markdown(task: str, control: dict, compacted: dict,
                   trigger: int, model: str) -> str:
    c, k = control["report"], compacted["report"]
    c_recs = _compaction_records(c)
    k_recs = _compaction_records(k)
    lines = [
        f"# Compaction A/B report — {task}",
        "",
        f"- generated: {dt.datetime.now().isoformat(timespec='seconds')}",
        f"- compacted trigger: {trigger:,} tokens  compactor: {model or '(default)'}",
        f"- control returncode: {control['returncode']}  wall {control['wall_s']}s",
        f"- compacted returncode: {compacted['returncode']}  wall {compacted['wall_s']}s",
        "",
        "## Task outcome & efficiency",
        "",
        "| metric | control (no compaction) | compacted | delta |",
        "|---|---|---|---|",
    ]
    for field in _FIELDS:
        cv, kv = c.get(field), k.get(field)
        delta = ""
        try:
            if isinstance(cv, (int, float)) and isinstance(kv, (int, float)):
                delta = f"{kv - cv:+,}"
        except Exception:
            delta = ""
        lines.append(f"| {field} | {_fmt(cv)} | {_fmt(kv)} | {delta} |")
    lines += [
        "",
        f"- tools (control): {json.dumps(c.get('tools', {}), ensure_ascii=False)[:400]}",
        f"- tools (compacted): {json.dumps(k.get('tools', {}), ensure_ascii=False)[:400]}",
        f"- files changed (compacted): {', '.join(k.get('files_changed', [])[:12]) or '-'}",
        "",
        "## Compaction events",
        "",
        f"- control records: {len(c_recs)}",
        f"- compacted records: {len(k_recs)}",
    ]
    for i, rec in enumerate(k_recs, 1):
        lines.append(
            f"- event {i}: chunks={rec.get('chunks')} ledgers={rec.get('ledgers')} "
            f"overlap={rec.get('overlap_tokens')} trimmed={rec.get('trimmed')} "
            f"failed={rec.get('failed_passes')} pre={rec.get('pre_tokens')} "
            f"summary={rec.get('summary_tokens')} ledger={rec.get('ledger_tokens')} "
            f"post={rec.get('post_context_tokens_est')} "
            f"duration={rec.get('duration_ms')}ms "
            f"served={','.join(rec.get('served_models') or []) or '-'}"
        )
    lines += [
        "",
        "## Interpretation hints",
        "",
    ]
    if k_recs:
        pass
    elif k.get("status"):
        lines.append(
            "- WARNING: the compacted arm produced 0 compaction events — the "
            "history never crossed the trigger. Lower `--compacted-trigger` or "
            "use a longer task; the comparison is control-vs-control otherwise."
        )
    lines += [
        "- Compare `pass_at`/`status`: compaction must not reduce correctness.",
        "- Compare `duration_s`, `input_tokens`, `cache_miss`, `model_requests`: "
        "compaction should reduce repeated exploration; a slow free compactor can "
        "still raise wall time.",
        "- Compare `natural_fix_cycles` and `tools`: more repeated reads/commands "
        "after compaction suggests the handoff lost context.",
        "- If compacted `status` is error with compaction events present, check "
        "the final-pass failure counters and `served` models.",
    ]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Compaction A/B task eval")
    parser.add_argument("--task", default="bugfix-offbyone")
    parser.add_argument("--compacted-trigger", type=int, default=8000)
    parser.add_argument("--control-trigger", type=int, default=999_999_999)
    parser.add_argument("--compaction-model", default="",
                        help="COMPACTION_MODEL for the compacted arm (default: engine default)")
    parser.add_argument("--timeout", type=int, default=1800, help="per-arm timeout seconds")
    args = parser.parse_args()

    if not os.path.isfile(_RUNNER):
        print(f"eval runner not found: {_RUNNER}")
        return 1
    os.makedirs(_REPORT_DIR, exist_ok=True)

    control = _run_arm(
        args.task, "control",
        {
            "COMPACTION_CHECKPOINT_TOKENS": str(args.control_trigger),
            "COMPACTION_HARD_LIMIT": str(args.control_trigger + 1),
        },
        args.timeout,
    )
    compacted_env = {
        "COMPACTION_CHECKPOINT_TOKENS": str(args.compacted_trigger),
        "COMPACTION_HARD_LIMIT": str(args.compacted_trigger + 6000),
    }
    if args.compaction_model:
        compacted_env["COMPACTION_MODEL"] = args.compaction_model
    compacted = _run_arm(args.task, "compacted", compacted_env, args.timeout)

    markdown = build_markdown(
        args.task, control, compacted, args.compacted_trigger, args.compaction_model
    )
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    out = os.path.join(_REPORT_DIR, f"ab-{args.task}-{stamp}.md")
    with open(out, "w", encoding="utf-8") as f:
        f.write(markdown)
    print("\n" + markdown)
    print(f"\n[ab] report: {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
