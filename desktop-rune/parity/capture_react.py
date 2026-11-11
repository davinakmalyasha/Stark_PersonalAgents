#!/usr/bin/env python3
"""Deterministic React capture over CDP: 1x scale, no scrollbars, scrolled to
the transcript bottom after fonts and data settle.

    python parity/capture_react.py OUT.png [--url URL] [--size 800x573]
                              [--settle 3.0] [--chrome PATH] [--port 9333]

Headless Chrome's `--screenshot` can fire before the app's scroll-to-bottom
effect, which leaves the transcript a dozen points high; this drives the page
over CDP instead, hides scrollbars the way the native shell has none, and only
then captures.
"""
import argparse
import asyncio
import base64
import json
import pathlib
import subprocess
import sys
import time
import urllib.request

import websockets

HIDE_CSS = (
    "*::-webkit-scrollbar{display:none!important;width:0!important;height:0!important}"
    " *{scrollbar-width:none!important}"
)


def http_json(url: str):
    with urllib.request.urlopen(url, timeout=5) as res:
        return json.loads(res.read().decode("utf-8"))


def wait_for_debugger(port: int, timeout: float = 20.0) -> dict:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            return http_json(f"http://127.0.0.1:{port}/json/version")
        except Exception:
            time.sleep(0.2)
    raise TimeoutError("Chrome remote debugger never came up")


def page_target(port: int) -> dict:
    deadline = time.time() + 20
    while time.time() < deadline:
        for target in http_json(f"http://127.0.0.1:{port}/json"):
            if target.get("type") == "page" and target.get("webSocketDebuggerUrl"):
                return target
        time.sleep(0.2)
    raise TimeoutError("no page target")


class Cdp:
    def __init__(self, ws):
        self.ws = ws
        self.next_id = 1
        self.events = []

    async def call(self, method: str, params: dict | None = None):
        cid = self.next_id
        self.next_id += 1
        await self.ws.send(json.dumps({"id": cid, "method": method, "params": params or {}}))
        while True:
            msg = json.loads(await self.ws.recv())
            if msg.get("id") == cid:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})
            self.events.append(msg)


async def capture(args) -> None:
    chrome = pathlib.Path(args.chrome)
    if not chrome.exists():
        raise SystemExit(f"chrome not found: {chrome}")
    profile = pathlib.Path(args.profile)
    profile.mkdir(parents=True, exist_ok=True)
    w, h = (int(v) for v in args.size.lower().split("x"))
    proc = subprocess.Popen(
        [
            str(chrome),
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            f"--user-data-dir={profile}",
            f"--remote-debugging-port={args.port}",
            f"--window-size={w},{h}",
            "--force-device-scale-factor=1",
            args.url,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for_debugger(args.port)
        target = page_target(args.port)
        async with websockets.connect(target["webSocketDebuggerUrl"], max_size=64 * 1024 * 1024) as ws:
            cdp = Cdp(ws)
            await cdp.call("Page.enable")
            await cdp.call("Runtime.enable")
            await cdp.call(
                "Emulation.setDeviceMetricsOverride",
                {
                    "width": w,
                    "height": h,
                    "deviceScaleFactor": 1,
                    "mobile": False,
                    "screenWidth": w,
                    "screenHeight": h,
                },
            )
            time.sleep(1.0)
            time.sleep(args.settle)
            await cdp.call(
                "Runtime.evaluate",
                {
                    "expression": f"""(() => {{
                        const st = document.createElement('style');
                        st.textContent = {json.dumps(HIDE_CSS)};
                        document.head.appendChild(st);
                        // The web build has no updater; drop its error toast.
                        for (const el of document.querySelectorAll('[role="status"]')) {{
                            if (/update/i.test(el.textContent || '')) el.remove();
                        }}
                        const sc = [...document.querySelectorAll('.overflow-y-auto')]
                            .find(s => s.className.includes('pl-7'));
                        if (sc) sc.scrollTop = sc.scrollHeight;
                        return {{ inner: [window.innerWidth, window.innerHeight],
                                  dpr: devicePixelRatio,
                                  scrollTop: sc ? sc.scrollTop : null,
                                  scrollHeight: sc ? sc.scrollHeight : null }};
                    }})()""",
                    "returnByValue": True,
                },
            )
            time.sleep(0.6)
            shot = await cdp.call(
                "Page.captureScreenshot",
                {"format": "png", "captureBeyondViewport": False},
            )
            data = base64.b64decode(shot["data"])
            out = pathlib.Path(args.out)
            out.write_bytes(data)
            print(f"{len(data)} bytes written to {out}")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--url", default="http://localhost:5200/")
    ap.add_argument("--size", default="800x573")
    ap.add_argument("--settle", type=float, default=3.0)
    ap.add_argument("--chrome", default="C:/Program Files/Google/Chrome/Application/chrome.exe")
    ap.add_argument("--profile", default="C:/Users/MY LENOVO/AppData/Local/Temp/opencode/chrome-parity")
    ap.add_argument("--port", type=int, default=9333)
    args = ap.parse_args()
    asyncio.run(capture(args))
    return 0


if __name__ == "__main__":
    sys.exit(main())
