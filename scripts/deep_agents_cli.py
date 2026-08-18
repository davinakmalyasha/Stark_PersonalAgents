import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)
import sys
import time
from dotenv import load_dotenv
from sync.sync_manager import run_and_sync_graph
from sync.state_sync import shared_state

load_dotenv(override=True)

def print_banner():
    print("=" * 65)
    print("=== DEEP AGENTS MULTI-AGENT IT DEPARTMENT CLI ===")
    print("=" * 65)

def main():
    print_banner()
    req = sys.argv[1] if len(sys.argv) > 1 else input("Enter your request: ").strip()
    if not req:
        print("Error: Request cannot be empty.")
        return

    init_state = {
        "client_request": req, "requirements": "", "tech_spec": "",
        "code": "", "test_report": "", "devops_config": "", "analytics_report": "",
        "error_count": 0, "next_agent": "", "agents_plan": "", "active_tasks": []
    }

    shared_state.update({
        "active_node": "supervisor", "next_agent": "", "completed_nodes": [],
        "thoughts": {
            "supervisor": "Evaluating...", "ba": "", "sa": "", 
            "developer": "", "tester": "", "devops": "", "analytics": ""
        },
        "client_request": req,
        "outputs": {
            "requirements": "", "tech_spec": "", "code": "", 
            "test_report": "", "devops_config": "", "analytics_report": ""
        },
        "agents_plan": "",
        "active_tasks": [],
        "deep_agents_log": []
    })

    print(f"\n[Client Request Received]: \"{req}\"\n")
    last_log_len = 0

    for state in run_and_sync_graph(req, chat_id="cli_thread"):
        node_name = state.get("active_node", "agent")
        print(f"\n[Step: {node_name.upper()}] status update.")
        thought = state.get("thoughts", {}).get("agent") or state.get("thoughts", {}).get("orchestrator") or state.get("thoughts", {}).get("developer")
        if thought:
            print(f"Thought: {thought}")

        current_logs = state.get("deep_agents_log", [])
        if len(current_logs) > last_log_len:
            for idx in range(last_log_len, len(current_logs)):
                log = current_logs[idx]
                print("-" * 65)
                print(f"[{log['sender']}] prompted Deep Agents:")
                print(f"Prompt:\n{log['prompt'][:150]}...")
                print(f"Deep Agents Response Preview:\n{log['response'][:150]}...")
                print("-" * 65)
            last_log_len = len(current_logs)

    print("\n" + "=" * 65)
    print("=== WORKFLOW COMPLETED SUCCESSFULLY ===")
    print("=" * 65)

if __name__ == "__main__":
    main()
