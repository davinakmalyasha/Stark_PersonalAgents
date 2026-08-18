"""P9 router tests — standalone (no LLM, no real registry writes).

Run: venv312\\Scripts\\python.exe scratch\\test_project_router.py
"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import project_router as pr

FAIL = []


def check(name, cond, detail=""):
    status = "PASS" if cond else "FAIL"
    print(f"  [{status}] {name}{' — ' + detail if detail and not cond else ''}")
    if not cond:
        FAIL.append(name)


def test_creation_phrasing():
    print("[1] _creation_phrasing — creation stays with orchestrator LLM")
    check("new project called X", pr._creation_phrasing("lets make a new project called blabla, a todo app"))
    check("create a project named X", pr._creation_phrasing("please create a project named alpha"))
    check("set up project", pr._creation_phrasing("set up project called beta"))
    check("plain dev task NOT flagged", not pr._creation_phrasing("add login page to project B"))
    check("bug fix NOT flagged", not pr._creation_phrasing("fix the bug in the auth flow"))
    check("feature ask NOT flagged", not pr._creation_phrasing("for project B, add dark mode support"))


def test_route_request():
    print("[2] route_request — direct-to-Dev only for EXISTING registered projects")
    fake_projects = [
        {"name": "Alpha", "workspace_id": "ws-a", "workspace_path": r"D:\MyProject\Alpha",
         "chat_id": "chat-a", "child_id": "cont-chat-a-Dev-1234abcd"},
        {"name": "Beta", "workspace_id": "ws-b", "workspace_path": r"D:\MyProject\Beta",
         "chat_id": "chat-b", "child_id": "cont-chat-b-Dev-5678efgh"},
    ]
    orig_list = pr.list_projects
    pr.list_projects = lambda: fake_projects
    try:
        # inside a project chat
        kind, p = pr.route_request("add a login page", r"D:\MyProject\Alpha", "chat-a")
        check("in-project chat -> project_dev Alpha", kind == "project_dev" and p["name"] == "Alpha")
        # explicit mention from General chat
        kind, p = pr.route_request("for project Beta, add an api route", "", "chat-gen")
        check("mention 'project Beta' -> project_dev Beta", kind == "project_dev" and p["name"] == "Beta")
        kind, p = pr.route_request("in project Alpha please fix the tests", "", "chat-gen")
        check("mention 'project Alpha' -> project_dev Alpha", kind == "project_dev" and p["name"] == "Alpha")
        # creation phrasing from inside a project chat -> orchestrator (LLM decides)
        kind, p = pr.route_request("make a new project called Gamma", r"D:\MyProject\Alpha", "chat-a")
        check("creation phrasing -> orchestrator", kind == "orchestrator" and p is None)
        # no match -> orchestrator
        kind, p = pr.route_request("what is the meaning of life?", "", "chat-gen")
        check("no match -> orchestrator", kind == "orchestrator" and p is None)
        # unknown project name -> orchestrator (never creates from keywords)
        kind, p = pr.route_request("can you make project Zeta for me", "", "chat-gen")
        check("unknown project -> orchestrator", kind == "orchestrator" and p is None)
        # project with NO dev child yet -> orchestrator (it assigns one)
        fake_projects.append({"name": "Zulu", "workspace_id": "ws-z", "workspace_path": r"D:\MyProject\Zulu",
                              "chat_id": "chat-z", "child_id": ""})
        kind, p = pr.route_request("work on project Zulu", r"D:\MyProject\Zulu", "chat-z")
        check("project without child -> orchestrator", kind == "orchestrator" and p is None)
    finally:
        pr.list_projects = orig_list


def test_index_roundtrip():
    print("[3] index round-trip (isolated temp file)")
    with tempfile.TemporaryDirectory() as tmp:
        orig_idx = pr.INDEX_PATH
        pr.INDEX_PATH = os.path.join(tmp, "project_agents.json")
        try:
            data = pr._load_index()
            check("empty index loads", data == {"projects": []})
            data["projects"].append({"name": "Test", "child_id": "x"})
            pr._save_index(data)
            again = pr._load_index()
            check("round-trip persists", again["projects"][0]["child_id"] == "x")
        finally:
            pr.INDEX_PATH = orig_idx


def test_sanitize():
    print("[4] _sanitize_name")
    check("strips path separators", pr._sanitize_name("a/b\\c:d") == "a_b_c_d")
    check("trims", pr._sanitize_name("  hello  ") == "hello")
    check("empty stays empty", pr._sanitize_name("") == "")


if __name__ == "__main__":
    test_creation_phrasing()
    test_route_request()
    test_index_roundtrip()
    test_sanitize()
    print()
    if FAIL:
        print(f"FAILED: {len(FAIL)} — {', '.join(FAIL)}")
        sys.exit(1)
    print("ALL PASS")
