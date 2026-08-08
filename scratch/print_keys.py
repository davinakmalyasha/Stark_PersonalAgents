import sys
import sqlite3
import json

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)
from it_department_graph import app_graph

db_path = r"d:\MyProject\LangChain\.antigravity\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-1-%';")
threads = [r[0] for r in cursor.fetchall()]
conn.close()

if not threads:
    print("No threads found.")
    sys.exit(0)

latest_thread = sorted(threads)[-1]
config = {"configurable": {"thread_id": latest_thread}}
state = app_graph.get_state(config)
print("Keys in state.values:")
for k, v in state.values.items():
    print(f"  - {k} ({type(v).__name__}): {repr(str(v)[:300])}")
