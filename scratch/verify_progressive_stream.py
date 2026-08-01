"""Verifies the api_server SSE poller now emits progressive intermediate snapshots."""
import json
import threading
import time
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from state_sync import safe_append_live_log, safe_serialize_state, reset_shared_state

reset_shared_state()


def worker():
    lines = [
        '[THINKING] Supervisor: analyzing request',
        '[SUPERVISOR THOUGHTS] >> User wants a feature',
        '[THINKING] Supervisor: delegating to Developer',
        '[THINKING] Developer: reading files',
        '[THINKING] Developer: implementing',
        '[ASSISTANT] Done! Here is the answer.',
    ]
    for l in lines:
        safe_append_live_log(l)
        time.sleep(0.15)


t = threading.Thread(target=worker, daemon=True)
t.start()

emitted = []
last_ver = 0
deadline = time.time() + 3
while time.time() < deadline:
    snap = json.loads(safe_serialize_state())
    cur_ver = snap.get("state_version", 0)
    if cur_ver > last_ver:
        last_ver = cur_ver
        emitted.append(snap["live_terminal_log"])
    if "Here is the answer" in snap["live_terminal_log"]:
        break
    time.sleep(0.05)

print(f"Snapshots emitted: {len(emitted)}")
for i, log in enumerate(emitted):
    print(f"  [{i+1}] last line={log.strip().splitlines()[-1]!r}")

grew = all(len(emitted[i]) < len(emitted[i + 1]) for i in range(len(emitted) - 1))
assert grew and len(emitted) == 6, f"expected 6 growing snapshots, got {len(emitted)}"
assert "Here is the answer" in emitted[-1]
print("INTEGRATION TEST PASSED: all 6 thinking/answer lines stream progressively")
