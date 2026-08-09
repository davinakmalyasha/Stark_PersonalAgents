import os
import json
import shutil
import workspace_manager as wm

# Use a temporary registry path for testing to avoid overwriting the real workspaces.json
temp_registry = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "workspaces_test_temp.json")
wm.REGISTRY_PATH = temp_registry

def cleanup():
    if os.path.exists(temp_registry):
        os.remove(temp_registry)

try:
    # Test case 1: Completely empty/missing workspaces registry
    cleanup()
    print("Test 1: Registry file is missing.")
    res = wm.init_default_workspace()
    print(f"Result (should be LangChain workspace dict): {res}")
    workspaces = wm.list_workspaces()
    print(f"Workspaces registered (should be 1): {len(workspaces)}")
    assert len(workspaces) == 1
    assert workspaces[0]["name"] == "LangChain"

    # Test case 2: LangChain is deleted but another workspace exists
    print("\nTest 2: LangChain deleted, but another workspace exists.")
    # Manually overwrite registry to simulate another workspace exists, and LangChain is deleted
    reg = {
        "workspaces": [
            {
                "id": "ws-dummy",
                "name": "DummyProject",
                "path": r"D:\MyProject\DummyProject",
                "addedAt": "2026-06-26T00:00:00"
            }
        ]
    }
    with open(temp_registry, "w", encoding="utf-8") as f:
        json.dump(reg, f, indent=2)

    res = wm.init_default_workspace()
    print(f"Result (should be None): {res}")
    assert res is None
    
    workspaces = wm.list_workspaces()
    print(f"Workspaces registered (should still be 1, i.e., only DummyProject): {len(workspaces)}")
    assert len(workspaces) == 1
    assert workspaces[0]["id"] == "ws-dummy"

    # Test case 3: LangChain already exists
    print("\nTest 3: LangChain already exists in registry.")
    reg["workspaces"].append({
        "id": "ws-langchain",
        "name": "LangChain",
        "path": r"d:\MyProject\LangChain",
        "addedAt": "2026-06-26T00:01:00"
    })
    with open(temp_registry, "w", encoding="utf-8") as f:
        json.dump(reg, f, indent=2)

    res = wm.init_default_workspace()
    print(f"Result (should be LangChain workspace dict): {res}")
    assert res is not None
    assert res["id"] == "ws-langchain"

    print("\nAll tests passed successfully!")

finally:
    cleanup()
