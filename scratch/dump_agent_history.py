import sys
import sqlite3
import json
import zlib

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

db_path = r"d:\MyProject\LangChain\.antigravity\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-1-%';")
threads = [r[0] for r in cursor.fetchall()]

if not threads:
    print("No threads found.")
    sys.exit(0)

latest_thread = sorted(threads)[-1]
print(f"Dumping tool calls for thread: {latest_thread}")

cursor.execute("SELECT checkpoint FROM checkpoints WHERE thread_id = ? ORDER BY checkpoint_id ASC", (latest_thread,))
rows = cursor.fetchall()
conn.close()

for idx, r in enumerate(rows):
    checkpoint_bytes = r[0]
    # Check if zipped
    try:
        if checkpoint_bytes.startswith(b'\x78\x9c'):
            checkpoint_str = zlib.decompress(checkpoint_bytes).decode('utf-8')
        else:
            checkpoint_str = checkpoint_bytes.decode('utf-8')
        checkpoint_data = json.loads(checkpoint_str)
    except Exception as e:
        print(f"Error decoding checkpoint: {e}")
        continue
    
    channel_values = checkpoint_data.get("channel_values", {})
    thoughts = channel_values.get("thoughts", {})
    next_agent = channel_values.get("next_agent", "")
    agent_report = channel_values.get("agent_report", "")
    code = channel_values.get("code", "")
    
    # We want to see developer_tool_log or tool_call_log
    dev_tool_log = channel_values.get("developer_tool_log", "")
    if dev_tool_log:
        try:
            log_data = json.loads(dev_tool_log)
            print(f"\n--- Checkpoint {idx} | Next: {next_agent} ---")
            for item in log_data:
                print(f"  Iter {item.get('iteration')}: {item.get('tool')}({json.dumps(item.get('args'))}) -> {repr(str(item.get('result_preview'))[:120])}")
        except Exception:
            print(f"\n--- Checkpoint {idx} | Next: {next_agent} ---")
            print("  Log:", dev_tool_log[:300])
