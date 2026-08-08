import os

legacy_files = [
    "test_async_subagents.py",
    "test_checkpointer_gc.py",
    "test_cost_optimization_24_26.py",
    "test_cost_optimization_5_8.py",
    "test_cost_optimization_9_13.py",
    "test_deconstruction_2_3.py",
    "test_planning_flow.py",
    "test_supervisor.py"
]

for filename in legacy_files:
    old_path = os.path.join("scratch", filename)
    new_path = os.path.join("scratch", f"legacy_{filename}")
    if os.path.exists(old_path):
        print(f"Renaming {old_path} -> {new_path}")
        try:
            os.rename(old_path, new_path)
        except Exception as e:
            print(f"Failed to rename {filename}: {e}")
    else:
        print(f"File {old_path} not found.")

print("Done renaming legacy tests.")
