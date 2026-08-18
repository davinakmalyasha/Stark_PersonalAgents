"""Add the 2 long-horizon build tasks to the gen9 suite (tasks.json)."""
import json, os, sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
ROOT = os.path.dirname(os.path.abspath(__file__))
GEN9 = os.path.join(ROOT, "swe_gen9")
path = os.path.join(GEN9, "tasks.json")
tasks = json.load(open(path, encoding="utf-8"))
tasks = [t for t in tasks if not t.get("build_task")]

BUILDS = [
    {
        "id": "g9r-saas-platform",
        "category": "gen9-real",
        "build_task": True,
        "lang": "node",
        "name": "SaaS platform",
        "prompt": (
            "Build a complete production-grade SaaS platform from scratch in this empty "
            "workspace. Stack: Node.js + Express + SQLite (node:sqlite) + a vanilla-JS SPA "
            "(no build step, no Docker, no ORM). Deliver:\n"
            "1. Auth & RBAC: register/login with hashed passwords, JWT sessions, /api/me; "
            "admin-only routes return 403 for regular users.\n"
            "2. Billing: subscribe to plans via a mock payment gateway, /api/billing/status; "
            "subscriptions persist.\n"
            "3. Background jobs: a scheduler that runs recurring jobs (e.g. daily digest) "
            "and records executed runs with results.\n"
            "4. SPA frontend in public/ (index.html + app.js + styles.css) talking to the API.\n"
            "5. A real unit test suite under tests/ (node --test).\n"
            "6. README.md with run instructions.\n"
            "The server must start with `node server.js`, honor PORT and DB_PATH env vars, "
            "and expose GET /api/health. npm install works (network available).\n"
            "(environment: Node.js and npm are pre-installed)"
        ),
        "budget": {"max_iters": 1000, "remaining_steps": 120, "max_cost_usd": 2.0},
    },
    {
        "id": "g9r-realtime-collab",
        "category": "gen9-real",
        "build_task": True,
        "lang": "python",
        "name": "Realtime collab app",
        "prompt": (
            "Build a complete realtime collaborative document app from scratch in this empty "
            "workspace. Stack: Python + FastAPI + SQLite (sqlite3) + WebSockets + a vanilla-JS "
            "frontend (no build step, no Docker, no ORM). Deliver:\n"
            "1. REST API: create/fetch/update documents (POST/GET/PUT /api/docs), persisted in SQLite.\n"
            "2. WebSocket endpoint /ws/{doc_id}: edits broadcast live to OTHER connected clients; "
            "presence events on connect/disconnect; every edit recorded in an edits table.\n"
            "3. GET /api/health.\n"
            "4. Frontend in static/ (index.html + app.js + styles.css): connect to a doc, edit, "
            "see remote edits + presence.\n"
            "5. A real unit test suite under tests/ (pytest).\n"
            "6. README.md with run instructions.\n"
            "The server must start with `python -m uvicorn app.main:app`, honor PORT and DB_PATH "
            "env vars.\n"
            "(environment: a ready-made venv with fastapi/uvicorn/pytest/websockets is at `.venv` — "
            "run `.venv/Scripts/python -m pytest tests/` and `.venv/Scripts/python -m uvicorn app.main:app`)"
        ),
        "budget": {"max_iters": 1000, "remaining_steps": 120, "max_cost_usd": 2.0},
    },
]

for b in BUILDS:
    b["snapshot"] = None
    b["testpatch"] = None
    b["src_files"] = []
    b["test_files"] = []
    b["repo"] = b["id"]
    tasks.append(b)

json.dump(tasks, open(path, "w", encoding="utf-8"), indent=2)
print(f"tasks.json now: {len(tasks)} tasks ({len(BUILDS)} builds + {len(tasks)-len(BUILDS)} bugs)")
