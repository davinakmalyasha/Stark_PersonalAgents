import os
import sys

os.environ["HARNESS_ROOT"] = r"D:\MyProject\LangChain"
sys.path.insert(0, r"D:\MyProject\LangChain")

from routing.router import decide_route

cases = [
    # (prompt, ws, chat, kind_hint, expected_kind)
    ("x", "", "", "house", "house"),
    ("x", "", "", "personal", "general"),
    ("x", "", "", "dfd", "dfd"),
    ("x", "", "", "design", "design"),
    ("x", "", "", "DESIGN_WORKSPACE", "design"),
    ("x", "", "", "design_workspace", "design"),
    ("hello", "", "", "", "orchestrator"),
]
ok = True
for prompt, ws, chat, hint, expected in cases:
    got = decide_route(prompt, ws, chat, hint)["kind"]
    mark = "OK " if got == expected else "FAIL"
    if got != expected:
        ok = False
    print(f"{mark} hint={hint!r:20} -> {got} (expect {expected})")

# stream_for_kind dispatches design without touching other branches
import inspect
from routing import router as R
src = inspect.getsource(R.stream_for_kind)
assert "run_ui_design_direct" in src, "design branch missing"
assert src.index('kind == "design"') > src.index('kind == "dfd"'), "order changed?"
print("OK  stream_for_kind has additive design branch after dfd")
print("ALL OK" if ok else "FAILURES PRESENT")
sys.exit(0 if ok else 1)
