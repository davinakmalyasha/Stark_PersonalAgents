"""
dfd_approvals.py --- Owner-approval channel for DFD agency actions.

The DFD agent must NOT create client scaffolds (or other gated agency
actions) without explicit owner approval. This module provides the approval
channel:

- default: Telegram inline keyboard (approve / reject) via the running bot;
- fallback: DFD chat approval via the existing ask_user machinery.

Design:
- request_approval(proposal) -> submits a proposal and BLOCKS until the owner
  answers (or the timeout elapses).
- The Telegram handler registers a callback and resolves the pending request.
- Pending requests are persisted to .deep_agents/dfd_approvals.json so the
  owner can answer later and the agent picks it up on resume.
"""

from __future__ import annotations

import json
import os
import threading
import time
import uuid
from datetime import datetime

_HARNESS_ROOT = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_APPROVALS_PATH = os.path.join(_HARNESS_ROOT, ".deep_agents", "dfd_approvals.json")

_lock = threading.RLock()
_pending: dict[str, dict] = {}
_waiters: dict[str, dict] = {}


def _load() -> None:
    global _pending
    try:
        if os.path.isfile(_APPROVALS_PATH):
            with open(_APPROVALS_PATH, "r", encoding="utf-8") as f:
                _pending = json.load(f)
    except Exception:
        _pending = {}


def _save() -> None:
    try:
        os.makedirs(os.path.dirname(_APPROVALS_PATH), exist_ok=True)
        with open(_APPROVALS_PATH, "w", encoding="utf-8") as f:
            json.dump(_pending, f, indent=2)
    except Exception:
        pass


def _ensure_loaded() -> None:
    if not _pending:
        _load()


def request_approval(
    proposal: dict,
    channel: str = "telegram",
    timeout: int = 0,
    blocking: bool = True,
) -> dict:
    """Request owner approval for a gated agency action.

    ``proposal`` must contain at least:
      - kind (e.g. "create_client")
      - summary (one-line human description, shown to the owner)
      - detail (multi-line structured detail)
    Returns ``{"approved": True/False, "request_id": ..., "answer": ...}``.
    With ``blocking=False``, returns immediately with the request id (the
    owner can answer later; resume with ``check_approval(request_id)``).
    """
    _ensure_loaded()
    request_id = f"aprv-{uuid.uuid4().hex[:10]}"
    entry = {
        "id": request_id,
        "kind": str(proposal.get("kind") or "general"),
        "summary": str(proposal.get("summary") or "Agency action"),
        "detail": str(proposal.get("detail") or ""),
        "channel": channel,
        "status": "PENDING",
        "approved": None,
        "createdAt": datetime.now().isoformat(),
        "answeredAt": None,
        "answer": "",
    }
    with _lock:
        _pending[request_id] = entry
        _save()

    # Deliver to the channel.
    delivered = _deliver(entry)
    if not delivered and channel == "telegram":
        # Fallback: no bot running — surface in the DFD chat via ask_user.
        entry["channel"] = "chat"
        _save()
        delivered = _deliver(entry)

    if not blocking:
        return {"request_id": request_id, "status": "PENDING", "delivered": delivered}

    # Block until answered or timeout.
    deadline = time.time() + (timeout if timeout > 0 else 600)
    while time.time() < deadline:
        with _lock:
            cur = _pending.get(request_id)
        if cur and cur.get("status") != "PENDING":
            return _result(cur)
        time.sleep(1)
    with _lock:
        cur = _pending.get(request_id)
    if cur and cur.get("status") != "PENDING":
        return _result(cur)
    return {"request_id": request_id, "approved": False, "status": "TIMEOUT", "answer": ""}


def check_approval(request_id: str) -> dict:
    """Poll a non-blocking approval request for its current state."""
    _ensure_loaded()
    with _lock:
        cur = _pending.get(request_id)
    if not cur:
        return {"request_id": request_id, "status": "NOT_FOUND", "approved": False, "answer": ""}
    return _result(cur)


def resolve_approval(request_id: str, approved: bool, answer: str = "") -> bool:
    """Resolve a pending approval (called by the Telegram callback / chat)."""
    _ensure_loaded()
    with _lock:
        cur = _pending.get(request_id)
        if not cur or cur.get("status") != "PENDING":
            return False
        cur["status"] = "RESOLVED"
        cur["approved"] = bool(approved)
        cur["answer"] = answer or ("Approved" if approved else "Rejected")
        cur["answeredAt"] = datetime.now().isoformat()
        _save()
    return True


def list_pending() -> list[dict]:
    _ensure_loaded()
    return [v for v in _pending.values() if v.get("status") == "PENDING"]


def _result(entry: dict) -> dict:
    return {
        "request_id": entry.get("id"),
        "kind": entry.get("kind"),
        "summary": entry.get("summary"),
        "approved": bool(entry.get("approved")),
        "status": entry.get("status"),
        "answer": entry.get("answer", ""),
    }


def _deliver(entry: dict) -> bool:
    channel = entry.get("channel", "")
    if channel == "telegram":
        return _deliver_telegram(entry)
    if channel == "chat":
        return _deliver_chat(entry)
    return False


def _deliver_telegram(entry: dict) -> bool:
    """Send an inline-keyboard approval message via the running Telegram bot.

    Falls back gracefully when the bot isn't loaded or can't send.
    """
    try:
        import telegram_bot_singleton as _singleton

        bot = _singleton.get_bot()
        chat_id = _singleton.BOT_OWNER_CHAT_ID
    except Exception:
        return False

    try:
        if not chat_id or not bot:
            return False
        from telebot import types

        rid = entry["id"]
        kb = types.InlineKeyboardMarkup(row_width=2)
        kb.add(
            types.InlineKeyboardButton("✅ Approve", callback_data=f"dfd_approve:{rid}"),
            types.InlineKeyboardButton("❌ Reject", callback_data=f"dfd_reject:{rid}"),
        )
        text = (
            f"📋 *DFD Approval Request*\n\n"
            f"*{entry.get('summary')}*\n\n"
            f"{entry.get('detail', '')}"
        )
        bot.send_message(chat_id, text, parse_mode="Markdown", reply_markup=kb)
        return True
    except Exception:
        return False


def _deliver_chat(entry: dict) -> bool:
    """Fallback: surface the approval in the DFD chat via ask_user."""
    try:
        from tools import ask_user

        summary = entry.get("summary", "")
        ans = ask_user(
            questions=[{
                "question": f"DFD approval needed: {summary}",
                "options": ["Approve", "Reject"],
                "header": "DFD Approval",
            }],
        )
        # ask_user returns a list of Answer objects/dicts in practice; treat
        # truthy as approved.
        approved = bool(ans)
        resolve_approval(entry["id"], approved, "Approved" if approved else "Rejected")
        return True
    except Exception:
        return False
