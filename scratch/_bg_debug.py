import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tools import run_command
from tools.MainTool import _procman


def http0_pids():
    out = subprocess.run(
        ["powershell", "-NoProfile", "-Command",
         "Get-CimInstance Win32_Process -Filter \"Name='python.exe'\" | "
         "Where-Object { $_.CommandLine -match 'http.server 0' } | "
         "Select-Object -ExpandProperty ProcessId"],
        capture_output=True, text=True, timeout=30,
    )
    return {int(x) for x in out.stdout.split() if x.strip().isdigit()}


before = http0_pids()
out = run_command(command="python -u -m http.server 0", timeout=2000, idle_timeout=1500)
print("result:", out.replace("\n", " | "))
name = "python"
entry = _procman._lookup(name)
if not entry:
    print("NO ENTRY")
    sys.exit(1)
pid = entry["proc"].pid
try:
    import psutil
    p = psutil.Process(pid)
    print("parent alive:", p.is_running(), "status:", p.status())
    kids = p.children(recursive=True)
    print("children:", [(c.pid, c.name(), c.status()) for c in kids])
except Exception as e:
    print("psutil parent error:", repr(e))
print("kill:", run_command(command=name, process_action="kill"))
time.sleep(1.0)
after = http0_pids()
print("new leftovers:", sorted(after - before))
