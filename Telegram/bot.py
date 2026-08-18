"""
Telegram Bot — Entry point for the Virtual IT Department.
Handles text/voice input and runs the agentic pipeline (orchestrator + subagents).
"""
import os
import sys

# ── Path setup: root (for .env, state_sync etc.) + Telegram/ (siblings) ───────
_HERE = os.path.dirname(os.path.abspath(__file__))   # Telegram/
_ROOT = os.path.dirname(_HERE)                       # project root
for _p in [_ROOT, _HERE]:
    if _p not in sys.path:
        sys.path.insert(0, _p)

import threading
import telebot
from dotenv import load_dotenv
from bot_helpers import transcribe_voice, safe_send_message, safe_edit_message_text, WORKSPACE_DIR
from bot_runner import handle_build_request

load_dotenv(override=True)

token = os.getenv("TELEGRAM_BOT_TOKEN")
if not token:
    raise ValueError("TELEGRAM_BOT_TOKEN not set in .env")
bot = telebot.TeleBot(token)

# ── P5: ask_user answers arrive through Telegram inline keyboards ──
try:
    from user_questions import register_provider, telegram_provider
    register_provider("telegram", telegram_provider(bot))
except Exception as e:
    print(f"[BOT] Telegram question provider unavailable: {e}")

# ── Inbox (P2): per-chat run tracking so a message during a run steers
# instead of spawning a concurrent second run ──
_active_runs: set[int] = set()
_active_runs_lock = threading.Lock()


def _is_run_active(chat_id: int) -> bool:
    with _active_runs_lock:
        return chat_id in _active_runs


def _mark_run(chat_id: int, active: bool) -> None:
    with _active_runs_lock:
        if active:
            _active_runs.add(chat_id)
        else:
            _active_runs.discard(chat_id)

@bot.message_handler(commands=["start", "help"])
def welcome(msg: telebot.types.Message) -> None:
    try:
        bot.reply_to(
            msg,
            "🏢 *Virtual IT Department*\n\n"
            "I can help you build, deploy, and analyze software projects autonomously. Send me a command to get started:\n"
            "• _\"Make a calculator app\"_ → spec & plan → Dev implementation → CI gate verification\n"
            "• _\"Harden my deployment / set up CI/CD\"_ → Dev + the CI security gate\n"
            "• _\"Write requirements for a CRM\"_ → requirements analysis\n"
            "• _\"Quick question or research\"_ → FreeAgent (zero cost)\n\n"
            "Ready to generate documentation, establish project environments, and build functional systems.",
            parse_mode="Markdown",
        )
    except Exception:
        try:
            bot.reply_to(
                msg,
                "🏢 Virtual IT Department\n\n"
                "I can help you build, deploy, and analyze software projects autonomously. Send me a command to get started:\n"
                "• \"Make a calculator app\"\n"
                "• \"Harden my deployment / set up CI/CD\"\n"
                "• \"Write requirements for a CRM\"\n"
                "• \"Quick question or research\"\n\n"
                "Ready to generate documentation, establish project environments, and build functional systems."
            )
        except Exception as e:
            print(f"Error sending welcome message: {e}")

_CHAT_KEYWORDS = {"hello", "hi", "hai", "yo", "halo", "helo", "hey", "thanks", "thank you", "good morning",
                  "good afternoon", "how are you", "what's up", "bye", "goodbye"}

def _is_obvious_chat(text: str) -> bool:
    import re
    text_lower = text.lower().strip()
    words = text_lower.rstrip(".!?").split()
    if len(words) > 6:
        return False
    
    # Greetings & thanks keyword check with word boundaries to avoid substring matches
    greetings = [
        r"\bhello\b", r"\bhi\b", r"\bhai\b", r"\byo\b", r"\bhalo\b", r"\bhelo\b",
        r"\bhey\b", r"\bthanks\b", r"\bthank\s+you\b",
        r"\bgood\s+morning\b", r"\bgood\s+afternoon\b", r"\bhow\s+are\s+you\b",
        r"\bwhat's\s+up\b", r"\bbye\b", r"\bgoodbye\b"
    ]
    has_greeting = any(re.search(pattern, text_lower) for pattern in greetings)
    if not has_greeting:
        return False

    # Ensure it doesn't contain any task action words
    actions = [
        "build", "create", "make", "write", "code", "fix", "implement",
        "add", "change", "modify", "delete", "remove", "refactor", "deploy",
        "run", "test"
    ]
    has_action = any(re.search(rf"\b{act}\b", text_lower) for act in actions)
    return not has_action

@bot.message_handler(content_types=["text", "voice"])
def handle_request(msg: telebot.types.Message) -> None:
    chat_id = msg.chat.id
    status_msg = None

    if msg.content_type == "voice":
        status_msg = safe_send_message(bot, chat_id, "🎙️ Transcribing voice...")
        if status_msg is None:
            return
        req = transcribe_voice(bot, msg)
        if req.startswith("Error"):
            safe_edit_message_text(bot, f"❌ {req}", chat_id, status_msg.message_id)
            return
        safe_edit_message_text(
            bot,
            f"📝 Transcribed: \"{req}\"\n\n⏳ Processing...",
            chat_id, status_msg.message_id,
        )
    else:
        req = msg.text

    # ── Inbox (P2): while a run is in progress, text steers; /cancel aborts ──
    if msg.content_type == "text":
        low = (req or "").strip().lower()
        if low in ("/cancel", "/stop", "/abort"):
            from inbox import abort
            abort(str(chat_id), keep_inbox=True)
            safe_send_message(bot, chat_id, "⏹️ Cancelling the current run...")
            return
        if _is_run_active(chat_id):
            from inbox import steer
            steer(str(chat_id), req)
            safe_send_message(bot, chat_id,
                              "📨 Queued as a steering message — the agent will see it at its next step. "
                              "Send /cancel to abort the run.")
            return

    from memory.chat_memory_manager import add_chat_message, resolve_request_text
    add_chat_message("user", req, workspace_path=WORKSPACE_DIR, chat_id=str(chat_id))

    # Clear failed models from previous requests
    try:
        from sync.state_sync import safe_update_state
        safe_update_state({"failed_models": []})
    except Exception:
        pass

    resolved_req = resolve_request_text(req, workspace_path=WORKSPACE_DIR, chat_id=str(chat_id))

    # Quick chat check — no LLM call needed
    if _is_obvious_chat(req):
        add_chat_message("jarvis", "Hi! Ready to help. What do you need?", workspace_path=WORKSPACE_DIR, chat_id=str(chat_id))
        reply = "Hi! Ready to help. What do you need?"
        if status_msg:
            safe_edit_message_text(bot, reply, chat_id, status_msg.message_id)
        else:
            safe_send_message(bot, chat_id, reply)
        return

    # Go directly to the main agent
    add_chat_message("jarvis", f"Initiating work: {resolved_req}", workspace_path=WORKSPACE_DIR, chat_id=str(chat_id))

    if status_msg is None:
        status_msg = safe_send_message(bot, chat_id, "⏳ Processing your request...")
    else:
        safe_edit_message_text(bot, "⏳ Processing your request...", chat_id, status_msg.message_id)

    _mark_run(chat_id, True)
    try:
        handle_build_request(bot, resolved_req, chat_id, status_msg)
    finally:
        _mark_run(chat_id, False)

if __name__ == "__main__":
    print("[BOT] Telegram bot is running...")
    bot.infinity_polling()
