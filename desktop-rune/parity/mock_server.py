#!/usr/bin/env python3
"""Replay recorded API fixtures for deterministic React/Rune parity captures.

    python parity/mock_server.py [--port 8125]

Run a second Vite with `STARK_PORT=8125` (e.g. on :5200) and the native app
with `STARK_API_PORT=8125`. `/api/events` stays open with heartbeats so SSE
clients behave as against the real server.
"""
import argparse
import json
import pathlib
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = pathlib.Path(__file__).resolve().parent
FIX = HERE / "fixtures"
SCENARIOS = HERE / "scenarios"
SCENARIO = ""
ACTIVE_CHAT = ""


def name_for(path: str) -> str:
    clean = path.split("?", 1)[0]
    return clean.strip("/").replace("/", "_") + ".json"


def fixture_path(name: str) -> pathlib.Path | None:
    if SCENARIO:
        scenario = SCENARIOS / SCENARIO / name
        if scenario.exists():
            return scenario
    base = FIX / name
    return base if base.exists() else None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args) -> None:  # quiet
        pass

    def _send(self, code: int, body: bytes, ctype: str) -> None:
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body)

    def _json(self, code: int, data) -> None:
        self._send(code, json.dumps(data).encode("utf-8"), "application/json")

    def _files(self):
        names = set()
        if FIX.is_dir():
            names.update(p.name[:-5] for p in FIX.glob("*.json"))
        if SCENARIO and (SCENARIOS / SCENARIO).is_dir():
            names.update(p.name[:-5] for p in (SCENARIOS / SCENARIO).glob("*.json"))
        return sorted(names)

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, PUT, POST, DELETE, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self) -> None:  # noqa: N802
        path = self.path.split("?", 1)[0]
        if path == "/api/events":
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "keep-alive")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            try:
                while True:
                    self.wfile.write(b": ping\n\n")
                    self.wfile.flush()
                    time.sleep(15)
            except (BrokenPipeError, ConnectionResetError):
                return
        name = name_for(path)
        file = fixture_path(name)
        if file is None:
            self._json(404, {"error": "no fixture for " + path, "have": self._files()})
            return
        body = file.read_bytes()
        if path == "/api/ui/state" and ACTIVE_CHAT:
            data = json.loads(body)
            key = ACTIVE_CHAT if ":" in ACTIVE_CHAT else f"_general:{ACTIVE_CHAT}"
            data["active_tab"] = key
            data["open_tabs"] = [key]
            body = json.dumps(data).encode("utf-8")
        self._send(200, body, "application/json")

    def do_PUT(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        self._json(200, {"ok": True})

    def do_POST(self) -> None:  # noqa: N802
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        self._json(200, {"ok": True})

    def do_DELETE(self) -> None:  # noqa: N802
        # The real server deletes the chat; a failure would make both sides
        # refetch the static list and resurrect the row.
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        self._json(200, {"ok": True})


def main() -> None:
    global SCENARIO, ACTIVE_CHAT
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8125)
    ap.add_argument(
        "--scenario",
        default="",
        help="overlay fixture directory under parity/scenarios (e.g. md)",
    )
    ap.add_argument(
        "--active-chat",
        default="",
        help="rewrite ui/state to open only this chat (chat id or ws:chat)",
    )
    args = ap.parse_args()
    SCENARIO = args.scenario
    ACTIVE_CHAT = args.active_chat
    if SCENARIO and not (SCENARIOS / SCENARIO).is_dir():
        raise SystemExit(f"no scenario at {SCENARIOS / SCENARIO}")
    if not FIX.is_dir():
        raise SystemExit(f"no fixtures at {FIX}; run parity/record_fixtures.py first")
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    extra = f", scenario {SCENARIO}" if SCENARIO else ""
    active = f", active {ACTIVE_CHAT}" if ACTIVE_CHAT else ""
    print(
        f"parity fixtures on http://127.0.0.1:{args.port} "
        f"({len(list(FIX.glob('*.json')))} files{extra}{active})"
    )
    server.serve_forever()


if __name__ == "__main__":
    main()
