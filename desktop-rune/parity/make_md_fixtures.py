#!/usr/bin/env python3
"""Build the synthetic markdown showcase fixtures under `scenarios/md/`.

    python parity/make_md_fixtures.py

These are committed (the recorded `fixtures/` stay private): a code block, a
measured table, and an 820-line unified diff. `mock_server.py --scenario md`
serves them; `--active-chat` picks which one boots.
"""
import json
import pathlib

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE / "scenarios" / "md"

CODE = """Here is the replay loop used by the fixture server:

```rust
use std::collections::HashMap;

fn replay(events: &[Event]) -> HashMap<String, usize> {
    let mut turns = HashMap::new();
    for event in events {
        *turns.entry(event.chat_id.clone()).or_insert(0) += 1;
    }
    let long = "a deliberately wide line that exercises the horizontal code well";
    turns
}

fn main() {
    let turns = replay(&load_events("./events.jsonl"));
    println!("{} sessions", turns.len());
}
```"""

TABLE = """The frozen parity numbers:

| Surface | React | Rune | Delta |
| --- | ---: | ---: | ---: |
| **Paragraph** | 2.103 | 2.103 | 0.000 |
| `Code block` | 2.064 | 2.064 | 0.000 |
| Diff view | 2.254 | 2.254 | 0.000 |"""


def make_diff(total: int = 820) -> str:
    """A unified diff whose parsed line count is exactly `total`."""
    lines = [
        "diff --git a/src/parity.rs b/src/parity.rs",
        "index 1a2b3c4..5d6e7f8 100644",
        "--- a/src/parity.rs",
        "+++ b/src/parity.rs",
        "@@ -1,3 +1,3 @@",
    ]
    body = total - len(lines)
    for i in range(1, body + 1):
        if i % 17 == 0:
            lines.append(f"-old snapshot line {i}")
        elif i % 23 == 0:
            lines.append(f"+new snapshot line {i}")
        else:
            lines.append(f" context row {i} keeps the diff readable")
    return "\n".join(lines)


DIFF = """The generated diff stays collapsed past 800 lines:

```diff
""" + make_diff() + """
```"""


def message(mid: str, role: str, content: str, stamp: str) -> dict:
    return {
        "content": content,
        "id": mid,
        "metadata": {},
        "role": role,
        "timestamp": stamp,
    }


def chat(chat_id: str, title: str, stamp: str, user: str, agent: str) -> dict:
    return {
        "createdAt": stamp,
        "id": chat_id,
        "isPinned": False,
        "messages": [
            message(f"{chat_id}-user", "user", user, stamp),
            message(f"{chat_id}-agent", "agent", agent, stamp.replace(":00Z", ":05Z")),
        ],
        "model": "deepseek-v4.1-flash",
        "title": title,
        "updatedAt": stamp,
        "workspaceId": "_general",
        "workspacePath": "",
    }


def summary(chat_id: str, title: str, stamp: str, user: str, agent: str) -> dict:
    return {
        "createdAt": stamp,
        "id": chat_id,
        "lastAgent": {
            "content": agent.splitlines()[0],
            "id": f"{chat_id}-agent",
            "role": "agent",
            "timestamp": stamp.replace(":00Z", ":05Z"),
        },
        "lastKind": "",
        "lastMessage": agent.splitlines()[0],
        "lastRole": "agent",
        "lastUser": {
            "content": user,
            "id": f"{chat_id}-user",
            "role": "user",
            "timestamp": stamp,
        },
        "messageCount": 2,
        "model": "deepseek-v4.1-flash",
        "status": "done",
        "title": title,
        "updatedAt": stamp,
    }


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    # Newest first: React renders this array in order while the native shell
    # sorts by `updated_at` descending, so both must agree on the listing.
    chats = [
        ("chat-md-diff", "Markdown · diff", "2026-11-12T11:24:00Z", "Show the parity diff.", DIFF),
        ("chat-md-table", "Markdown · table", "2026-11-12T11:22:00Z", "Render the parity table.", TABLE),
        ("chat-md-code", "Markdown · code", "2026-11-12T11:20:00Z", "Show the replay loop.", CODE),
    ]
    workspaces = {
        "workspaces": [
            {
                "addedAt": "",
                "chatCount": len(chats),
                "exists": True,
                "id": "_general",
                "isDfd": False,
                "isGeneral": True,
                "isHouse": False,
                "isPersonal": False,
                "name": "General",
                "path": "",
                "updatedAt": "2026-11-12T11:24:05Z",
                "workspaceKind": "general",
            }
        ]
    }
    (OUT / "api_workspaces.json").write_text(json.dumps(workspaces, indent=2) + "\n")
    (OUT / "api_workspaces__general_chats.json").write_text(
        json.dumps({"chats": [summary(*c) for c in chats]}, indent=2) + "\n"
    )
    (OUT / "api_ui_state.json").write_text(
        json.dumps(
            {
                "active_tab": "_general:chat-md-code",
                "chats_filter": "general",
                "open_tabs": ["_general:chat-md-code"],
                "pinned_chats": [],
            },
            indent=2,
        )
        + "\n"
    )
    for chat_id, title, stamp, user, agent in chats:
        payload = chat(chat_id, title, stamp, user, agent)
        (OUT / f"api_workspaces__general_chats_{chat_id}.json").write_text(
            json.dumps(payload, indent=2) + "\n"
        )
    print(f"wrote {len(list(OUT.glob('*.json')))} fixtures to {OUT}")


if __name__ == "__main__":
    main()
