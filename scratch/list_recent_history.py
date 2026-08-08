import os
import time
import sys

sys.stdout.reconfigure(encoding='utf-8')

dirs = [
    r"d:\MyProject\LangChain\.deep_agents\conversation_history",
    r"d:\MyProject\LangChain\.antigravity\conversation_history"
]

print("Recent history files (last 30 mins):")
now = time.time()
for d in dirs:
    if os.path.exists(d):
        for f in os.listdir(d):
            path = os.path.join(d, f)
            mtime = os.path.getmtime(path)
            if now - mtime < 1800: # 30 mins
                print(f"  {f} | Size: {os.path.getsize(path)} bytes | Modified: {time.ctime(mtime)} ({int(now - mtime)}s ago)")
