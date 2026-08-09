"""Live smoke test for the free-only BSA spec agent (bsa_spec_agent.py).

Verifies BOTH modes the user cares about:
  A) FEATURE mode  — existing project files exist; research reads them and the spec
                     references existing code.
  B) GREENFIELD    — empty project; spec is designed from scratch.

Each run is limited to a few sections (default max_sections=2) so it finishes in
~1-2 minutes. Set SECTIONS=8 to do the full walk.

Usage:
    python scratch/test_bsa_free.py            # feature mode (2 sections)
    python scratch/test_bsa_free.py greenfield # greenfield mode (1 section)
"""
import os
import shutil
import sys
import tempfile

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from bsa_spec_agent import run_bsa_spec  # noqa: E402


def _make_feature_project(tmp):
    """A tiny existing codebase so research has real files to read."""
    app = os.path.join(tmp, "app")
    os.makedirs(app, exist_ok=True)
    with open(os.path.join(app, "server.js"), "w", encoding="utf-8") as f:
        f.write(
            "const express = require('express');\n"
            "const app = express();\n"
            "app.use(express.json());\n"
            "// GET /api/users returns a hardcoded list\n"
            "app.get('/api/users', (req, res) => res.json([{ id: 1, name: 'alice' }]));\n"
            "module.exports = app;\n"
        )
    with open(os.path.join(app, "users.js"), "w", encoding="utf-8") as f:
        f.write("// User repository — currently in-memory array.\n"
                "const users = [{ id: 1, name: 'alice', email: 'alice@example.com' }];\n"
                "module.exports = { users };\n")
    with open(os.path.join(app, "package.json"), "w", encoding="utf-8") as f:
        f.write('{ "name": "demo-app", "dependencies": { "express": "^4.19.0" } }\n')
    return tmp


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "feature"
    sections = int(os.environ.get("SECTIONS", "2"))

    tmp = tempfile.mkdtemp(prefix="bsa_free_test_")
    try:
        if mode == "greenfield":
            project = tmp
            task = "Build a todo list application with add and complete items"
            label = "GREENFIELD"
        else:
            project = _make_feature_project(tmp)
            task = "Add an edit-user endpoint to the existing app so a user's name and email can be updated"
            label = "FEATURE"

        print(f"\n{'='*70}\n[{label}] project={project}\n[{label}] task={task[:70]}\n"
              f"[{label}] max_sections={sections}\n{'='*70}\n")

        result = run_bsa_spec(task, project, chat_id=f"test-{mode}", max_sections=sections)
        print("\n=== RESULT ===\n" + result)

        spec_path = os.path.join(project, "specs")
        spec_files = [os.path.join(spec_path, f) for f in os.listdir(spec_path)] if os.path.isdir(spec_path) else []
        if not spec_files:
            print("\n!!! NO SPEC FILE WRITTEN")
            sys.exit(1)
        latest = max(spec_files, key=os.path.getmtime)
        print("\n=== SPEC FILE: " + latest + " ===")
        with open(latest, encoding="utf-8") as f:
            print(f.read())

        with open(latest, encoding="utf-8") as f:
            body = f.read()
        n_sections = body.count("\n## ")
        print(f"\n=== VERIFY: {n_sections} section header(s) in file; "
              f"{'existing-code reference found' if ('server.js' in body or 'users.js' in body) else 'no existing-code reference (expected for greenfield)'} ===")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        print(f"\n(cleaned up temp project {tmp})")


if __name__ == "__main__":
    main()
