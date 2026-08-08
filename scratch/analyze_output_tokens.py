import sqlite3
import json
import zlib

db_path = r"d:\MyProject\LangChain\.antigravity\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-%' OR thread_id LIKE 'inter-chat-%';")
threads = [r[0] for r in cursor.fetchall()]

# We want to inspect the actual LLM responses if possible, or read the logs
# Let's inspect the latest thread's state history
if threads:
    latest_thread = sorted(threads)[-1]
    print(f"Analyzing output tokens for latest thread: {latest_thread}")
    cursor.execute("SELECT checkpoint FROM checkpoints WHERE thread_id = ? ORDER BY checkpoint_id ASC", (latest_thread,))
    rows = cursor.fetchall()
    
    for idx, r in enumerate(rows):
        checkpoint_bytes = r[0]
        try:
            if checkpoint_bytes.startswith(b'\x78\x9c'):
                checkpoint_str = zlib.decompress(checkpoint_bytes).decode('utf-8')
            else:
                checkpoint_str = checkpoint_bytes.decode('utf-8')
            checkpoint_data = json.loads(checkpoint_str)
        except Exception:
            continue
            
        channel_values = checkpoint_data.get("channel_values", {})
        code = channel_values.get("code", "")
        agent_report = channel_values.get("agent_report", "")
        test_report = channel_values.get("test_report", "")
        
        # If code or agent_report is large, print it
        if code and len(code) > 1000:
            print(f" - Step {idx} Code output size: {len(code)} chars")
        if agent_report and len(agent_report) > 1000:
            print(f" - Step {idx} Agent Report size: {len(agent_report)} chars")
            
conn.close()
