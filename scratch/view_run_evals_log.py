log_path = r"C:\Users\MY LENOVO\.gemini\antigravity\brain\51c27cd1-5d9d-4c63-8c54-bdfc913b753e\.system_generated\tasks\task-3913.log"
try:
    with open(log_path, "r", encoding="utf-8") as f:
        lines = f.readlines()
    print("Last 150 lines:")
    for line in lines[-150:]:
        print(line, end="")
except Exception as e:
    print(f"Error: {e}")
