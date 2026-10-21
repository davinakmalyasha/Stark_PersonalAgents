"""Zero-dependency test runner for tests/test_*.py.

**pytest is the canonical runner** (``python -m pytest tests/ -q``). This one
exists for a zero-dependency path and for driving a single module by name, and
it deliberately SKIPS any test that needs a fixture rather than failing it —
it cannot supply arguments, so reporting ``TypeError: missing 1 required
positional argument`` as a FAIL was a lie about the code. Run pytest to
actually exercise those.

Usage:
    python tests/run_all.py              # everything runnable without fixtures
    python tests/run_all.py test_branch_levels test_ci_gate_push   # subset

Each test file exposes plain `test_*()` functions. A test:
    - returns normally            -> PASS
    - raises Skip                 -> SKIP
    - takes any required argument -> SKIP (needs pytest fixtures)
    - raises anything else        -> FAIL
Exit code: 0 iff no FAIL.
"""
import importlib.util
import inspect
import os
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

# The repo root must be importable too: a test module that does `import agents`
# or `from tools import ...` fails without it. Previously this only worked by
# accident — whichever module imported `_util` first added it as a side effect,
# so the alphabetically earlier modules failed and the later ones passed.
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)


def _fixture_params(fn) -> list[str]:
    """Names of parameters this runner cannot supply."""
    try:
        signature = inspect.signature(fn)
    except (TypeError, ValueError):
        return []
    missing = []
    for param in signature.parameters.values():
        if param.kind in (param.VAR_POSITIONAL, param.VAR_KEYWORD):
            continue
        if param.default is inspect.Parameter.empty:
            missing.append(param.name)
    return missing


def discover():
    files = []
    for fn in sorted(os.listdir(HERE)):
        if fn.startswith("test_") and fn.endswith(".py"):
            files.append(fn[:-3])
    if len(sys.argv) > 1:
        wanted = set(sys.argv[1:])
        files = [f for f in files if f in wanted]
    return files


def run_module(modname):
    path = os.path.join(HERE, modname + ".py")
    spec = importlib.util.spec_from_file_location(modname, path)
    mod = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(mod)
    except Exception as e:
        return [("module-load", "FAIL", f"{type(e).__name__}: {e}",
                 traceback.format_exc())]
    results = []
    for name in sorted(dir(mod)):
        if not name.startswith("test_"):
            continue
        fn = getattr(mod, name)
        if not callable(fn):
            continue
        needs = _fixture_params(fn)
        if needs:
            # Cannot be driven from here. Skipping is honest; reporting the
            # TypeError as a FAIL claimed the test was broken when it was the
            # runner that was.
            results.append((f"{modname}.{name}", "SKIP",
                            f"needs fixtures ({', '.join(needs)}) — run pytest", 0))
            continue
        t0 = time.time()
        try:
            fn()
            results.append((f"{modname}.{name}", "PASS", "", time.time() - t0))
        except ImportError as e:
            if "Skip" in str(type(e).__name__):
                results.append((f"{modname}.{name}", "SKIP", str(e), 0))
            else:
                results.append((f"{modname}.{name}", "FAIL",
                                f"{type(e).__name__}: {e}", traceback.format_exc()))
        except Exception as e:
            if type(e).__name__ == "Skip":
                results.append((f"{modname}.{name}", "SKIP", str(e) or "-", 0))
            else:
                results.append((f"{modname}.{name}", "FAIL",
                                f"{type(e).__name__}: {e}", traceback.format_exc()))
    return results


def main():
    mods = discover()
    if not mods:
        print("no test modules matched")
        return 1
    all_results = []
    for m in mods:
        all_results.extend(run_module(m))
    width = max(len(r[0]) for r in all_results) + 2
    counts = {"PASS": 0, "FAIL": 0, "SKIP": 0}
    for name, status, msg, extra in all_results:
        counts[status] += 1
        mark = {"PASS": ".", "FAIL": "F", "SKIP": "s"}[status]
        line = f"{mark} {name:<{width}} {status}"
        if status == "SKIP" and msg and msg != "-":
            line += f"  ({msg[:60]})"
        print(line)
        if status == "FAIL":
            for ln in (extra or "").strip().splitlines()[-6:]:
                print("    " + ln)
            if msg:
                print(f"    >> {msg[:300]}")
    dur = sum(r[3] for r in all_results if isinstance(r[3], (int, float)))
    print(f"\n{counts['PASS']} passed, {counts['FAIL']} failed, "
          f"{counts['SKIP']} skipped  ({dur:.1f}s)")
    return 1 if counts["FAIL"] else 0


if __name__ == "__main__":
    sys.exit(main())
