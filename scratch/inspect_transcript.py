import json
import os
import sys

sys.stdout.reconfigure(encoding='utf-8')

# The transcript is located in the conversation's log directory
log_dir = r"C:\Users\MY LENOVO\.gemini\antigravity\brain\ab3f8f02-4cb0-4956-b920-a4bdeaac8a12\.system_generated\logs"
transcript_path = os.path.join(log_dir, "transcript.jsonl")

if not os.path.exists(transcript_path):
    print(f"Transcript does not exist at: {transcript_path}")
    sys.exit(0)

print("Reading transcript...")
steps = []
with open(transcript_path, "r", encoding="utf-8") as f:
    for line in f:
        try:
            steps.append(json.loads(line))
        except Exception:
            pass

print(f"Total steps in transcript: {len(steps)}")

# We want to find the latest steps where the model made tool calls or returned responses.
# Specifically, we look for LLM calls (usually in tools or run_command)
# Or we search for steps containing "DeveloperFixing" or "Iter"
for idx in range(len(steps) - 1, -1, -1):
    step = steps[idx]
    content = str(step.get("content", ""))
    # Look for model tool calls or responses
    tool_calls = step.get("tool_calls", [])
    if tool_calls:
        print(f"\n==================================================")
        print(f"STEP INDEX {step.get('step_index')} | Type: {step.get('type')} | Status: {step.get('status')}")
        print(f"Tool calls:")
        for tc in tool_calls:
            print(f"  - {tc.get('name')} with args: {str(tc.get('args'))[:200]}")
        if content:
            print(f"Content preview:\n{content[:1000]}")
        print("==================================================")
        
        # Stop after printing 5 steps with tool calls
        if idx < len(steps) - 30:
            break
