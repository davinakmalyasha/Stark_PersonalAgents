"""preflight_gen2.py — verify every gen2 task is SOLVABLE before running the suite.

For each task:
  1. setup() the BUGGY starter in a throwaway dir
  2. run the hidden test          -> expect FAIL  (starter must not pass)
  3. write the task's `_fixed` files (hand-written correct version)
  4. run the hidden test again    -> expect PASS  (proves the task is solvable)

No LLM calls — this is pure authoring verification. Run:
    venv312/Scripts/python.exe scratch/preflight_gen2.py
"""
import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from eval_runner import _write, _ensure_deps, _require_test, _require_async_test, \
    _api_test, _py_test, _refactor_test, TASKS_BY_ID

import gen2_tasks  # noqa: F401  (ensures module builds)


def main():
    gen2 = [t for t in TASKS_BY_ID.values() if t.get("category", "").startswith("gen2")]
    gen3 = [t for t in TASKS_BY_ID.values() if t.get("category", "").startswith("gen3")]
    gen4 = [t for t in TASKS_BY_ID.values() if t.get("category", "").startswith("gen4")]
    gen5 = [t for t in TASKS_BY_ID.values() if t.get("category", "").startswith("gen5")]
    gen6 = [t for t in TASKS_BY_ID.values() if t.get("category", "").startswith("gen6")]
    gen7 = [t for t in TASKS_BY_ID.values() if t.get("category", "").startswith("gen7")]
    print(f"preflight: {len(gen2)} gen2, {len(gen3)} gen3, {len(gen4)} gen4, {len(gen5)} gen5, "
          f"{len(gen6)} gen6, {len(gen7)} gen7 tasks\n")
    results = []
    for task in sorted(gen2 + gen3 + gen4 + gen5 + gen6 + gen7, key=lambda t: t["id"]):
        tid = task["id"]
        tmp = tempfile.mkdtemp(prefix=f"preflight_{tid}_")
        try:
            task["setup"](tmp)
            ok_buggy, _ = task["test"](tmp)

            if not task.get("_fixed"):
                print(f"  {tid:<22} SKIP (no _fixed)")
                results.append(("SKIP", tid, ""))
                continue

            # Re-run setup first: some hidden tests MUTATE project files (e.g. a
            # stateful cache write) — the fixed run must start from pristine state,
            # not from whatever the buggy run left behind.
            task["setup"](tmp)
            _write(tmp, task["_fixed"])
            for rel in task.get("_remove", []):
                full = os.path.join(tmp, rel)
                if os.path.isdir(full):
                    shutil.rmtree(full, ignore_errors=True)
                elif os.path.isfile(full):
                    os.remove(full)
            # ensure deps again in case fixed version added express usage
            try:
                _ensure_deps(tmp)
            except Exception:
                pass
            ok_fixed, detail_fixed = task["test"](tmp)

            buggy_ok = "BUGGY-PASSES(!)" if ok_buggy else "buggy-fails-ok"
            fixed_ok = "FIXED-PASSES" if ok_fixed else f"FIXED-FAILS: {detail_fixed[:160]}"
            mark = "OK " if (not ok_buggy and ok_fixed) else "BAD"
            print(f"  {mark} {tid:<22} {buggy_ok:<20} {fixed_ok}")
            results.append(("OK" if (not ok_buggy and ok_fixed) else "BAD", tid,
                            detail_fixed if not ok_fixed else ""))
        except Exception as e:
            import traceback
            print(f"  !! {tid:<22} EXCEPTION: {e}")
            traceback.print_exc()
            results.append(("EXC", tid, str(e)))
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

    bad = [r for r in results if r[0] != "OK"]
    print(f"\n{len(results) - len(bad)}/{len(results)} tasks verified solvable")
    if bad:
        print("\nFAILING PRE-FLIGHT:")
        for st, tid, det in bad:
            print(f"  {st} {tid}  {det}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
