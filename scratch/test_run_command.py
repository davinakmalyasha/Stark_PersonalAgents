"""run_command checks: policy parsing, timeouts, tree kill, background lifecycle.

Run: venv\\Scripts\\python.exe scratch\\test_run_command.py
"""
import os
import re
import shutil
import sys
import tempfile
import time
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)

# Warm the tool facade at import time: the first `tools` import is ~10s cold
# and would otherwise land inside the first test's timing assertions.
from tools import run_command  # noqa: E402

FIXTURE = os.path.join(ROOT, "scratch", "_run_cmd_test_tmp")


def _run(**kw):
    from tools import run_command
    return run_command(**kw)


def _bg_name(text: str) -> str:
    m = re.search(r"(?:Started background process|moved to background)[^\n]*?'([^']+)'", text)
    if m:
        return m.group(1)
    m = re.search(r"'([^']+)' (?:still running|\(PID)", text)
    return m.group(1) if m else ""


class PolicyTest(unittest.TestCase):
    def _policy(self):
        from tools.MainTool._cmdpolicy import (
            check_command, is_read_only, is_server_like, needs_credentials,
        )
        return check_command, is_read_only, is_server_like, needs_credentials

    def test_destructive_blocked(self):
        check, *_ = self._policy()
        for cmd in (
            "rm -rf /",
            "rm -rf ~",
            'rm -rf "$HOME"',
            "rm -rf .",
            "git  push",
            'git "push"',
            "git push origin master",
            "git -C /repo push origin main",
            "rm -rf ./build && rm -rf /",
            "echo $(rm -rf /)",
            "powershell -Command \"Remove-Item -Recurse -Force C:\"",
            "reg delete HKLM\\Software\\X /f",
            "taskkill /F /IM node.exe",
            "rm -f /etc/hosts",
            r'rm -f "C:\Windows\System32\drivers\etc\hosts"',
            "rm -f C:Windows/System32/drivers/etc/hosts",
            "rm -f /c/Windows/System32/drivers/etc/hosts",
            ":(){ :|:& };:",
        ):
            with self.subTest(cmd=cmd):
                self.assertFalse(check(cmd).allowed, cmd)

    def test_benign_allowed(self):
        check, *_ = self._policy()
        for cmd in (
            "rm -rf ./node_modules",
            "rm -rf .next build dist",
            'echo "git push is dangerous"',
            "npm test",
            "git status && git log --oneline -5",
            "find . -name '*.py' | head -20",
        ):
            with self.subTest(cmd=cmd):
                self.assertTrue(check(cmd).allowed, cmd)

    def test_read_only(self):
        _check, is_ro, *_ = self._policy()
        self.assertTrue(is_ro("ls -la")[0])
        self.assertTrue(is_ro("git status && git log --oneline -5")[0])
        self.assertTrue(is_ro('python -c "import json; print(json.dumps({}))"')[0])
        self.assertFalse(is_ro("rm -rf build")[0])
        self.assertFalse(is_ro("echo hi > out.txt")[0])
        self.assertFalse(is_ro("npm install")[0])
        self.assertFalse(is_ro("python -c \"open('x','w').write('1')\"")[0])
        self.assertFalse(is_ro("bash -c 'ls'")[0])
        self.assertFalse(is_ro("find . -name x -delete")[0])
        self.assertTrue(is_ro("git config --get user.name")[0])
        self.assertFalse(is_ro("git config user.name someone")[0])
        self.assertTrue(is_ro("git branch -a")[0])
        self.assertFalse(is_ro("git branch new-feature")[0])

    def test_server_like(self):
        _c, _r, is_server, _n = self._policy()
        self.assertTrue(is_server("npm run dev"))
        self.assertTrue(is_server("npm run dev:web"))
        self.assertTrue(is_server("npm run dev -- --host 0.0.0.0"))
        self.assertTrue(is_server("npm --prefix web run dev"))
        self.assertTrue(is_server("npx vite --host"))
        self.assertTrue(is_server("vite --host"))
        self.assertTrue(is_server("python -m http.server 8000"))
        self.assertTrue(is_server("uvicorn app:app --reload"))
        self.assertTrue(is_server("docker compose up"))
        self.assertFalse(is_server("npm test"))
        self.assertFalse(is_server("pytest -q"))
        self.assertFalse(is_server("node script.js"))

    def test_needs_credentials(self):
        _c, _r, _s, needs = self._policy()
        self.assertTrue(needs("gh repo list"))
        self.assertTrue(needs("git fetch origin"))
        self.assertTrue(needs("curl -s https://api.github.com"))
        self.assertFalse(needs("python scripts/build.py"))
        self.assertFalse(needs("echo hello"))

    def test_expected_nonzero(self):
        from tools.MainTool._cmdpolicy import expected_nonzero
        self.assertEqual(expected_nonzero(1, "grep x f"), "no match")
        self.assertEqual(expected_nonzero(1, "git diff --quiet"), "git diff: differences")
        self.assertEqual(expected_nonzero(2, "grep x f"), "")
        self.assertEqual(expected_nonzero(1, "pytest -q"), "")

    def test_secret_reads_blocked(self):
        check, *_ = self._policy()
        self.assertFalse(check("cat .env").allowed)
        self.assertFalse(check("head -5 .env.local").allowed)
        self.assertFalse(check("cat ~/.ssh/id_rsa").allowed)
        self.assertFalse(check("cat .aws/credentials").allowed)
        self.assertFalse(check("grep -r x .env").allowed)
        self.assertFalse(check("powershell Get-Content .env").allowed)
        self.assertTrue(check("cat .env.example").allowed)
        self.assertTrue(check('grep ".env" src').allowed)


class OutputHelpersTest(unittest.TestCase):
    def test_repeat_compression(self):
        from tools.MainTool.runCommand import _compress_repeats
        text = "\n".join(["same line"] * 10)
        out, changed = _compress_repeats(text)
        self.assertTrue(changed)
        self.assertIn("repeated x9", out)
        short, changed2 = _compress_repeats("\n".join(["x"] * 3))
        self.assertFalse(changed2)
        self.assertEqual(short.count("x"), 3)

    def test_binary_detection(self):
        from tools.MainTool.runCommand import _looks_binary
        self.assertTrue(_looks_binary("\x00\xffabc"))
        self.assertFalse(_looks_binary("normal text\nwith lines\n"))

    def test_msys_noise_stripped(self):
        from tools.MainTool._shared import _clean_terminal_output
        noisy = (
            "ln: failed to create symbolic link '/etc/mtab': Permission denied\n"
            "'C:\\WINDOWS\\system32\\drivers\\etc\\hosts' -> '/etc/hosts'\n"
            "/usr/bin/cp: cannot create regular file '/etc/hosts': Permission denied\n"
            "rm: cannot remove '/etc/post-install/01-devices.post': Permission denied\n"
            "real output"
        )
        cleaned = _clean_terminal_output(noisy)
        self.assertIn("real output", cleaned)
        self.assertNotIn("/etc/mtab", cleaned)
        self.assertNotIn("post-install", cleaned)

    def test_msys_pathconv_knob(self):
        from unittest.mock import patch
        from tools.MainTool._shellenv import per_call_env
        with patch.dict(os.environ, {"AGENT_MSYS_NO_PATHCONV": "1"}):
            self.assertEqual(per_call_env("echo hi").get("MSYS_NO_PATHCONV"), "1")
        self.assertIsNone(per_call_env("echo hi").get("MSYS_NO_PATHCONV"))


class RunCommandTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        shutil.rmtree(FIXTURE, ignore_errors=True)
        os.makedirs(FIXTURE)
        cls.script = os.path.join(FIXTURE, "spawn_child.py")
        with open(cls.script, "w", encoding="utf-8") as f:
            f.write(
                "import subprocess, sys, time\n"
                "p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\n"
                "print('CHILD %d' % p.pid, flush=True)\n"
                "time.sleep(60)\n"
            )
        cls.spin = os.path.join(FIXTURE, "spin.py")
        with open(cls.spin, "w", encoding="utf-8") as f:
            f.write(
                "import sys, time\n"
                "end = time.time() + float(sys.argv[1] if len(sys.argv) > 1 else 5)\n"
                "x = 0\n"
                "while time.time() < end:\n"
                "    x += 1\n"
            )
        cls.url_server = os.path.join(FIXTURE, "url_server.py")
        with open(cls.url_server, "w", encoding="utf-8") as f:
            f.write(
                "import http.server, socketserver\n"
                "class H(http.server.SimpleHTTPRequestHandler):\n"
                "    def log_message(self, *a):\n"
                "        pass\n"
                "with socketserver.TCPServer(('127.0.0.1', 0), H) as s:\n"
                "    print('Local:   http://localhost:%d/' % s.server_address[1], flush=True)\n"
                "    s.serve_forever()\n"
            )

    @classmethod
    def tearDownClass(cls):
        try:
            _run(process_action="kill_all")
        except Exception:
            pass
        shutil.rmtree(FIXTURE, ignore_errors=True)

    def test_foreground_ok_and_exit_code(self):
        out = _run(command="echo runcmd-ok-1")
        self.assertIn("[OK] Exit 0", out)
        self.assertIn("runcmd-ok-1", out)
        out = _run(command="bash -c 'exit 3'")
        self.assertIn("[ERR] Exit 3", out)

    def test_expect(self):
        out = _run(command="echo expect-token-here", expect="expect-token-here")
        self.assertIn("[EXPECT matched", out)
        out = _run(command="echo nothing", expect="absent-token-xyz")
        self.assertIn("[EXPECT failed", out)

    def test_workdir_and_cache_key(self):
        out = _run(command="pwd", workdir="scratch")
        self.assertIn("/scratch", out.replace("\\", "/"))

    def test_dedup_same_workdir(self):
        cmd = f"echo dedup-{os.getpid()}"
        first = _run(command=cmd)
        second = _run(command=cmd)
        self.assertIn("[OK] Exit 0", first)
        self.assertIn("REPEAT RUN", second)
        # Same command text, different workdir must NOT be stubbed.
        third = _run(command=cmd, workdir="scratch")
        self.assertNotIn("REPEAT RUN", third)

    def test_binary_output(self):
        out = _run(command="printf '\\x00\\xffbinary\\x00'")
        self.assertIn("binary output", out)

    def test_idle_timeout(self):
        t0 = time.time()
        out = _run(command='python -c "import time; time.sleep(10)"',
                   timeout=15000, idle_timeout=1500)
        self.assertIn("no output for", out)
        self.assertIn("[TIMEOUT]", out)
        self.assertLess(time.time() - t0, 8.0)

    def test_timeout_kills_process_tree(self):
        script = self.script.replace("\\", "/")
        out = _run(command=f'python "{script}"', timeout=15000, idle_timeout=2000)
        self.assertIn("[TIMEOUT]", out)
        m = re.search(r"CHILD (\d+)", out)
        self.assertIsNotNone(m, out)
        child_pid = int(m.group(1))
        try:
            import psutil
        except Exception:
            self.skipTest("psutil unavailable")
        gone = False
        deadline = time.time() + 5
        while time.time() < deadline:
            if not psutil.pid_exists(child_pid):
                gone = True
                break
            time.sleep(0.2)
        self.assertTrue(gone, f"grandchild {child_pid} survived the tree kill")

    def test_background_lifecycle(self):
        out = _run(
            command='python -u -c "import time; print(\'bg-one\', flush=True); '
                    'time.sleep(0.4); print(\'bg-two\', flush=True)"',
            background=True,
        )
        self.assertIn("Started background process", out)
        name = _bg_name(out)
        self.assertTrue(name, out)
        waited = _run(command=name, process_action="wait", timeout=15000)
        self.assertIn("exited with code 0", waited)
        self.assertIn("bg-two", waited)
        logs = _run(command=name, process_action="logs")
        self.assertIn("bg-one", logs)
        listed = _run(process_action="list")
        self.assertIn(name, listed)

    def test_auto_background_server(self):
        out = _run(command="python -u -m http.server 0", timeout=1500, idle_timeout=1000)
        self.assertIn("moved to background", out)
        name = _bg_name(out)
        self.assertTrue(name, out)
        killed = _run(command=name, process_action="kill")
        self.assertIn("killed", killed.lower())

    def test_expected_negative_grep(self):
        target = os.path.join(self.script)  # any existing file
        out = _run(command=f'grep zzz-not-in-this-file "{target.replace(chr(92), "/")}"')
        self.assertIn("[OK] Exit 1 (no match", out)
        from tools._registry import execute_tool_detailed
        res = execute_tool_detailed(
            "run_command",
            {"command": f'grep zzz-not-in-this-file "{target.replace(chr(92), "/")}"'},
        )
        self.assertEqual(res.exit_code, 1)
        self.assertFalse(res.is_error)

    def test_cpu_activity_backgrounds(self):
        spin = self.spin.replace("\\", "/")
        out = _run(command=f'python "{spin}" 10', timeout=2500, idle_timeout=1000)
        self.assertIn("moved to background", out)
        self.assertIn("CPU active", out)
        _run(process_action="kill_all")

    def test_stdin_send(self):
        out = _run(command='python -u -c "print(input())"', background=True)
        self.assertIn("Started background process", out)
        name = _bg_name(out)
        self.assertTrue(name, out)
        sent = _run(command=name, process_action="send", input="hello-stdin")
        self.assertIn("[OK] Sent", sent)
        waited = _run(command=name, process_action="wait", timeout=8000)
        self.assertIn("exited with code 0", waited)
        self.assertIn("hello-stdin", waited)

    def test_background_ready_url(self):
        server = self.url_server.replace("\\", "/")
        out = _run(command=f'python -u "{server}"', background=True,
                   expect="Local:", ready_timeout=8000)
        self.assertIn("Started background process", out)
        self.assertIn("[READY] listening on http://127.0.0.1:", out)
        name = _bg_name(out)
        again = _run(command=name, process_action="ready", expect="Local:")
        self.assertIn("[READY]", again)
        self.assertIn("listening on http://127.0.0.1:", again)
        self.assertIn("expect=matched", again)
        _run(command=name, process_action="kill")

    def test_blocks_destructive(self):
        from pydantic_ai.exceptions import ToolFailed
        with self.assertRaises(ToolFailed):
            _run(command="git  push origin main")


if __name__ == "__main__":
    unittest.main(verbosity=2)
