import sys
import sqlite3
import os

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from compressed_checkpointer import CompressedSqliteSaver

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)
saver = CompressedSqliteSaver(conn)

thread_id = "unit-chat-5-1781457181"
config = {"configurable": {"thread_id": thread_id, "checkpoint_ns": ""}}
history = list(saver.list(config))

print(f"Total checkpoints: {len(history)}")

if history:
    # Let's inspect the first few checkpoints
    for i, tup in enumerate(history[:3]):
        checkpoint = tup.checkpoint
        metadata = tup.metadata
        print(f"\n--- Checkpoint index {i} ---")
        print(f"Keys: {list(checkpoint.keys()) if checkpoint else None}")
        if checkpoint and "channel_values" in checkpoint:
            cv = checkpoint["channel_values"]
            print(f"Channel values keys: {list(cv.keys())}")
            # If there is a messages key
            if "messages" in cv:
                msgs = cv["messages"]
                print(f"Messages count: {len(msgs)}")
                for idx, m in enumerate(msgs[-3:]):
                    content = m.content if hasattr(m, 'content') else str(m)
                    print(f"  [{idx}] {type(m).__name__}: {len(content)} chars: {content[:150]}")
            else:
                print("No messages key in channel_values")
        else:
            print("No channel_values in checkpoint")
else:
    print("No history found for thread.")

conn.close()
