"""Probe: reproduce the drill's read_file failure with Dev-like deps."""
import os
import sys
import traceback

ROOT = r"D:\MyProject\LangChain"
os.chdir(ROOT)
sys.path.insert(0, ROOT)
os.environ.setdefault("HARNESS_ROOT", ROOT)

TARGET = os.path.join(ROOT, "scratch", "agents_works", "design")

from llm.deps import WorkspaceDeps
from tools import execute_tool_detailed

deps = WorkspaceDeps.from_shared_state(role="Developer", chat_id="probe-1",
                                       project_path=TARGET)

for label, args in [
    ("repo-relative skill", {"file_path": ".agents/skills/ui-ux-design/SKILL.md"}),
    ("abs skill", {"file_path": os.path.join(ROOT, ".agents", "skills", "ui-ux-design", "SKILL.md")}),
    ("drill-local probe", {"file_path": "scratch/agents_works/design/nonexistent.txt"}),
]:
    try:
        res = execute_tool_detailed("read_file", args, deps)
        s = str(res)
        print(f"--- {label}: OK len={len(s)} head={s[:160]!r}")
    except Exception as e:
        print(f"--- {label}: RAISED {type(e).__name__}: {e}")
        traceback.print_exc(limit=3)
