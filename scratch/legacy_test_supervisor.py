import os
import sys
sys.path.append(r'd:\MyProject\LangChain')

from dotenv import load_dotenv
load_dotenv()

from state_sync import shared_state, safe_get_state
from supervisor_agent import supervisor_node

# Set up test state
state = {
    "client_request": "lets make something",
    "requirements": "",
    "tech_spec": "",
    "code": "",
    "test_report": "",
    "error_count": 0,
    "next_agent": "",
    "project_path": r"D:\MyProject\TestProjectForAgent",
    "agents_plan": "",
    "active_tasks": [],
    "requirements_updated": False,
    "tech_spec_updated": False,
    "code_updated": False,
}

print("Running supervisor node...")
res = supervisor_node(state)

# Write output to UTF-8 file
output_path = r"d:\MyProject\LangChain\scratch\test_supervisor_output.txt"
with open(output_path, "w", encoding="utf-8") as f:
    f.write("Supervisor node returned: " + str(res) + "\n\n")
    f.write("Supervisor thoughts in shared_state: " + str(shared_state["thoughts"]["supervisor"]) + "\n\n")
    f.write("Live terminal log in shared_state:\n")
    f.write(shared_state["live_terminal_log"] + "\n")

print("Done. Output written to scratch\\test_supervisor_output.txt")
