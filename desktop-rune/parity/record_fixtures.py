#!/usr/bin/env python3
"""Record the live Stark API into `parity/fixtures/` for deterministic captures.

    python parity/record_fixtures.py [--base http://127.0.0.1:8124]

One snapshot for both frontends: the React app points its Vite proxy at the
replay server, the native app reads `STARK_API_PORT`. Re-record whenever the
canonical scene should change; captures are only comparable within one
snapshot.
"""
import argparse
import json
import pathlib
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "fixtures"


def get(base: str, path: str):
    with urllib.request.urlopen(base + path, timeout=20) as res:
        return json.loads(res.read().decode("utf-8"))


def name_for(path: str) -> str:
    clean = path.split("?", 1)[0]
    return clean.strip("/").replace("/", "_") + ".json"


def save(path: str, data) -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / name_for(path)).write_text(
        json.dumps(data, indent=2, sort_keys=True), encoding="utf-8"
    )
    print("recorded", name_for(path))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:8124")
    args = ap.parse_args()
    base = args.base.rstrip("/")

    save("/api/ui/state", get(base, "/api/ui/state"))
    save("/api/runs/active", get(base, "/api/runs/active"))
    for path in ("/api/provider", "/api/models", "/api/ask/pending"):
        try:
            save(path, get(base, path))
        except Exception as err:  # a missing optional endpoint must not fail the snapshot
            save(path, {})
            print("  (empty:", err, ")")

    workspaces = get(base, "/api/workspaces")
    save("/api/workspaces", workspaces)
    ui = get(base, "/api/ui/state")
    for ws in workspaces.get("workspaces", workspaces.get("data", [])) or []:
        ws_id = ws.get("id")
        if not ws_id:
            continue
        chats_path = f"/api/workspaces/{ws_id}/chats"
        save(chats_path, get(base, chats_path))
        for tab in ui.get("open_tabs", []):
            if not tab.startswith(f"{ws_id}:"):
                continue
            chat_id = tab.split(":", 1)[1]
            chat_path = f"/api/workspaces/{ws_id}/chats/{chat_id}"
            try:
                save(chat_path, get(base, chat_path))
            except Exception as err:
                print("  (skip", chat_path, err, ")")


if __name__ == "__main__":
    main()
