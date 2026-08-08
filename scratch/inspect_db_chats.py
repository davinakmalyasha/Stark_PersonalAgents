import sqlite3
import json
import zlib
import sys

sys.stdout.reconfigure(encoding='utf-8')

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()

# Get all threads
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints;")
threads = [r[0] for r in cursor.fetchall()]
print(f"Threads in DB: {threads}")

for thread_id in sorted(threads):
    print(f"\n==================================================")
    print(f"THREAD: {thread_id}")
    print(f"==================================================")
    # Get latest checkpoint for this thread
    cursor.execute("SELECT checkpoint_id, checkpoint FROM checkpoints WHERE thread_id = ? ORDER BY checkpoint_id DESC LIMIT 1;", (thread_id,))
    row = cursor.fetchone()
    if not row:
        continue
    checkpoint_id, checkpoint_bytes = row
    print(f"Checkpoint ID: {checkpoint_id}")
    
    try:
        if checkpoint_bytes.startswith(b'\x78\x9c'):
            checkpoint_str = zlib.decompress(checkpoint_bytes).decode('utf-8')
        else:
            checkpoint_str = checkpoint_bytes.decode('utf-8')
        checkpoint_data = json.loads(checkpoint_str)
    except Exception as e:
        print(f"Failed to decompress checkpoint: {e}")
        continue
        
    channel_values = checkpoint_data.get("channel_values", {})
    # Let's inspect keys in channel_values
    print("Channel keys:", list(channel_values.keys()))
    
    # Check messages
    messages = channel_values.get("messages", [])
    print(f"Number of messages: {len(messages)}")
    
    # Print type and length of last few messages
    for i, msg in enumerate(messages[-8:]):
        m_idx = len(messages) - 8 + i
        # In langchain/langgraph, msg is serialized. Let's see its structure
        print(f"Msg {m_idx}: Type={type(msg)}")
        try:
            print(f"  Content: {str(msg)[:200]}...")
        except Exception:
            pass

conn.close()
