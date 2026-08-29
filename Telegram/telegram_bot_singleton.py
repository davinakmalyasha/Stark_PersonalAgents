"""telegram_bot_singleton.py --- Shared reference to the running Telegram bot.

The bot instance lives in Telegram/bot.py (built at import). DFD approval
requests (Telegram/dfd_approvals.py) import this module to send inline
keyboard messages to the owner, and the callback handler in bot.py resolves
them back into dfd_approvals.
"""

from __future__ import annotations

import os

#: Populated by Telegram/bot.py once the TeleBot is created.
bot = None

#: Owner chat id (the person who approves DFD actions) — env-configured.
BOT_OWNER_CHAT_ID = os.getenv("TELEGRAM_OWNER_CHAT_ID", "")


def get_bot():
    return bot
