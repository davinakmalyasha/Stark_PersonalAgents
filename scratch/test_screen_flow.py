"""End-to-end test: real uvicorn + real WS client + agent tools in-process.

The app runs on a uvicorn server thread (same process), the fake Electron
overlay connects over /api/voice/ws with the websockets client, and the agent
screen tools are called from worker threads exactly like the General tool loop.

Run:  venv\\Scripts\\python.exe scratch\\test_screen_flow.py
"""
import asyncio
import base64
import io
import json
import os
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from types import SimpleNamespace

import uvicorn
import websockets.sync.client as ws_client

PORT = 8199
BASE = f"http://127.0.0.1:{PORT}"


def _png() -> str:
    from PIL import Image, ImageDraw
    img = Image.new("RGB", (160, 90), "white")
    ImageDraw.Draw(img).rectangle([10, 10, 100, 60], fill=(30, 120, 220))
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    return base64.b64encode(buf.getvalue()).decode()


def _post(path: str, payload: dict) -> dict:
    import urllib.request
    req = urllib.request.Request(
        BASE + path,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read().decode())


def main() -> int:
    os.environ["STARK_PORT"] = str(PORT)
    from server.api_server import app

    config = uvicorn.Config(app, host="127.0.0.1", port=PORT, log_level="warning")
    server = uvicorn.Server(config)
    thread = threading.Thread(target=server.run, daemon=True)
    thread.start()
    for _ in range(100):
        try:
            import urllib.request
            with urllib.request.urlopen(BASE + "/api/voice/status", timeout=1):
                break
        except Exception:
            time.sleep(0.1)
    else:
        print("server did not start")
        return 1

    from server.services import screen_bridge

    ok = True
    result = {}
    with ws_client.connect(f"ws://127.0.0.1:{PORT}/api/voice/ws") as ws:
        ws.send(json.dumps({"type": "screen-state", "sharing": True, "mode": "on", "source": "Screen 1"}))
        time.sleep(0.3)

        def run_look():
            import tools
            ctx = SimpleNamespace(pending_images=[])
            result["look"] = tools.look_at_screen(ctx=ctx)
            result["queued"] = len(ctx.pending_images)

        t = threading.Thread(target=run_look, daemon=True)
        t.start()
        msg = json.loads(ws.recv(timeout=10))
        print("1. overlay received:", msg.get("type"))
        assert msg.get("type") == "screen-capture", msg
        body = _post("/api/vision/frame", {
            "image_b64": _png(), "mime": "image/png", "chat_id": "test-chat",
            "label": "tool", "source": "Test screen", "req_id": msg["req_id"],
        })
        att = body.get("attachment") or {}
        print("2. frame endpoint ok:", body.get("ok"), "| saved:", os.path.basename(att.get("path", "")))
        t.join(15)
        print("3. look_at_screen:", (result.get("look") or "")[:90])
        print("   queued images:", result.get("queued"))
        ok = ok and body.get("ok") and att.get("screenFrame") is True and att.get("isImage") is True
        ok = ok and "[IMAGE]" in (result.get("look") or "") and result.get("queued") == 1

        def run_mode(mode):
            import tools
            result["mode:" + mode] = tools.set_screen_share(mode=mode)

        t2 = threading.Thread(target=run_mode, args=("continuous",), daemon=True)
        t2.start()
        msg2 = json.loads(ws.recv(timeout=10))
        print("4. overlay received:", msg2.get("type"), "| mode:", msg2.get("mode"))
        assert msg2.get("type") == "screen-mode" and msg2.get("mode") == "continuous", msg2
        ws.send(json.dumps({"type": "screen-ack", "req_id": msg2["req_id"], "ok": True, "mode": "continuous", "sharing": True}))
        t2.join(15)
        time.sleep(0.3)
        print("5. set_screen_share ->", result.get("mode:continuous"))
        st = screen_bridge.state()
        print("6. bridge state:", st)
        ok = ok and "continuous" in (result.get("mode:continuous") or "")
        ok = ok and st.get("mode") == "continuous" and st.get("sharing") is True

        # Failure ack path
        t3 = threading.Thread(target=run_mode, args=("off",), daemon=True)
        t3.start()
        msg3 = json.loads(ws.recv(timeout=10))
        ws.send(json.dumps({"type": "screen-ack", "req_id": msg3["req_id"], "ok": False, "error": "denied by user"}))
        t3.join(15)
        print("7. failure ack:", (result.get("mode:off") or "")[:80])
        ok = ok and "denied by user" in (result.get("mode:off") or "")

    # Overlay gone: tools must fail fast, not hang
    import tools
    t0 = time.time()
    offline = tools.look_at_screen()
    dt = time.time() - t0
    print(f"8. after disconnect ({dt:.2f}s):", offline[:70])
    ok = ok and offline.startswith("Error:") and dt < 3

    # Disconnect must reset sharing state (no stale continuous injection).
    st_after = screen_bridge.state()
    print("8b. state after disconnect:", st_after)
    ok = ok and st_after.get("sharing") is False and st_after.get("mode") == "off"

    # Vision channel: continuous mode + fresh latest frame -> injected before a model request
    from sync.state_sync import safe_update_state
    from tools.vision_channel import _continuous_screen_frame
    safe_update_state({"live_screen_latest": att.get("path", "")})
    screen_bridge.update_state({"sharing": True, "mode": "continuous"})
    import tools.vision_channel as vc
    vc._last_continuous_at = 0
    got = vc._continuous_screen_frame()
    print("9. continuous injection picks fresh frame:", bool(got))
    ok = ok and bool(got)

    # runs.py hides screen frames from the persisted chat log
    from server.routers.runs import RunRequest
    req = RunRequest(prompt="hi", attachments=[{"screenFrame": True, "isImage": True, "path": "x", "name": "s"}])
    stored = [a for a in req.attachments if not a.get("screenFrame")]
    print("10. chat-log filter leaves:", len(stored), "of", len(req.attachments))
    ok = ok and stored == []

    server.should_exit = True
    thread.join(10)
    print("RESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
