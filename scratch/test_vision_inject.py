"""Focused test: paid-path vision injection + continuous-frame throttle.

Verifies the ProcessHistory capability built by
tools.vision_channel.get_vision_history_capability() injects tool-queued
images as ONE synthetic user message before a model request, and that
continuous-mode frames obey the 15s throttle / freshness gates.

Run:  venv\\Scripts\\python.exe scratch\\test_vision_inject.py
"""
import asyncio
import os
import sys
import time
from types import SimpleNamespace

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from PIL import Image


def _png(path: str) -> str:
    Image.new("RGB", (60, 40), "white").save(path, format="PNG")
    return path


def main() -> int:
    ok = True

    # ── 1. Queue → capability processor → history message ──
    from tools.vision_channel import get_vision_history_capability, queue_image_file

    cap = get_vision_history_capability()
    print("1. capability built:", cap is not None)
    ok = ok and cap is not None

    tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp_vision.png")
    _png(tmp)
    deps = SimpleNamespace(pending_images=[])
    queued = queue_image_file(tmp, note="unit-test frame", ctx=deps)
    print("2. queued:", queued, "| pending:", len(deps.pending_images))
    ok = ok and queued and len(deps.pending_images) == 1

    ctx = SimpleNamespace(deps=deps)
    history = []
    out = asyncio.run(cap.processor(ctx, history))
    from pydantic_ai.messages import ModelRequest, UserPromptPart
    from pydantic_ai.messages import BinaryContent

    has_image_part = False
    n_requests = 0
    for msg in out:
        if isinstance(msg, ModelRequest):
            n_requests += 1
            for part in msg.parts:
                if isinstance(part, UserPromptPart):
                    content = part.content
                    if isinstance(content, list):
                        for c in content:
                            if isinstance(c, BinaryContent):
                                has_image_part = True
    print("3. injected messages:", n_requests, "| image part:", has_image_part, "| drained:", len(deps.pending_images) == 0)
    ok = ok and n_requests == 1 and has_image_part and not deps.pending_images

    out2 = asyncio.run(cap.processor(ctx, []))
    print("4. second request with empty queue injects nothing:", out2 == [])
    ok = ok and out2 == []

    try:
        os.remove(tmp)
    except OSError:
        pass

    # ── 2. Continuous-frame gates ──
    from tools import vision_channel as vc
    from server.services import screen_bridge
    from sync.state_sync import safe_update_state

    tmp2 = os.path.join(os.path.dirname(os.path.abspath(__file__)), "_tmp_vision2.png")
    _png(tmp2)
    safe_update_state({"live_screen_latest": tmp2})

    vc._last_continuous_at = 0
    screen_bridge.update_state({"sharing": True, "mode": "on"})
    off_mode = vc._continuous_screen_frame()
    screen_bridge.update_state({"sharing": True, "mode": "continuous"})
    vc._last_continuous_at = 0
    first = vc._continuous_screen_frame()
    second = vc._continuous_screen_frame()
    os.utime(tmp2, (time.time() - 120, time.time() - 120))
    vc._last_continuous_at = 0
    stale = vc._continuous_screen_frame()
    print("5. gates: mode=on ->", repr(off_mode), "| first ->", bool(first), "| throttled ->", repr(second), "| stale ->", repr(stale))
    ok = ok and off_mode == "" and bool(first) and second == "" and stale == ""

    # Per-run throttle: a NEW run injects immediately even right after another
    # run consumed the frame; within one run the 15s gap still applies.
    _png(tmp2)  # refresh freshness after the stale check
    safe_update_state({"live_screen_latest": tmp2})
    screen_bridge.update_state({"sharing": True, "mode": "continuous"})
    deps_a = SimpleNamespace(pending_images=[])
    deps_b = SimpleNamespace(pending_images=[])
    run_a1 = vc._continuous_screen_frame(deps_a)
    run_a2 = vc._continuous_screen_frame(deps_a)
    run_b1 = vc._continuous_screen_frame(deps_b)
    vc._last_continuous_at = time.time()
    global_throttled = vc._continuous_screen_frame()
    print("6. per-run: a1 ->", bool(run_a1), "| a2 throttled ->", repr(run_a2), "| new run b1 ->", bool(run_b1), "| global throttled ->", repr(global_throttled))
    ok = ok and bool(run_a1) and run_a2 == "" and bool(run_b1) and global_throttled == ""

    try:
        os.remove(tmp2)
    except OSError:
        pass
    safe_update_state({"live_screen_latest": ""})
    screen_bridge.update_state({"sharing": False, "mode": "off"})
    vc._last_continuous_at = 0

    print("RESULT:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
