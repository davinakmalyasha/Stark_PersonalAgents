import sys
import os
sys.path.append(r"d:\MyProject\LangChain")
print("Importing workspace_manager...")
import workspace_manager as wm
print("Imported workspace_manager.")

print("Calling init_default_workspace()...")
ws = wm.init_default_workspace()
print("init_default_workspace() returned:", ws)
