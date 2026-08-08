import sys
import sqlite3
import json

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)
from it_department_graph import app_graph

db_path = r"d:\MyProject\LangChain\.antigravity\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'inter-chat-10-%';")
threads = [r[0] for r in cursor.fetchall()]
conn.close()

if not threads:
    print("No Task 10 threads found.")
    sys.exit(0)

latest_thread = sorted(threads)[-1]
print(f"Dumping state history for Task 10 thread: {latest_thread}")

config = {"configurable": {"thread_id": latest_thread}}
history = list(app_graph.get_state_history(config))

for s in reversed(history):
    metadata = s.metadata
    val = s.values
    step = metadata.get("step")
    node = metadata.get("node")
    print(f"\n==========================================")
    print(f"Step: {step} | Node: {node}")
    print("Next Agent:", val.get("next_agent"))
    print("Code updated:", val.get("code_updated"))
    print("Code:", repr(val.get("code")))
    print("Agent Report:", repr(val.get("agent_report")))
    print("Test Report:", repr(val.get("test_report")))
