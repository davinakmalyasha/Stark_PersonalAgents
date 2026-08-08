import sys
import sqlite3
import os

sys.stdout.reconfigure(encoding='utf-8')

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from compressed_checkpointer import CompressedSqliteSaver

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)

cursor = conn.cursor()
cursor.execute("SELECT thread_id FROM checkpoints ORDER BY checkpoint_id DESC LIMIT 1;")
row = cursor.fetchone()
if not row:
    print("No checkpoints found in DB.")
    sys.exit(0)
    
active_thread = row[0]
print(f"Active/Latest Thread: {active_thread}")

saver = CompressedSqliteSaver(conn)
config = {"configurable": {"thread_id": active_thread, "checkpoint_ns": ""}}
history = list(saver.list(config))

if history:
    latest_tup = history[0]
    checkpoint = latest_tup.checkpoint
    if checkpoint and "channel_values" in checkpoint:
        cv = checkpoint["channel_values"]
        print("Channel keys in latest checkpoint:")
        for k, v in cv.items():
            print(f"  - {k} ({type(v).__name__}): {str(v)[:150]}...")
conn.close()
