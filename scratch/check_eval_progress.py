import re

log_path = r"C:\Users\MY LENOVO\.gemini\antigravity\brain\51c27cd1-5d9d-4c63-8c54-bdfc913b753e\.system_generated\tasks\task-3969.log"
try:
    with open(log_path, "r", encoding="utf-8") as f:
        content = f.read()
    
    # Find all "RUNNING UNIT TASK" or "RUNNING INTERACTIVE TASK"
    runs = re.findall(r"RUNNING (?:UNIT|INTERACTIVE) TASK \d+: .*", content)
    completed_passes = re.findall(r"ID\s+\|\s+Task Name.*?\n(?:.*?\n)+", content)
    
    print("Tasks started so far:")
    for run in runs:
        print(f" - {run}")
    
    # Get last 15 lines of the log to see immediate details
    lines = content.splitlines()
    print("\nLast 15 lines of log:")
    for line in lines[-15:]:
        print(line)
except Exception as e:
    print(f"Error reading log: {e}")
