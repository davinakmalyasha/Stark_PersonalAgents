"""gen10_review.py — post-run quality reviewer + evidence bundler for gen10.

The gen10 long-horizon tasks deliberately do NOT encode quality as deterministic
hidden-test checks: the hidden gate only answers "does it run / do the required
journeys work". Quality (design, codebase structure, performance, scalability,
maintainability, security) is scored by a reviewer — agent or human — after the run.

Workflow:
    venv312/Scripts/python.exe scratch/eval_runner.py --only g10-refine-notes --keep
    venv312/Scripts/python.exe scratch/gen10_review.py g10-refine-notes
    venv312/Scripts/python.exe scratch/gen10_review.py g10-refine-notes --score design=4 --verdict "solid"
    venv312/Scripts/python.exe scratch/gen10_review.py --table

Each call writes/updates scratch/gen10_runs/<task_id>/:
    run.json       copy of scratch/eval_runs/<task_id>.json
    events.jsonl   that task's events from scratch/eval_runs/_events.jsonl
    calls.jsonl    raw .deep_agents/gen5_eval_calls.jsonl (whole run, per-call)
    manifest.json  workspace inventory (files, LOC, deps) when --keep preserved it
    review.json    rubric scores + auto metrics (scores are never auto-overwritten)
"""
import argparse
import collections
import glob
import json
import os
import shutil
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCRATCH = os.path.join(ROOT, "scratch")
RUNS = os.path.join(SCRATCH, "eval_runs")
BUNDLE = os.path.join(SCRATCH, "gen10_runs")
EVENTS = os.path.join(RUNS, "_events.jsonl")
CALLS = os.path.join(ROOT, ".deep_agents", "gen5_eval_calls.jsonl")

RUBRIC = {
    "design": {
        "desc": "Product/visual quality and API design against the complex, production-grade "
                "standard of the domain (big-brand marketing site / real storefront / real API).",
        "hints": [
            "Landing/build: browse the rendered app (screenshots), check hierarchy, spacing, "
            "color/type system, states (hover/focus/empty/error), mobile vs desktop, SEO surface.",
            "Refine/build: is the API contract coherent (naming, status codes, shapes, "
            "pagination) and rich enough for real clients, or just patched together?",
        ],
    },
    "codebase_structure": {
        "desc": "Layering, module boundaries, naming, cohesion, absence of monolith/AI-slop.",
        "hints": [
            "File tree + LOC distribution: routes/services/middleware/components split, "
            "no 600-line god files, no dead/duplicate modules.",
            "Do responsibilities sit in the right layer (no DB access in routes, etc.)?",
        ],
    },
    "performance": {
        "desc": "Runtime behavior: latency, wasted work, payload sizes, build/start health.",
        "hints": [
            "Any N+1-ish scans, repeated full-file rewrites, blocking sync I/O on hot paths?",
            "Landing: build output size, unnecessary re-renders, unoptimized assets.",
        ],
    },
    "scalability": {
        "desc": "How the system behaves as data/users grow: state model, concurrency, config.",
        "hints": [
            "Persistence/state choices, uniqueness under concurrency, pagination everywhere, "
            "secrets/config via env, no hardcoded limits that break at N+1.",
        ],
    },
    "maintainability": {
        "desc": "Tests, docs, readability, error handling, ease of safe change.",
        "hints": [
            "Do the tests actually cover the risky paths (not just happy path)?",
            "Spec/README accuracy; consistent error model; no leftover debug code.",
        ],
    },
    "security": {
        "desc": "Authn/z, input validation, secret handling, dependency and output safety.",
        "hints": [
            "bcrypt/JWT/rate-limit correctness, role checks on every admin path, validation "
            "on every write, no secrets in source, no XSS in rendered content.",
        ],
    },
}

TASK_FOCUS = {
    "g10-landing-nextjs": {
        "design": "Judged from the rendered pages (browse + screenshots), not the code. "
                  "Bar: award-level marketing site — distinct identity, design system, full IA, "
                  "real copy, all interaction states, responsive, WCAG AA a11y.",
        "performance": "Next.js build/start health, prerendering, layout stability, bundle "
                       "weight, asset discipline.",
        "scalability": "Content model, metadata per route, room to add pages, form persistence.",
    },
    "g10-refine-notes": {
        "design": "API ergonomics: the contract upgraded to a real service (history, restore, "
                  "export, rate limiting) and the product visibly better than the starter.",
        "codebase_structure": "The NoteHub monolith must now be layered (routes/services/"
                              "middleware); check the split is real, not cosmetic.",
        "maintainability": "Regression safety + upgrade evidence: tests cover old behavior AND "
                           "the new features; docs/TODO.md items closed; specs present.",
    },
    "g10-ecommerce-complete": {
        "design": "Storefront UX + full API contract coherence against the spec.",
        "security": "Auth/roles/rate-limit/validation on every path; check admin-only routes.",
        "scalability": "Stock/cart/order consistency, pagination, persistence across restarts.",
        "maintainability": "Deployment evidence: git remote origin set to "
                           "davinakmalyasha/TestAgents, project pushed, README run/deploy notes.",
    },
}


def _load_json(path, default=None):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return default


def _auto_metrics(report):
    if not report:
        return {}
    cache_hit = report.get("cache_hit", 0) or 0
    cache_miss = report.get("cache_miss", 0) or 0
    denom = cache_hit + cache_miss
    paid = report.get("paid_calls", 0) or 0
    duration = report.get("duration_s", 0) or 0
    return {
        "status": report.get("status"),
        "pass_at": report.get("pass_at"),
        "pass_at_1": report.get("pass_at_1"),
        "attempts": report.get("attempts"),
        "repairs": report.get("repairs"),
        "hollow_retries": report.get("hollow_retries"),
        "cost_usd": report.get("cost"),
        "duration_s": duration,
        "input_tokens": report.get("input_tokens"),
        "output_tokens": report.get("output_tokens"),
        "reasoning_tokens": report.get("reasoning_tokens"),
        "cache_hit": cache_hit,
        "cache_miss": cache_miss,
        "cache_hit_pct": round(100.0 * cache_hit / denom, 1) if denom else None,
        "paid_calls": paid,
        "model_requests": report.get("model_requests"),
        "input_tokens_per_call": round((report.get("input_tokens", 0) or 0) / paid, 1) if paid else None,
        "cost_per_minute": round((report.get("cost", 0) or 0) / (duration / 60.0), 6) if duration else None,
        "self_checks": report.get("self_checks"),
        "natural_fix_cycles": report.get("natural_fix_cycles"),
        "spec_produced": report.get("spec_produced"),
        "files_changed": len(report.get("files_changed") or []),
        "agents": report.get("agents"),
        "tools": report.get("tools"),
        "attempt_log": report.get("attempt_log"),
        "prompt_style": report.get("prompt_style"),
        "test_detail_head": (report.get("test_detail") or "")[:400],
    }


_DEVICE_FILE_NAMES = {
    "nul", "con", "prn", "aux",
    *(f"com{i}" for i in range(1, 10)),
    *(f"lpt{i}" for i in range(1, 10)),
}


def _manifest(tmp_dir):
    if not tmp_dir or not os.path.isdir(tmp_dir):
        return None
    skip = ("node_modules", ".git", ".next", "dist", ".agent-memory", ".deep_agents", "eval_tmp")
    exts = collections.Counter()
    loc = collections.Counter()
    files = []
    for root, dirs, names in os.walk(tmp_dir):
        dirs[:] = [d for d in dirs if d not in skip]
        for fn in names:
            # Windows reserved device names (nul, con, ...) can appear via shell
            # redirects; abspath maps them to \\.\<name> and relpath raises.
            if fn.lower().split(".")[0] in _DEVICE_FILE_NAMES:
                continue
            full = os.path.join(root, fn)
            rel = os.path.relpath(full, tmp_dir)
            ext = os.path.splitext(fn)[1].lower() or "(none)"
            exts[ext] += 1
            files.append(rel)
            if ext in (".js", ".ts", ".tsx", ".jsx", ".css", ".html", ".py", ".json", ".md"):
                try:
                    with open(full, encoding="utf-8", errors="replace") as f:
                        loc[ext] += sum(1 for _ in f)
                except OSError:
                    pass
    pkg = _load_json(os.path.join(tmp_dir, "package.json")) or {}
    return {
        "tmp_dir": tmp_dir,
        "file_count": len(files),
        "files_by_ext": dict(exts.most_common()),
        "loc_by_ext": dict(loc.most_common()),
        "total_loc": sum(loc.values()),
        "dependencies": pkg.get("dependencies"),
        "scripts": pkg.get("scripts"),
        "files": sorted(files)[:500],
    }


def _task_events(task_id):
    rows = []
    if not os.path.isfile(EVENTS):
        return rows
    for line in open(EVENTS, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except Exception:
            continue
        if rec.get("id") == task_id:
            rows.append(rec)
    return rows


def _new_review(task_id, report, manifest):
    focus = TASK_FOCUS.get(task_id, {})
    rubric = {}
    for dim, meta in RUBRIC.items():
        rubric[dim] = {
            "score": None,
            "notes": focus.get(dim, ""),
            "evidence": [],
        }
    return {
        "task_id": task_id,
        "category": (report or {}).get("category"),
        "created": time.strftime("%Y-%m-%d %H:%M:%S"),
        "reviewer": "",
        "verdict": "",
        "rubric": rubric,
        "rubric_definitions": {d: {"desc": m["desc"], "hints": m["hints"]} for d, m in RUBRIC.items()},
        "auto": _auto_metrics(report),
        "workspace": {
            "tmp_dir": (manifest or {}).get("tmp_dir") or (report or {}).get("tmp_dir"),
            "kept": bool((report or {}).get("tmp_dir")),
            "file_count": (manifest or {}).get("file_count"),
            "total_loc": (manifest or {}).get("total_loc"),
        },
        "overall": {"score_avg": None, "max": 5},
    }


def _avg(review):
    scores = [d.get("score") for d in (review.get("rubric") or {}).values()
              if isinstance(d.get("score"), (int, float))]
    return round(sum(scores) / len(scores), 2) if scores else None


def _bundle(task_id, score_updates, note_updates, verdict, reviewer, quiet=False):
    os.makedirs(BUNDLE, exist_ok=True)
    outdir = os.path.join(BUNDLE, task_id)
    os.makedirs(outdir, exist_ok=True)

    report = _load_json(os.path.join(RUNS, f"{task_id}.json"))
    if report is None:
        print(f"no run report at {os.path.join(RUNS, task_id + '.json')} — run the task first")
        return None
    shutil.copy2(os.path.join(RUNS, f"{task_id}.json"), os.path.join(outdir, "run.json"))

    events = _task_events(task_id)
    with open(os.path.join(outdir, "events.jsonl"), "w", encoding="utf-8") as f:
        for rec in events:
            f.write(json.dumps(rec, default=str) + "\n")

    calls_note = "missing"
    calls_src = os.path.join(ROOT, ".deep_agents", f"gen5_eval_calls_{task_id}.jsonl")
    if not os.path.isfile(calls_src):
        calls_src = CALLS  # legacy shared log fallback
    if os.path.isfile(calls_src):
        shutil.copy2(calls_src, os.path.join(outdir, "calls.jsonl"))
        calls_note = "whole-run raw per-call log (filter by timestamp/agent)"

    manifest = None
    tmp_dir = report.get("tmp_dir")
    if tmp_dir and os.path.isdir(tmp_dir):
        manifest = _manifest(tmp_dir)
        with open(os.path.join(outdir, "manifest.json"), "w", encoding="utf-8") as f:
            json.dump(manifest, f, indent=1, default=str)

    review_path = os.path.join(outdir, "review.json")
    review = _load_json(review_path)
    if review is None:
        review = _new_review(task_id, report, manifest)
    else:
        review["auto"] = _auto_metrics(report)
        review["workspace"].update({
            "tmp_dir": (manifest or {}).get("tmp_dir") or tmp_dir,
            "kept": bool(tmp_dir),
            "file_count": (manifest or {}).get("file_count"),
            "total_loc": (manifest or {}).get("total_loc"),
        })
    review["calls_log"] = calls_note

    for dim, val in (score_updates or {}).items():
        if dim not in review["rubric"]:
            print(f"unknown rubric dimension {dim!r}; valid: {sorted(RUBRIC)}")
            continue
        review["rubric"][dim]["score"] = val
    for dim, text in (note_updates or {}).items():
        if dim not in review["rubric"]:
            print(f"unknown rubric dimension {dim!r}; valid: {sorted(RUBRIC)}")
            continue
        old = review["rubric"][dim].get("notes") or ""
        review["rubric"][dim]["notes"] = (old + ("\n" if old else "") + text).strip()
    if verdict:
        review["verdict"] = verdict
    if reviewer:
        review["reviewer"] = reviewer
    review["overall"]["score_avg"] = _avg(review)

    with open(review_path, "w", encoding="utf-8") as f:
        json.dump(review, f, indent=1, default=str)

    if not quiet:
        a = review["auto"]
        w = review["workspace"]
        print(f"\n=== gen10 review bundle: {task_id} -> {outdir} ===")
        print(f"  status={a.get('status')}  pass@1={a.get('pass_at_1')}  attempts={a.get('attempts')}  "
              f"repairs={a.get('repairs')}")
        print(f"  cost=${a.get('cost_usd')}  duration={a.get('duration_s')}s  "
              f"cache-hit={a.get('cache_hit_pct')}%  self_checks={a.get('self_checks')}  "
              f"natural_fix_cycles={a.get('natural_fix_cycles')}")
        print(f"  workspace kept={w.get('kept')}  files={w.get('file_count')}  "
              f"loc={w.get('total_loc')}  spec_produced={a.get('spec_produced')}")
        if a.get("test_detail_head"):
            print(f"  gate detail: {a.get('test_detail_head', '')[:200]}")
        print("  rubric scores (edit via --score <dim>=<0-5>, then --table):")
        for dim, meta in review["rubric"].items():
            print(f"    {dim:<20} {meta.get('score')}   {str(meta.get('notes') or '')[:90]}")
        print("  artifacts: run.json, events.jsonl, calls.jsonl, manifest.json, review.json")
    return review


def _table():
    entries = []
    for rp in sorted(glob.glob(os.path.join(BUNDLE, "*", "review.json"))):
        review = _load_json(rp) or {}
        a = review.get("auto") or {}
        entries.append((review.get("task_id") or os.path.basename(os.path.dirname(rp)), review, a))
    if not entries:
        print(f"no review bundles in {BUNDLE} — run gen10_review.py <task_id> first")
        return
    print("\n=== gen10 LONG-HORIZON REVIEW TABLE ===")
    hdr = (f"{'task':<24}{'st':<6}{'p@1':<5}{'cost$':<9}{'dur s':<8}{'cache%':<8}"
           f"{'selfchk':<8}{'fixcyc':<7}{'qual':<6}{'d/s/p/sc/m/sec':<16}verdict")
    print(hdr)
    for task_id, review, a in entries:
        dims = []
        for dim in ("design", "codebase_structure", "performance", "scalability",
                    "maintainability", "security"):
            s = (review.get("rubric", {}).get(dim) or {}).get("score")
            dims.append(str(s) if s is not None else "-")
        print(f"{task_id:<24}{str(a.get('status')):<6}{str(a.get('pass_at_1') if a.get('pass_at_1') is not None else '-'):<5}"
              f"{('$' + format(float(a.get('cost_usd') or 0), '.4f')):<9}{str(a.get('duration_s')):<8}"
              f"{str(a.get('cache_hit_pct')):<8}{str(a.get('self_checks')):<8}"
              f"{str(a.get('natural_fix_cycles')):<7}"
              f"{str((review.get('overall') or {}).get('score_avg')):<6}"
              f"{'/'.join(dims):<16}{str(review.get('verdict') or '')[:40]}")
    unscored = [t for t, r, _a in entries
                if (r.get("overall") or {}).get("score_avg") is None]
    if unscored:
        print(f"\nunscored: {', '.join(unscored)}  "
              f"(score with: gen10_review.py <task> --score design=4 ...)")


def main():
    ap = argparse.ArgumentParser(description="gen10 post-run review bundler")
    ap.add_argument("task_id", nargs="?", help="gen10 task id (e.g. g10-refine-notes)")
    ap.add_argument("--score", action="append", default=[],
                    help="dimension=value (0-5); repeatable, e.g. --score design=4")
    ap.add_argument("--note", action="append", default=[],
                    help="dimension=text; appends a note; repeatable")
    ap.add_argument("--verdict", default="", help="short overall verdict")
    ap.add_argument("--reviewer", default="", help="who scored this run (name/agent)")
    ap.add_argument("--table", action="store_true", help="aggregate table of all bundles")
    args = ap.parse_args()

    if args.table:
        _table()
        return 0
    if not args.task_id:
        ap.print_help()
        return 1

    score_updates, note_updates = {}, {}
    for item in args.score:
        if "=" not in item:
            print(f"bad --score {item!r} (want dim=value)")
            return 1
        dim, _, val = item.partition("=")
        try:
            score_updates[dim.strip()] = int(val)
        except ValueError:
            print(f"bad score value in {item!r}")
            return 1
    for item in args.note:
        if "=" not in item:
            print(f"bad --note {item!r} (want dim=text)")
            return 1
        dim, _, text = item.partition("=")
        note_updates[dim.strip()] = text.strip()

    review = _bundle(args.task_id, score_updates, note_updates, args.verdict, args.reviewer)
    return 0 if review is not None else 1


if __name__ == "__main__":
    sys.exit(main())
