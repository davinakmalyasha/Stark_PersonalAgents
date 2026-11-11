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


def name_for(path: str) -> str:
    clean = path.split("?", 1)[0]
    return clean.strip("/").replace("/", "_") + ".json"


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
        return sorted(p.name[:-5] for p in FIX.glob("*.json")) if FIX.is_dir() else []

    def do_OPTIONS(self) -> None:  # noqa: N802
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, PUT, POST, OPTIONS")
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
        file = FIX / name
        if not file.exists():
            self._json(404, {"error": "no fixture for " + path, "have": self._files()})
            return
        self._send(200, file.read_bytes(), "application/json")

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


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8125)
    args = ap.parse_args()
    if not FIX.is_dir():
        raise SystemExit(f"no fixtures at {FIX}; run parity/record_fixtures.py first")
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    print(f"parity fixtures on http://127.0.0.1:{args.port} ({len(list(FIX.glob('*.json')))} files)")
    server.serve_forever()


if __name__ == "__main__":
    main()
