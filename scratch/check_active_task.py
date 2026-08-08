import sqlite3
import json

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)
c = conn.cursor()

c.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-3-%';")
threads = [r[0] for r in c.fetchall()]
print("Found unit-chat-3 threads:", threads)

for t in threads:
    c.execute("SELECT COUNT(*) FROM checkpoints WHERE thread_id = ?;", (t,))
    count = c.fetchone()[0]
    print(f"Thread {t} checkpoints count: {count}")
    
    # Get the latest checkpoint metadata
    c.execute("SELECT checkpoint_id, metadata FROM checkpoints WHERE thread_id = ? ORDER BY checkpoint_id DESC LIMIT 1;", (t,))
    row = c.fetchone()
    if row:
        print(f"Latest checkpoint ID: {row[0]}")
        try:
            meta = json.loads(row[1])
            print("Metadata:", json.dumps(meta, indent=2))
        except Exception as e:
            print(f"Error decoding metadata: {e}")

conn.close()
