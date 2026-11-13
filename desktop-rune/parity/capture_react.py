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
import datetime
import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.request

import websockets

HIDE_CSS = (
    "*::-webkit-scrollbar{display:none!important;width:0!important;height:0!important}"
    " *{scrollbar-width:none!important}"
    # Rune has no scroll-to-latest pill; hide React's so mid-transcript
    # markdown captures compare the content, not a control Rune does not own.
    " .scroll-bottom-pill{display:none!important}"
)

# Applied before any app script runs, so relative labels (`20d`, `Today`) and
# anything else reading the wall clock see the frozen rig instant.
DATE_PATCH = """
(() => {
  const fixed = __FIXED__;
  const RealDate = Date;
  function FakeDate(...args) {
    if (!new.target) return new RealDate(fixed).toString();
    return args.length === 0 ? new RealDate(fixed) : new RealDate(...args);
  }
  FakeDate.prototype = RealDate.prototype;
  FakeDate.now = () => fixed;
  FakeDate.parse = RealDate.parse;
  FakeDate.UTC = RealDate.UTC;
  Object.setPrototypeOf(FakeDate, RealDate);
  window.Date = FakeDate;
})();
"""


def parity_millis(raw: str) -> int:
    raw = raw.strip()
    if not raw:
        return 0
    if raw.isdigit():
        value = int(raw)
        return value if len(raw) >= 13 else value * 1000
    try:
        text = raw[:-1] + "+00:00" if raw[-1:] in ("Z", "z") else raw
        stamp = datetime.datetime.fromisoformat(text)
        if stamp.tzinfo is None:
            stamp = stamp.replace(tzinfo=datetime.timezone.utc)
        return int(stamp.timestamp() * 1000)
    except ValueError as exc:
        raise SystemExit(f"bad --parity-now {raw!r}: {exc}") from exc


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
            # Hide scrollbars from the very first layout: the app's initial
            # fitView then sees the same stage width as the native window.
            "--hide-scrollbars",
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
            if args.parity_now:
                fixed = parity_millis(args.parity_now)
                await cdp.call(
                    "Page.addScriptToEvaluateOnNewDocument",
                    {"source": DATE_PATCH.replace("__FIXED__", str(fixed))},
                )
                await cdp.call("Page.reload", {"ignoreCache": True})
                deadline = time.time() + 20
                while time.time() < deadline:
                    state = await cdp.call(
                        "Runtime.evaluate",
                        {"expression": "document.readyState", "returnByValue": True},
                    )
                    if state.get("result", {}).get("value") == "complete":
                        break
                    time.sleep(0.1)
                time.sleep(1.0)
            else:
                time.sleep(1.0)
            time.sleep(args.settle)
            scrolled = await cdp.call(
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
                        if (sc) {{
                            const top = {args.scroll_top};
                            sc.scrollTop = top >= 0
                                ? top
                                : Math.max(0, sc.scrollHeight - sc.clientHeight - {max(0, args.scroll_by)});
                        }}
                        return {{ inner: [window.innerWidth, window.innerHeight],
                                  dpr: devicePixelRatio,
                                  scrollTop: sc ? sc.scrollTop : null,
                                  scrollHeight: sc ? sc.scrollHeight : null }};
                    }})()""",
                    "returnByValue": True,
                },
            )
            if args.debug:
                print("scroll:", json.dumps(scrolled.get("result", {}).get("value")))
                probe = await cdp.call(
                    "Runtime.evaluate",
                    {
                        "expression": """(() => {
                            const rows = [...document.querySelectorAll('.overflow-y-auto')].map((el) => {
                                const r = el.getBoundingClientRect();
                                return { cls: el.className, top: r.top, height: r.height,
                                         scrollTop: el.scrollTop, scrollHeight: el.scrollHeight,
                                         clientHeight: el.clientHeight };
                            });
                            const body = document.body.innerText;
                            const table = document.querySelector('.markdown-body table');
                            const wrap = table ? table.parentElement : null;
                            const rect = (el) => { const r = el.getBoundingClientRect();
                                return { x: r.x, y: r.y, w: r.width, h: r.height }; };
                            let intrinsic = [];
                            if (table) {
                                const host = document.createElement('div');
                                host.style.cssText = 'position:fixed;left:-10000px;top:0;width:max-content;';
                                const copy = table.cloneNode(true);
                                copy.style.width = 'max-content';
                                host.appendChild(copy);
                                document.body.appendChild(host);
                                intrinsic = [...copy.querySelectorAll('th')].map((el) => {
                                    const r = el.getBoundingClientRect();
                                    return { text: el.textContent, w: r.width };
                                });
                                host.remove();
                            }
                            const cells = table ? [...table.querySelectorAll('th,td')].slice(0, 32).map((el) => {
                                const r = el.getBoundingClientRect();
                                const cs = getComputedStyle(el);
                                const range = document.createRange(); range.selectNodeContents(el);
                                const tw = range.getBoundingClientRect().width;
                                return { tag: el.tagName, x: r.x, w: r.width, h: r.height, tw,
                                         text: (el.textContent || '').slice(0, 12),
                                         fs: cs.fontSize, lh: cs.lineHeight, ls: cs.letterSpacing,
                                         pad: cs.padding, weight: cs.fontWeight,
                                         bb: cs.borderBottomWidth, bt: cs.borderTopWidth,
                                         valign: cs.verticalAlign };
                            }) : [];
                            const trs = table ? [...table.querySelectorAll('tr')].slice(0, 12).map(rect) : [];
                            const mdBlocks = [...document.querySelectorAll('.markdown-body > *')].map((el) => {
                                const r = el.getBoundingClientRect();
                                const cs = getComputedStyle(el);
                                return { tag: el.tagName, rect: { x: r.x, y: r.y, w: r.width, h: r.height },
                                         mt: cs.marginTop, mb: cs.marginBottom, lh: cs.lineHeight };
                            });
                            const chain = (text) => {
                                const body = [...document.querySelectorAll('.markdown-body')]
                                    .find((el) => (el.textContent || '').startsWith(text));
                                if (!body) return null;
                                const out = [];
                                let el = body;
                                for (let i = 0; i < 7 && el; i += 1, el = el.parentElement) {
                                    out.push({ cls: el.className, rect: rect(el) });
                                }
                                return out;
                            };
                            const chains = { user: chain('Render the parity table.'), agent: chain('The frozen parity numbers:') };
                            const stream = [...document.querySelectorAll('div')]
                                .find((el) => typeof el.className === 'string' && el.className.includes('max-w-[1100px]'));
                            const streamKids = stream ? [...stream.children].map((el) => ({
                                cls: el.className, rect: rect(el), text: (el.textContent || '').slice(0, 40) })) : [];
                            const inline = table ? table.querySelector('td code') : null;
                            const inlineBox = inline ? (() => {
                                const r = inline.getBoundingClientRect();
                                const cs = getComputedStyle(inline);
                                const range = document.createRange(); range.selectNodeContents(inline);
                                const text = range.getBoundingClientRect();
                                return { rect: rect(inline), text: { x: text.x, y: text.y, w: text.width, h: text.height }, fs: cs.fontSize,
                                         family: cs.fontFamily, pad: cs.padding, border: cs.borderWidth,
                                         lh: cs.lineHeight };
                            })() : null;
                            const codeOuter = [...document.querySelectorAll('.markdown-body > div')]
                                .find((el) => el.querySelector('pre'));
                            const codeBox = codeOuter ? (() => {
                                const box = (el) => {
                                    const cs = getComputedStyle(el);
                                    return { rect: rect(el), pad: cs.padding, border: cs.borderWidth,
                                             bt: cs.borderTopWidth, bb: cs.borderBottomWidth,
                                             fs: cs.fontSize, lh: cs.lineHeight, fam: cs.fontFamily };
                                };
                                const pre = codeOuter.querySelector('pre');
                                const tok = pre ? [...pre.querySelectorAll('code span')].map((el) => ({
                                    cls: el.className, text: (el.textContent || '').slice(0, 24),
                                    color: getComputedStyle(el).color })) : [];
                                return { outer: box(codeOuter), head: box(codeOuter.children[0]),
                                         body: box(codeOuter.children[1]), pre: box(pre),
                                         code: pre ? box(pre.querySelector('code') || pre) : null, tok };
                            })() : null;
                            const anchors = [...document.querySelectorAll('[data-message-anchor]')];
                            const agentAnchor = anchors[anchors.length - 1] || null;
                            const agentBox = agentAnchor ? {
                                anchor: rect(agentAnchor),
                                kids: [...agentAnchor.firstElementChild.children].map((el) => ({
                                    cls: el.className, rect: rect(el) })),
                                footers: [...agentAnchor.querySelectorAll('.agent-message-footer')].map((el) => ({
                                    cls: el.className, rect: rect(el), order: getComputedStyle(el).order,
                                    mt: getComputedStyle(el).marginTop })),
                                response: [...agentAnchor.querySelectorAll('.agent-message-response')].map((el) => ({
                                    rect: rect(el), cls: el.className,
                                    pb: getComputedStyle(el).paddingBottom, mb: getComputedStyle(el).marginBottom })),
                            } : null;
                            const diffWrap = [...document.querySelectorAll('.markdown-body > div')]
                                .find((el) => el.querySelector('table'));
                            const diffOuter = diffWrap ? [...diffWrap.querySelectorAll('div')]
                                .find((el) => typeof el.className === 'string' && el.className.includes('rounded-lg') && el.querySelector('table')) : null;
                            const diffBox = diffOuter ? (() => {
                                const dbox = (el) => {
                                    if (!el) return null;
                                    const cs = getComputedStyle(el);
                                    return { rect: rect(el), pad: cs.padding, border: cs.borderWidth,
                                             bt: cs.borderTopWidth, bb: cs.borderBottomWidth,
                                             fs: cs.fontSize, lh: cs.lineHeight, fam: cs.fontFamily };
                                };
                                const scroll = [...diffOuter.querySelectorAll('div')]
                                    .find((el) => typeof el.className === 'string' && el.className.includes('max-h-[420px]'));
                                const dtable = scroll ? scroll.querySelector('table') : null;
                                const tr = dtable && dtable.tBodies[0] ? dtable.tBodies[0].rows[0] : null;
                                const cell = tr ? tr.cells[0] : null;
                                const all = scroll ? [...scroll.children].map((el) => ({ tag: el.tagName, cls: el.className, rect: rect(el) })) : [];
                                const head = diffOuter.children[0];
                                return { wrap: dbox(diffWrap), outer: dbox(diffOuter), head: dbox(head),
                                         span: dbox(head ? head.querySelector('span') : null),
                                         stats: dbox(head ? head.querySelector('span.flex') : null),
                                         scroll: scroll ? { rect: rect(scroll), cs: (() => { const c = getComputedStyle(scroll); return { maxH: c.maxHeight, overflow: c.overflow, pad: c.padding }; })() } : null,
                                         table: dtable ? rect(dtable) : null, tr: tr ? rect(tr) : null,
                                         cell: cell ? { rect: rect(cell), cs: (() => { const c = getComputedStyle(cell); return { pad: c.padding, w: c.width, border: c.borderRightWidth, lh: c.lineHeight, fs: c.fontSize }; })() } : null,
                                         all };
                            })() : null;
                            return { rows, hasFooter: /06:22/.test(body), intrinsic, cells,
                                     wrap: wrap ? { rect: rect(wrap), cs: (() => { const c = getComputedStyle(wrap); return { border: c.borderWidth, pad: c.padding, overflow: c.overflowX }; })() } : null,
                                     table: table ? { rect: rect(table), cs: (() => { const c = getComputedStyle(table); return { borderCollapse: c.borderCollapse, width: c.width, pad: c.padding, layout: c.tableLayout }; })() } : null,
                                     thead: table && table.tHead ? rect(table.tHead) : null, trs, mdBlocks, chains, streamKids, inlineBox, codeBox, diffBox, agentBox };
                        })()""",
                        "returnByValue": True,
                    },
                )
                print("probe:", json.dumps(probe.get("result", {}).get("value"), indent=1))
                if probe.get("result", {}).get("value") is None:
                    print("probe error:", json.dumps(probe.get("result", {}).get("exceptionDetails", probe), indent=1)[:3000])
            # An explicit scroll is a frozen mid-transcript pose: the app's own
            # auto-scroll can fire a beat later, so shoot right away.
            time.sleep(0.05 if args.scroll_top >= 0 else 0.6)
            if args.debug:
                post = await cdp.call(
                    "Runtime.evaluate",
                    {
                        "expression": """(() => {
                            const sc = [...document.querySelectorAll('.overflow-y-auto')]
                                .find(s => s.className.includes('pl-7'));
                            const table = document.querySelector('.markdown-body table');
                            const r = table ? table.getBoundingClientRect() : null;
                            return { scrollTop: sc ? sc.scrollTop : null,
                                     tableY: r ? r.y : null, tableH: r ? r.height : null };
                        })()""",
                        "returnByValue": True,
                    },
                )
                print("post:", json.dumps(post.get("result", {}).get("value")))
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
    ap.add_argument(
        "--scroll-by",
        type=int,
        default=0,
        help="scroll the transcript up this many CSS px from its end",
    )
    ap.add_argument(
        "--scroll-top",
        type=int,
        default=-1,
        help="set the transcript scrollTop to this absolute CSS px (overrides --scroll-by)",
    )
    ap.add_argument("--chrome", default="C:/Program Files/Google/Chrome/Application/chrome.exe")
    ap.add_argument("--profile", default="C:/Users/MY LENOVO/AppData/Local/Temp/opencode/chrome-parity")
    ap.add_argument("--port", type=int, default=9333)
    ap.add_argument("--debug", action="store_true", help="print scroller diagnostics")
    ap.add_argument(
        "--parity-now",
        default=os.environ.get("STARK_PARITY_NOW", "2026-11-12T12:00:00Z"),
        help="freeze Date to this instant (Unix s/ms or RFC 3339); empty = live clock",
    )
    args = ap.parse_args()
    asyncio.run(capture(args))
    return 0


if __name__ == "__main__":
    sys.exit(main())
