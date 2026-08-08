import sys
import sqlite3
import json

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints;")
threads = [r[0] for r in cursor.fetchall()]
conn.close()

if not threads:
    print("No threads found.")
    sys.exit(0)

# Sort threads by their timestamp suffix if they end with digits
def thread_key(t):
    parts = t.split("-")
    try:
        return int(parts[-1])
    except ValueError:
        return 0

latest_thread = sorted(threads, key=thread_key)[-1]
print(f"All threads: {threads}")
print(f"Latest thread: {latest_thread}")

# Let's inspect the latest checkpoints for this thread_id
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT checkpoint FROM checkpoints WHERE thread_id = ? ORDER BY checkpoint_id DESC LIMIT 1;", (latest_thread,))
row = cursor.fetchone()
conn.close()

if row:
    import pickle
    try:
        checkpoint = pickle.loads(row[0])
        print(f"Checkpoint keys: {checkpoint.keys()}")
        if "channel_values" in checkpoint:
            messages = checkpoint["channel_values"].get("messages", [])
            print(f"Number of messages: {len(messages)}")
            for idx, m in enumerate(messages):
                print(f"[{idx}] {type(m).__name__}: {str(m.content)[:300]}")
    except Exception as e:
        print(f"Error loading checkpoint pickle: {e}")
