import sys
import os
import sqlite3
import json

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)
from it_department_graph import app_graph

# Find latest thread
db_path = r"d:\MyProject\LangChain\.antigravity\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-1-%';")
threads = [r[0] for r in cursor.fetchall()]
conn.close()

if not threads:
    print("No threads found.")
else:
    latest_thread = sorted(threads)[-1]
    print(f"Latest thread: {latest_thread}")
    config = {"configurable": {"thread_id": latest_thread}}
    history = list(app_graph.get_state_history(config))
    for s in reversed(history):
        metadata = s.metadata
        val = s.values
        step = metadata.get("step")
        node = metadata.get("node")
        print(f"\n=== Step: {step} | Node: {node} ===")
        print("thoughts:", val.get("thoughts"))
        print("next_agent:", val.get("next_agent"))
        print("code:", val.get("code"))
        print("agent_report:", val.get("agent_report"))
        print("test_report:", val.get("test_report"))

# Read sandbox planning.md
sandbox_plan = r"D:\MyProject\TestProjectForAgent\planning.md"
if os.path.exists(sandbox_plan):
    print(f"\n=== SANDBOX planning.md ===")
    with open(sandbox_plan, 'r', encoding='utf-8') as f:
        print(f.read())
else:
    print(f"\nSANDBOX planning.md does not exist at {sandbox_plan}")

# Check files in sandbox
sandbox_dir = r"D:\MyProject\TestProjectForAgent"
if os.path.exists(sandbox_dir):
    print(f"\n=== FILES IN SANDBOX ===")
    for root, dirs, files in os.walk(sandbox_dir):
        for file in files:
            full_path = os.path.join(root, file)
            rel_path = os.path.relpath(full_path, sandbox_dir)
            print(f" - {rel_path} ({os.path.getsize(full_path)} bytes)")
