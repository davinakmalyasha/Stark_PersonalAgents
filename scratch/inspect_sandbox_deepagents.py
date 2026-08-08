import os
import sys

sys.stdout.reconfigure(encoding='utf-8')

path = r"D:\MyProject\TestProjectForAgent\.deep_agents"
if os.path.exists(path):
    print(f"Listing all files recursively in {path}:")
    for root, dirs, files in os.walk(path):
        for f in files:
            full_path = os.path.join(root, f)
            print(f"  {f} | Size: {os.path.getsize(full_path)} bytes | Modified: {os.path.getmtime(full_path)}")
else:
    print(f"Path does not exist: {path}")
