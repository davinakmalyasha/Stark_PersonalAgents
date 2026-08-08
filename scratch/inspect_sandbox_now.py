import os

sandbox_dir = r"D:\MyProject\TestProjectForAgent"
print("=== Files in sandbox ===")
if os.path.exists(sandbox_dir):
    for root, dirs, files in os.walk(sandbox_dir):
        for file in files:
            full_path = os.path.join(root, file)
            rel_path = os.path.relpath(full_path, sandbox_dir)
            print(f" - {rel_path} ({os.path.getsize(full_path)} bytes)")
else:
    print("Sandbox does not exist.")

plan_path = os.path.join(sandbox_dir, "planning.md")
if os.path.exists(plan_path):
    print("\n=== planning.md ===")
    with open(plan_path, "r", encoding="utf-8") as f:
        print(f.read())
