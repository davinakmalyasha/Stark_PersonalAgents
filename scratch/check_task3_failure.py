import sqlite3
import json
import sys

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)
from it_department_graph import app_graph

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-3-%' ORDER BY thread_id DESC LIMIT 1;")
row = cursor.fetchone()
if not row:
    print("No unit-chat-3 threads found.")
    sys.exit(0)
thread = row[0]
print(f"Latest thread: {thread}")
conn.close()

config = {"configurable": {"thread_id": thread}}
history = list(app_graph.get_state_history(config))

for s in reversed(history):
    metadata = s.metadata
    val = s.values
    step = metadata.get("step")
    node = metadata.get("node")
    test_rep = val.get("test_report")
    agent_rep = val.get("agent_report")
    
    if test_rep or agent_rep:
        # Write report to a utf-8 file to avoid console encoding issues
        with open("scratch/task3_err_report.txt", "w", encoding="utf-8") as f:
            f.write(f"Step: {step} | Node: {node}\n")
            f.write(f"Test Report:\n{test_rep}\n\n")
            f.write(f"Agent Report:\n{agent_rep}\n\n")
            dev_log = val.get("developer_tool_log")
            if dev_log:
                f.write(f"Developer Tool Log:\n{dev_log}\n")
        print("Wrote Task 3 failure details to scratch/task3_err_report.txt")
        break
