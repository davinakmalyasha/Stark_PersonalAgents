"""Repair agent replies that contain earlier agent replies as a prefix.

Run without --apply for a dry run. The script backs up SQLite and every JSONL
file it changes, then rewrites only exact repeated prefixes. Ambiguous messages
are left alone and reported.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DB = ROOT / ".deep_agents" / "chats.db"
DEFAULT_SESSIONS = ROOT / ".deep_agents" / "sessions"
DEFAULT_CHATS = ROOT / ".deep_agents" / "chats"


def _agent_rows(con: sqlite3.Connection, chat_id: str):
    return con.execute(
        "SELECT rowid, id, content FROM messages "
        "WHERE chat_id=? AND role IN ('agent','assistant') AND length(content)>0 "
        "ORDER BY timestamp, rowid",
        (chat_id,),
    ).fetchall()


def _repaired_content(content: str, previous: list[str]) -> tuple[str, bool]:
    """Strip repeated earlier replies from the front, longest match first."""
    current = content
    changed = False
    candidates = sorted({text for text in previous if text}, key=len, reverse=True)
    while True:
        match = next((text for text in candidates if current.startswith(text)), None)
        if not match or match == current:
            break
        rest = current[len(match):]
        if not rest.strip():
            break
        current = rest.lstrip("\n")
        changed = True
    return current, changed


def _plan(db: Path) -> list[dict]:
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    try:
        chats = con.execute("SELECT id, title FROM chats ORDER BY updated_at DESC").fetchall()
        changes: list[dict] = []
        for chat in chats:
            previous: list[str] = []
            for row in _agent_rows(con, chat["id"]):
                content = str(row["content"] or "")
                repaired, changed = _repaired_content(content, previous)
                if changed:
                    changes.append({
                        "chat_id": chat["id"],
                        "title": chat["title"],
                        "message_id": row["id"],
                        "before": len(content),
                        "after": len(repaired),
                        "content": repaired,
                    })
                previous.append(repaired)
        return changes
    finally:
        con.close()


def _backup_db(db: Path, stamp: str) -> Path:
    target = db.with_name(f"{db.name}.bak-{stamp}")
    src = sqlite3.connect(str(db))
    dst = sqlite3.connect(str(target))
    try:
        src.backup(dst)
    finally:
        dst.close()
        src.close()
    return target


def _apply_db(db: Path, changes: list[dict]) -> None:
    con = sqlite3.connect(str(db))
    try:
        for change in changes:
            con.execute(
                "UPDATE messages SET content=? WHERE id=?",
                (change["content"], change["message_id"]),
            )
        con.commit()
    finally:
        con.close()


def _backup_file(path: Path, stamp: str) -> Path | None:
    if not path.exists():
        return None
    target = path.with_name(f"{path.name}.bak-{stamp}")
    shutil.copy2(path, target)
    return target


def _repair_jsonl(path: Path, replacements: dict[str, str]) -> int:
    if not path.exists():
        return 0
    changed = 0
    lines: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            lines.append(line)
            continue
        message_id = str(entry.get("id") or "")
        if message_id in replacements and entry.get("content") != replacements[message_id]:
            entry["content"] = replacements[message_id]
            lines.append(json.dumps(entry, ensure_ascii=False))
            changed += 1
        else:
            lines.append(line)
    if changed:
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return changed


def _repair_chat_json(path: Path, replacements: dict[str, str]) -> int:
    if not path.exists():
        return 0
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return 0
    changed = 0
    for message in data.get("messages") or []:
        if not isinstance(message, dict):
            continue
        message_id = str(message.get("id") or "")
        if message_id in replacements and message.get("content") != replacements[message_id]:
            message["content"] = replacements[message_id]
            changed += 1
    if changed:
        path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return changed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", type=Path, default=DEFAULT_DB)
    parser.add_argument("--sessions", type=Path, default=DEFAULT_SESSIONS)
    parser.add_argument("--chats", type=Path, default=DEFAULT_CHATS)
    parser.add_argument("--chat", action="append", default=[], help="limit to a chat id")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()

    changes = _plan(args.db)
    if args.chat:
        wanted = set(args.chat)
        changes = [change for change in changes if change["chat_id"] in wanted]

    by_chat: dict[str, list[dict]] = {}
    for change in changes:
        by_chat.setdefault(change["chat_id"], []).append(change)

    print(f"affected chats: {len(by_chat)}")
    for chat_id, rows in by_chat.items():
        print(f"  {chat_id} ({rows[0]['title']}): {len(rows)} message(s)")
        for row in rows:
            print(f"    {row['message_id']}: {row['before']} -> {row['after']} chars")

    if not changes:
        return 0
    if not args.apply:
        print("dry run: pass --apply to write repairs")
        return 0

    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    print(f"backup: {_backup_db(args.db, stamp)}")
    _apply_db(args.db, changes)

    replacements: dict[str, str] = {}
    for rows in by_chat.values():
        for row in rows:
            replacements[row["message_id"]] = row["content"]

    jsonl_changed = 0
    for chat_id in by_chat:
        for path in args.sessions.rglob(f"{chat_id}.jsonl"):
            _backup_file(path, stamp)
            jsonl_changed += _repair_jsonl(path, replacements)
    chat_json_changed = 0
    for chat_id in by_chat:
        for path in args.chats.rglob(f"{chat_id}.json"):
            _backup_file(path, stamp)
            chat_json_changed += _repair_chat_json(path, replacements)

    print(f"repaired: {len(changes)} message(s), {jsonl_changed} session line(s), {chat_json_changed} chat json message(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
