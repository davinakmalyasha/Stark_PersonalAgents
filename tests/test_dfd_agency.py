"""Offline tests for the DFD agency layer (workspace, routing, client manager,
project-root guard). Pure stdlib + the real modules — no network, no LLM.

Run: python tests/test_dfd_agency.py
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import sys
import tempfile

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)

PASS = 0
FAIL = 0
FAILURES: list[str] = []


def check(name: str, cond: bool, extra: str = "") -> None:
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f". {name:<58} PASS")
    else:
        FAIL += 1
        FAILURES.append(f"{name} {extra}".strip())
        print(f"X {name:<58} FAIL {extra}")


def load_module(name: str, path: str):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


# ── isolate from the heavy tools facade: stub the top-level `tools` package
# only for modules that lazily import it (client_manager/project_router do
# not, but keep it for safety). Real submodules are loaded by path below.
import types

_stub = types.ModuleType("tools")
_stub._HARNESS_ROOT = REPO_ROOT
_stub.shared_state = {}
_stub.ask_user = lambda *a, **k: []
sys.modules.setdefault("tools", _stub)


def _assert_checks() -> None:
    """Raise if any check failed so the run_all runner reports a real FAIL."""
    if FAILURES:
        raise AssertionError(f"{len(FAILURES)} DFD checks failed: {FAILURES[:5]}")


def test_routing_dfd_kind():
    try:
        import routing.router as rr
    except Exception as e:
        check("routing import", False, str(e))
        _assert_checks()
        return
    d = rr.decide_route("check orders", workspace_kind="dfd")
    check("decide_route workspace_kind=dfd -> kind dfd", d.get("kind") == "dfd", str(d))
    d2 = rr.decide_route("hello", workspace_kind="personal")
    check("personal hint unaffected", d2.get("kind") == "general", str(d2))
    _assert_checks()


def test_workspace_registry_dfd():
    try:
        import server.store.workspace_registry as wr
    except Exception as e:
        check("workspace_registry import", False, str(e))
        _assert_checks()
        return
    check("DFD_WORKSPACE_ID == _dfd", wr.DFD_WORKSPACE_ID == "_dfd")
    check("_dfd in VIRTUAL_WORKSPACE_IDS", "_dfd" in wr.VIRTUAL_WORKSPACE_IDS)
    # list_workspaces is chat-store-bound; just confirm the virtual set contains it
    check("DFD_CHAT_DIR set", bool(wr.DFD_CHAT_DIR))
    _assert_checks()


def test_client_manager_gate():
    cm_path = os.path.join(REPO_ROOT, "agents", "client_manager.py")
    cm = load_module("client_manager_test", cm_path)

    # Monkeypatch the root + index to a temp dir so tests never touch real disk.
    tmp = tempfile.mkdtemp(prefix="dfd-test-")
    old_root, old_idx = cm.DFD_CLIENTS_ROOT, cm.INDEX_PATH
    cm.DFD_CLIENTS_ROOT = os.path.join(tmp, "DFDClientProjects")
    cm.INDEX_PATH = os.path.join(tmp, "dfd_clients.json")

    try:
        # Gate: no approval / weak markers are rejected.
        r0 = cm.create_client("Cafe X", "fnb")
        check("no approval -> error", bool(r0.get("error")), str(r0))
        r1 = cm.create_client("Cafe X", "fnb", approved_by="owner")
        check("approved_by=owner -> error", bool(r1.get("error")), str(r1))
        r2 = cm.create_client("Cafe X", "fnb", approved_by="auto")
        check("approved_by=auto -> error", bool(r2.get("error")), str(r2))

        # Invalid vertical rejected.
        r3 = cm.create_client("Cafe X", "not-a-vertical", approved_by="telegram:t1")
        check("invalid vertical -> error", bool(r3.get("error")), str(r3))

        # Real approval marker -> scaffold created.
        r4 = cm.create_client("Cafe X", "fnb", features=["online_order"],
                              whatsapp="+62812", approved_by="telegram:aprv-1")
        check("approved -> no error", not r4.get("error"), str(r4))
        check("approved -> path under root",
              r4.get("path", "").startswith(cm.DFD_CLIENTS_ROOT), r4.get("path", ""))
        for f in ("client.json", "progress.md"):
            check(f"{f} exists", os.path.isfile(os.path.join(r4["path"], f)))
        check("mem/ dir exists", os.path.isdir(os.path.join(r4["path"], "mem")))

        # Idempotent: same slug reuses.
        r5 = cm.create_client("Cafe X", "fnb", approved_by="telegram:aprv-2")
        check("idempotent reuse", bool(r5.get("already_existed")))

        # slugify sanity.
        check("slugify", cm._slugify("Warung Sari Café & Grill") == "warung-sari-caf-grill")
    finally:
        cm.DFD_CLIENTS_ROOT, cm.INDEX_PATH = old_root, old_idx
        shutil.rmtree(tmp, ignore_errors=True)
    _assert_checks()


def test_project_root_guard():
    pr_path = os.path.join(REPO_ROOT, "agents", "project_router.py")
    pr = load_module("project_router_test", pr_path)

    old_root, old_index = pr.PROJECTS_ROOT, pr.INDEX_PATH
    pr.PROJECTS_ROOT = r"D:\MyProject"
    tmp = tempfile.mkdtemp(prefix="dfd-root-")
    pr.INDEX_PATH = os.path.join(tmp, "project_agents.json")
    pr.DFD_CLIENTS_ROOT = os.path.join(tmp, "DFDClientProjects")

    try:
        # Outside DFDClientProjects -> rejected.
        r = pr.create_project("bad", "x", parent_root=r"D:\MyProject\Other")
        check("parent_root outside client root -> error", bool(r.get("error")), str(r))
        r2 = pr.create_project("bad2", "x", parent_root=r"D:\MyProject")
        check("parent_root = plain MyProject -> error", bool(r2.get("error")), str(r2))
        # Inside DFDClientProjects -> allowed (folder + workspace registration
        # needs the workspace_manager chain, which we avoid by pointing at a
        # nonexistent index; instead assert the guard passes and path builds).
        client_dir = os.path.join(pr.DFD_CLIENTS_ROOT, "cafe-x")
        os.makedirs(client_dir)
        # No workspace_manager in offline tests — the folder creation should
        # happen BEFORE the workspace setup. Verify guard accepted by calling
        # with wait=False and expecting an error from the missing workspace
        # manager (not from the root guard).
        r3 = pr.create_project("website", "build site", wait=False, parent_root=client_dir)
        check("parent_root inside client root passes guard (fails later, not at guard)",
              "parent_root must be inside" not in str(r3.get("error", "")), str(r3))
    finally:
        pr.PROJECTS_ROOT, pr.INDEX_PATH = old_root, old_index
        shutil.rmtree(tmp, ignore_errors=True)
    _assert_checks()


def test_dfd_scope_schema():
    # Load the real tools._schemas by file path (the top-level `tools`
    # package is stubbed for the harness, so import it directly).
    try:
        schemas = load_module(
            "schemas_real",
            os.path.join(REPO_ROOT, "tools", "_schemas.py"),
        )
    except Exception as e:
        check("tools._schemas import", False, str(e))
        return
    scope = schemas._SCOPE_MAP.get("dfd", [])
    check("dfd scope defined", bool(scope))
    check("dfd scope has dfd_bridge", "dfd_bridge" in scope)
    check("dfd scope has create_client", "create_client" in scope)
    check("dfd scope has create_project", "create_project" in scope)
    check("dfd scope has NO money tools", not ({"payment", "invoice"} & set(scope)))
    defs = [d["name"] for d in schemas.TOOL_DEFINITIONS]
    check("dfd_bridge defined in TOOL_DEFINITIONS", "dfd_bridge" in defs)
    check("create_client defined in TOOL_DEFINITIONS", "create_client" in defs)
    _assert_checks()


def main():
    for fn in (test_routing_dfd_kind, test_workspace_registry_dfd,
               test_client_manager_gate, test_project_root_guard,
               test_dfd_scope_schema):
        try:
            fn()
        except AssertionError as e:
            print(f"[MODULE] {fn.__name__} FAILED: {e}")
    print(f"\n{FAIL} failed, {PASS} passed")
    sys.exit(1 if FAIL else 0)


if __name__ == "__main__":
    main()
