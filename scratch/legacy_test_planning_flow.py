import os
import sys
sys.path.append(r'd:\MyProject\LangChain')

from dotenv import load_dotenv
load_dotenv()

from chat_memory_manager import resolve_request_text
from supervisor_agent import supervisor_node
from state_sync import shared_state

def test_flow():
    project_path = r"D:\MyProject\TestProjectForAgent"
    chat_id = "test-chat-id"

    # Clean up any leftover planning.md in the test folder to start clean
    planning_md_path = os.path.join(project_path, "planning.md")
    if os.path.isfile(planning_md_path):
        os.remove(planning_md_path)

    print("=== Test 1: resolve_request_text with plan command ===")
    prompt = "then lets go /plan , u the one that plan"
    resolved = resolve_request_text(prompt, workspace_path=project_path, chat_id=chat_id)
    print(f"Original: '{prompt}'")
    print(f"Resolved: '{resolved}'")
    assert "/plan" in resolved, "Error: resolved prompt lost /plan!"
    print("Test 1 Passed!")

    print("\n=== Test 2: Supervisor first turn (code_updated = False) ===")
    state = {
        "client_request": resolved,
        "requirements": "",
        "tech_spec": "",
        "code": "",
        "test_report": "",
        "error_count": 0,
        "next_agent": "",
        "project_path": project_path,
        "agents_plan": "",
        "active_tasks": [],
        "requirements_updated": False,
        "tech_spec_updated": False,
        "code_updated": False,
    }
    res = supervisor_node(state)
    print("Supervisor returned:", res)
    assert res["next_agent"] == "developer", "Error: Supervisor did not route to developer for planning!"
    print("Test 2 Passed!")

    print("\n=== Test 3: Supervisor second turn (code_updated = True, planning.md missing) ===")
    state["code_updated"] = True
    state["code"] = "--- ## 📊 Codebase Exploration ---\n\nSome exploration summary here..."
    
    # Reset shared state logs
    shared_state["live_terminal_log"] = ""
    res = supervisor_node(state)
    print("Supervisor returned:", res)
    assert res["next_agent"] == "finish", "Error: Supervisor did not finish when planning.md was missing!"
    print("Terminal log output:\n", shared_state["live_terminal_log"].encode("ascii", "replace").decode("ascii"))
    assert "Warning: planning.md was not generated" in shared_state["live_terminal_log"], "Error: Missing warning message!"
    print("Test 3 Passed!")

    print("\n=== Test 4: Execution safeguard when planning.md is missing ===")
    exec_prompt = "Execute the plan from planning.md for: then lets go , u the one that plan"
    state_exec = {
        "client_request": exec_prompt,
        "requirements": "",
        "tech_spec": "",
        "code": "",
        "test_report": "",
        "error_count": 0,
        "next_agent": "",
        "project_path": project_path,
        "agents_plan": "",
        "active_tasks": [],
        "requirements_updated": False,
        "tech_spec_updated": False,
        "code_updated": False,
    }
    shared_state["live_terminal_log"] = ""
    res = supervisor_node(state_exec)
    print("Supervisor returned:", res)
    assert res["next_agent"] == "finish", "Error: Supervisor did not halt execution when planning.md is missing!"
    assert "Error: No valid implementation plan" in shared_state["live_terminal_log"], "Error: Missing safeguard error message!"
    print("Terminal log output:\n", shared_state["live_terminal_log"].encode("ascii", "replace").decode("ascii"))
    print("Test 4 Passed!")

if __name__ == "__main__":
    test_flow()
