"""
User Questions — provider-neutral ask() seam (dsh-style user-questions).

An agent asks the human one or more structured questions and BLOCKS until the
answers arrive (or a timeout aborts). Providers deliver the question and
collect answers:

- ``HttpStoreProvider`` (default): pending batches live in a process store;
  the FastAPI surface exposes GET /api/ask/pending and POST /api/ask/answer.
- ``TelegramProvider``: inline-keyboard buttons via a telebot instance
  (registered from bot.py).
- ``CliProvider``: print + input() for headless runs.

The ``plan-review`` intent tags a question as a plan submission for approval:
``approve`` names the affirmative option; every other option declines.

Errors are structured (``UserQuestionError.code``): EMPTY_QUESTIONS,
NO_PROVIDER, ASK_ABORTED, CALLER_NOT_LIVE.
"""

from __future__ import annotations

import json
import threading
import time
import uuid
from dataclasses import dataclass, field
from typing import Any, Callable, Optional


class UserQuestionError(Exception):
    def __init__(self, code: str, message: str = ""):
        self.code = code
        super().__init__(message or code)


# ── Question vocabulary ──────────────────────────────────────────────────────


@dataclass
class QuestionOption:
    label: str
    description: str = ""


@dataclass
class PlanReviewIntent:
    kind: str = "plan-review"
    approve: str = ""


@dataclass
class AskQuestion:
    id: str
    question: str
    detail: str = ""
    header: str = ""
    options: list[QuestionOption] = field(default_factory=list)
    multi_select: bool = False
    intent: Optional[PlanReviewIntent] = None


@dataclass
class AnswerItem:
    id: str
    selected: list[str] = field(default_factory=list)
    custom: str = ""


def _parse_questions(raw: Any) -> list[AskQuestion]:
    """Coerce model-provided question args into AskQuestion objects."""
    if isinstance(raw, dict):
        raw = [raw]
    questions: list[AskQuestion] = []
    for item in raw or []:
        if not isinstance(item, dict):
            continue
        qid = str(item.get("id") or "").strip()
        text = str(item.get("question") or "").strip()
        if not qid or not text:
            continue
        options = []
        for opt in item.get("options") or []:
            if isinstance(opt, str):
                options.append(QuestionOption(label=opt))
            elif isinstance(opt, dict):
                options.append(QuestionOption(
                    label=str(opt.get("label") or ""),
                    description=str(opt.get("description") or ""),
                ))
        intent = None
        if isinstance(item.get("intent"), dict) and item["intent"].get("kind") == "plan-review":
            intent = PlanReviewIntent(
                kind="plan-review",
                approve=str(item["intent"].get("approve") or ""),
            )
        questions.append(AskQuestion(
            id=qid,
            question=text,
            detail=str(item.get("detail") or ""),
            header=str(item.get("header") or ""),
            options=options,
            multi_select=bool(item.get("multiSelect") or item.get("multi_select")),
            intent=intent,
        ))
    return questions


# ── Provider protocol ────────────────────────────────────────────────────────

ProviderFn = Callable[[list[AskQuestion], float, Optional[str]], list[AnswerItem]]

_active_provider: Optional[tuple[str, ProviderFn]] = None
_provider_lock = threading.RLock()

# Process store for the default HTTP provider: batch_id -> pending batch
_pending: dict[str, dict[str, Any]] = {}
_pending_lock = threading.RLock()


def register_provider(name: str, fn: ProviderFn) -> None:
    """Set the active provider (last registration wins)."""
    global _active_provider
    with _provider_lock:
        _active_provider = (name, fn)


def active_provider() -> Optional[tuple[str, ProviderFn]]:
    with _provider_lock:
        return _active_provider


# ── ask() ────────────────────────────────────────────────────────────────────


def ask(
    questions: Any,
    timeout: Optional[float] = None,
    chat_id: Optional[str] = None,
) -> list[AnswerItem]:
    """Ask the human; blocks until answered or aborted.

    Returns a list of :class:`AnswerItem` in question order. Raises
    :class:`UserQuestionError` with codes EMPTY_QUESTIONS / NO_PROVIDER /
    ASK_ABORTED.
    """
    parsed = _parse_questions(questions)
    if not parsed:
        raise UserQuestionError("EMPTY_QUESTIONS", "No valid questions supplied")

    provider = active_provider()
    if provider is None:
        # Default: the process-store HTTP provider (resolvable via the API
        # surface even when nothing else registered a provider).
        provider = ("http-store", http_provider)

    if timeout is None:
        timeout = float(_ask_timeout())

    try:
        answers = provider[1](parsed, timeout, chat_id)
    except UserQuestionError:
        raise
    except Exception as e:
        raise UserQuestionError("PROVIDER_ERROR", f"Provider failed: {e}")

    by_id = {q.id: q for q in parsed}
    out: list[AnswerItem] = []
    for a in answers or []:
        q = by_id.get(a.id)
        if q is None:
            continue
        selected = list(a.selected or [])
        if q.multi_select:
            out.append(AnswerItem(id=a.id, selected=selected, custom=a.custom or ""))
        else:
            if a.custom:
                out.append(AnswerItem(id=a.id, selected=[], custom=a.custom))
            else:
                out.append(AnswerItem(id=a.id, selected=selected[:1]))
    # fill missing questions with empty answers so the caller sees every id
    answered_ids = {a.id for a in out}
    for q in parsed:
        if q.id not in answered_ids:
            out.append(AnswerItem(id=q.id))
    return out


def _ask_timeout() -> str:
    import os
    return os.environ.get("ASK_TIMEOUT", "600")


# ── HttpStoreProvider (default) ──────────────────────────────────────────────


def http_provider(questions: list[AskQuestion], timeout: float, chat_id: Optional[str] = None) -> list[AnswerItem]:
    """Register a pending batch and block until answered or timed out."""
    batch_id = uuid.uuid4().hex[:8]
    event = threading.Event()
    batch = {
        "batch_id": batch_id,
        "questions": [_question_to_dict(q) for q in questions],
        "answers": {},
        "event": event,
        "created": time.time(),
        "timeout": timeout,
    }
    with _pending_lock:
        _pending[batch_id] = batch

    try:
        # Drop stale batches opportunistically
        now = time.time()
        for bid, b in list(_pending.items()):
            if b is not batch and now - b["created"] > b["timeout"] + 60:
                _pending.pop(bid, None)
        if not event.wait(timeout):
            raise UserQuestionError(
                "ASK_ABORTED",
                f"Timed out waiting for the user's answer to '{questions[0].id}' after {int(timeout)}s.",
            )
        return [_answer_to_item(a) for a in batch["answers"].values()]
    finally:
        with _pending_lock:
            _pending.pop(batch_id, None)


def pending_batches() -> list[dict]:
    """Summaries of unanswered batches (GET /api/ask/pending)."""
    with _pending_lock:
        return [
            {
                "batch_id": b["batch_id"],
                "questions": b["questions"],
                "created": b["created"],
            }
            for b in _pending.values()
        ]


def resolve_batch(batch_id: str, answers: list[dict]) -> bool:
    """Resolve one pending batch (POST /api/ask/answer)."""
    with _pending_lock:
        batch = _pending.get(batch_id)
        if batch is None:
            return False
        for a in answers or []:
            if not isinstance(a, dict):
                continue
            qid = str(a.get("id") or "")
            if not qid:
                continue
            batch["answers"][qid] = {
                "id": qid,
                "selected": list(a.get("selected") or []),
                "custom": str(a.get("custom") or ""),
            }
        batch["event"].set()
        return True


# ── TelegramProvider ─────────────────────────────────────────────────────────


def _question_to_dict(q: AskQuestion) -> dict:
    d: dict[str, Any] = {
        "id": q.id,
        "question": q.question,
        "multiSelect": q.multi_select,
    }
    if q.detail:
        d["detail"] = q.detail
    if q.header:
        d["header"] = q.header
    if q.options:
        d["options"] = [
            {"label": o.label, "description": o.description} if o.description else o.label
            for o in q.options
        ]
    if q.intent is not None:
        d["intent"] = {"kind": q.intent.kind, "approve": q.intent.approve}
    return d


def _answer_to_item(a: dict) -> AnswerItem:
    return AnswerItem(
        id=str(a.get("id") or ""),
        selected=list(a.get("selected") or []),
        custom=str(a.get("custom") or ""),
    )


def telegram_provider(bot: Any) -> ProviderFn:
    """Build a provider bound to a telebot instance.

    Renders one message per question with an inline keyboard (one button per
    option). Single-select resolves on click; multi-select toggles until
    "Done". "Type answer" lets the user reply with free text.
    """
    handler_registered = threading.Event()
    pending: dict[str, dict[str, Any]] = {}
    pending_lock = threading.RLock()

    def _handle_callback(call: Any) -> None:
        try:
            data = call.data or ""
            if not data.startswith("uq:"):
                return
            _, batch_id, qid, action = data.split(":", 3)
            with pending_lock:
                batch = pending.get(batch_id)
                if batch is None:
                    return
                question = next((q for q in batch["questions"] if q.id == qid), None)
                if question is None:
                    return
                if action == "done":
                    batch["event"].set()
                elif action == "custom":
                    batch["awaiting_custom"] = (qid, call.message.chat.id, call.message.message_id)
                    try:
                        bot.answer_callback_query(call.id, "Reply with your answer text.")
                    except Exception:
                        pass
                elif question.multi_select:
                    current = batch["answers"].get(qid, {"id": qid, "selected": [], "custom": ""})
                    selected = current["selected"]
                    if action in selected:
                        selected = [s for s in selected if s != action]
                    else:
                        selected.append(action)
                    batch["answers"][qid] = {"id": qid, "selected": selected, "custom": ""}
                    try:
                        bot.answer_callback_query(call.id, "Toggled")
                    except Exception:
                        pass
                else:
                    batch["answers"][qid] = {"id": qid, "selected": [action], "custom": ""}
                    batch["event"].set()
        except Exception:
            pass

    def _handle_text(msg: Any) -> None:
        try:
            with pending_lock:
                for batch_id, batch in pending.items():
                    awaiting = batch.get("awaiting_custom")
                    if not awaiting:
                        continue
                    qid, chat_id, message_id = awaiting
                    if int(msg.chat.id) != int(chat_id):
                        continue
                    batch["answers"][qid] = {"id": qid, "selected": [], "custom": msg.text or ""}
                    batch.pop("awaiting_custom", None)
                    batch["event"].set()
                    try:
                        bot.edit_message_text(
                            f"*{qid}*: answered (custom).",
                            chat_id=chat_id, message_id=message_id,
                            parse_mode="Markdown",
                        )
                    except Exception:
                        pass
                    return
        except Exception:
            pass

    def _ensure_handlers() -> None:
        if handler_registered.is_set():
            return
        with pending_lock:
            if handler_registered.is_set():
                return
            try:
                bot.callback_query_handler(func=lambda c: True)(_handle_callback)
                bot.message_handler(func=lambda m: True, content_types=["text"])(_handle_text)
            except Exception:
                pass
            handler_registered.set()

    def _ask(questions: list[AskQuestion], timeout: float, chat_id: Optional[str] = None) -> list[AnswerItem]:
        _ensure_handlers()
        batch_id = uuid.uuid4().hex[:8]
        event = threading.Event()
        batch = {
            "batch_id": batch_id,
            "chat_id": chat_id or "0",
            "questions": questions,
            "answers": {},
            "event": event,
            "created": time.time(),
            "timeout": timeout,
        }
        with pending_lock:
            pending[batch_id] = batch

        try:
            for q in questions:
                buttons = []
                for opt in q.options:
                    cb = f"uq:{batch_id}:{q.id}:{opt.label}"
                    if len(cb) > 64:
                        cb = f"uq:{batch_id}:{q.id}:{opt.label[:40]}"
                    buttons.append(
                        [{"text": opt.label, "callback_data": cb}]
                    )
                row = []
                if q.multi_select:
                    row.append({"text": "✓ Done", "callback_data": f"uq:{batch_id}:{q.id}:done"})
                row.append({"text": "✏️ Type answer", "callback_data": f"uq:{batch_id}:{q.id}:custom"})
                buttons.append(row)

                header = f"*{q.header}*\n" if q.header else ""
                detail = f"\n{q.detail}" if q.detail else ""
                text = f"{header}*{q.question}*{detail}"
                if q.intent is not None and q.intent.kind == "plan-review":
                    text = f"{text}\n\n*(plan review — {q.intent.approve or 'approve'} to accept, any other option to decline)*"
                if q.multi_select:
                    text = f"{text}\n\n_(multi-select: tap options, then Done)_"
                try:
                    bot.send_message(batch["chat_id"], text, reply_markup={"inline_keyboard": buttons}, parse_mode="Markdown")
                except Exception:
                    pass
            if not event.wait(timeout):
                raise UserQuestionError(
                    "ASK_ABORTED",
                    f"Timed out waiting for the user's answer after {int(timeout)}s.",
                )
            return [_answer_to_item(a) for a in batch["answers"].values()]
        finally:
            with pending_lock:
                pending.pop(batch_id, None)

    return _ask


# ── CliProvider ──────────────────────────────────────────────────────────────


def cli_provider(questions: list[AskQuestion], timeout: float, chat_id: Optional[str] = None) -> list[AnswerItem]:
    """Headless fallback: print the questions and read answers from stdin."""
    answers: list[AnswerItem] = []
    for q in questions:
        print(f"\n[{q.id}] {q.question}")
        if q.detail:
            print(f"  {q.detail}")
        if q.intent is not None and q.intent.kind == "plan-review":
            print(f"  (plan review — {q.intent.approve or 'approve'} to accept, any other option to decline)")
        options = [o.label for o in q.options]
        if options:
            print("  Options: " + ", ".join(options))
            choice = input("  > ").strip()
            if choice in options:
                answers.append(AnswerItem(id=q.id, selected=[choice]))
            elif choice:
                answers.append(AnswerItem(id=q.id, custom=choice))
            else:
                answers.append(AnswerItem(id=q.id))
        else:
            choice = input("  > ").strip()
            answers.append(AnswerItem(id=q.id, custom=choice))
    return answers
