import sqlite3
import json

db_path = r"d:\MyProject\LangChain\.antigravity\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints;")
threads = [r[0] for r in cursor.fetchall()]

print("Token usage and cost per thread in checkpoints.db:")
print(f"{'Thread ID':<35} | {'Input Tokens':<12} | {'Output Tokens':<12} | {'Cost':<8}")
print("-" * 75)

total_in = 0
total_out = 0
total_cost = 0.0

for thread in sorted(threads):
    cursor.execute("SELECT checkpoint FROM checkpoints WHERE thread_id = ? ORDER BY checkpoint_id DESC LIMIT 1;", (thread,))
    row = cursor.fetchone()
    if not row:
        continue
    
    # decode checkpoint
    import zlib
    checkpoint_bytes = row[0]
    try:
        if checkpoint_bytes.startswith(b'\x78\x9c'):
            checkpoint_str = zlib.decompress(checkpoint_bytes).decode('utf-8')
        else:
            checkpoint_str = checkpoint_bytes.decode('utf-8')
        checkpoint_data = json.loads(checkpoint_str)
    except Exception:
        continue
        
    channel_values = checkpoint_data.get("channel_values", {})
    # Get token usage stats if stored
    # Note: token_usage is stored in state of run_evals or similar
    # Let's inspect all values in channel_values
    input_tok = 0
    output_tok = 0
    cost = 0.0
    
    # If the thread ID is a unit-chat or inter-chat, let's check its values
    if "chat" in thread:
        # In our ProjectState, token usage is in shared_state or stored in a JSON file
        # Let's look if we can extract it from the database or the task.json file.
        pass
    
    # Let's read the task.json files in the sandbox to see their stats!

import os
sandbox_dir = r"D:\MyProject\TestProjectForAgent"
task_files = []
if os.path.exists(sandbox_dir):
    antigravity_dir = os.path.join(sandbox_dir, ".antigravity")
    if os.path.exists(antigravity_dir):
        for file in os.listdir(antigravity_dir):
            if file.startswith("task_") and file.endswith(".json"):
                task_files.append(os.path.join(antigravity_dir, file))

print("\nTask JSON files stats:")
print(f"{'File':<35} | {'Status':<10} | {'Steps':<5}")
print("-" * 55)
for tf in task_files:
    try:
        with open(tf, "r", encoding="utf-8") as f:
            data = json.load(f)
        print(f"{os.path.basename(tf):<35} | {data.get('status'):<10} | {len(data.get('steps', [])):<5}")
    except Exception as e:
        print(f"Error reading {tf}: {e}")
