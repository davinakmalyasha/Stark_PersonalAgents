"""P9 create_project offline smoke test — no LLM, temp dirs only.

Patches: PROJECTS_ROOT, INDEX_PATH, workspace registry, chat dir, and the
continuable spawner (returns a fake child). Verifies the full creation
plumbing: folder → workspace → chat → child assignment → index persistence.

Run: venv312\\Scripts\\python.exe scratch\\test_project_router_e2e.py
"""
import os
import shutil
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import project_router as pr
import workspace_manager as wm

FAIL = []


def check(name, cond, detail=""):
    status = "PASS" if cond else "FAIL"
    print(f"  [{status}] {name}{' — ' + detail if detail and not cond else ''}")
    if not cond:
        FAIL.append(name)


def main():
    tmp = tempfile.mkdtemp(prefix="pr_e2e_")
    projects_root = os.path.join(tmp, "projects")
    os.makedirs(projects_root, exist_ok=True)
    chats_root = os.path.join(tmp, "chats")
    os.makedirs(chats_root, exist_ok=True)

    orig = {
        "projects_root": pr.PROJECTS_ROOT,
        "index": pr.INDEX_PATH,
        "reg": wm.REGISTRY_PATH,
        "general": wm.GENERAL_CHAT_DIR,
        "probe": wm.probe_environment,
    }

    # Redirect every persistent store into the temp dir.
    pr.PROJECTS_ROOT = projects_root
    pr.INDEX_PATH = os.path.join(tmp, "project_agents.json")
    wm.REGISTRY_PATH = os.path.join(tmp, "workspaces.json")
    wm.GENERAL_CHAT_DIR = os.path.join(chats_root, "_general")
    wm._chat_dir = lambda ws: os.path.join(chats_root, wm._resolve_workspace_id(ws) or "_unregistered") if ws and ws != "_general" else wm.GENERAL_CHAT_DIR
    wm.probe_environment = lambda path: {}

    try:
        from agents from agents import subagent_manager
        orig_start = subagent_manager.start_continuable
        subagent_manager.start_continuable = lambda chat_id, ws, name, task, label=None, parent_session=None: {
            "child_id": f"cont-{chat_id}-Dev-fake1234",
            "result": "FAKE DEV FIRST REPLY",
            "status": "idle",
        }
        try:
            print("[1] create_project happy path")
            entry = pr.create_project("Smoke Test", "build a todo app", "chat-src")
            check("no error", not entry.get("error"), entry.get("error", ""))
            check("folder created", os.path.isdir(os.path.join(projects_root, "Smoke Test")))
            check("workspace id present", bool(entry.get("workspace_id")))
            check("chat id present", bool(entry.get("chat_id")))
            check("child id present", entry.get("child_id", "").startswith("cont-"))
            check("first reply captured", entry.get("first_reply") == "FAKE DEV FIRST REPLY")

            print("[2] index persisted")
            idx = pr._load_index()
            check("entry in index", any(p.get("name") == "Smoke Test" for p in idx["projects"]))

            print("[3] idempotent re-create (reuses same workspace)")
            entry2 = pr.create_project("Smoke Test", "again", "chat-src2")
            check("same workspace id reused", entry2.get("workspace_id") == entry.get("workspace_id"))

            print("[4] discovery finds the child")
            found = pr.find_project_by_workspace(os.path.join(projects_root, "Smoke Test"))
            check("find by workspace", found is not None and found.get("child_id") == entry.get("child_id"))

            print("[5] invalid names rejected")
            bad = pr.create_project("", "concept", "chat-src")
            check("empty name -> error", bool(bad.get("error")))
            bad2 = pr.create_project("general", "concept", "chat-src")
            check("reserved 'general' -> error", bool(bad2.get("error")))

            print("[6] route via registry after creation")
            kind, p = pr.route_request("improve the login flow", os.path.join(projects_root, "Smoke Test"), entry["chat_id"])
            check("in-project chat -> project_dev", kind == "project_dev" and p is not None)
            kind, p = pr.route_request("for project Smoke Test add tests", "", "chat-gen")
            check("name mention -> project_dev", kind == "project_dev" and p is not None)
        finally:
            subagent_manager.start_continuable = orig_start
    finally:
        pr.PROJECTS_ROOT = orig["projects_root"]
        pr.INDEX_PATH = orig["index"]
        wm.REGISTRY_PATH = orig["reg"]
        wm.GENERAL_CHAT_DIR = orig["general"]
        wm.probe_environment = orig["probe"]
        shutil.rmtree(tmp, ignore_errors=True)

    print()
    if FAIL:
        print(f"FAILED: {len(FAIL)} — {', '.join(FAIL)}")
        sys.exit(1)
    print("ALL PASS")


if __name__ == "__main__":
    main()
