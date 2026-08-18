import os
import sys

# ── Path setup: root + Voice/ (voice_services lives in Voice/) ────────────────
_HERE = os.path.dirname(os.path.abspath(__file__))          # Telegram/
_ROOT = os.path.dirname(_HERE)                              # project root
_VOICE = os.path.join(_ROOT, "Voice")
for _p in [_ROOT, _VOICE]:
    if _p not in sys.path:
        sys.path.insert(0, _p)

WORKSPACE_DIR = r"D:\MyProject"

def safe_remove(path: str) -> None:
    try:
        os.remove(path)
    except Exception:
        pass

def transcribe_voice(bot, msg) -> str:
    """Downloads and transcribes a voice message."""
    import voice_services
    try:
        file_info = bot.get_file(msg.voice.file_id)
        downloaded = bot.download_file(file_info.file_path)
        audio_path = os.path.join(WORKSPACE_DIR, f"voice_{msg.message_id}.ogg")
        with open(audio_path, "wb") as f:
            f.write(downloaded)
        text = voice_services.transcribe_audio(audio_path)
        safe_remove(audio_path)
        return text
    except Exception as e:
        return f"Error transcribing: {e}"


def safe_send_message(bot, chat_id, text, retries=3, **kwargs):
    """Resilient wrapper for bot.send_message with exponential backoff on timeouts."""
    import time
    from requests.exceptions import ReadTimeout, ConnectionError
    for attempt in range(retries):
        try:
            return bot.send_message(chat_id, text, **kwargs)
        except (ReadTimeout, ConnectionError) as e:
            if attempt == retries - 1:
                print(f"Error sending message after {retries} attempts: {e}")
                # Try sending one last time without special timeout
                try:
                    return bot.send_message(chat_id, text, **kwargs)
                except Exception:
                    return None
            time.sleep(1 * (attempt + 1))
        except Exception as e:
            print(f"Unexpected error sending message: {e}")
            return None


def safe_edit_message_text(bot, text, chat_id, message_id, retries=3, **kwargs):
    """Resilient wrapper for bot.edit_message_text to handle timeouts and 'not modified' errors."""
    import time
    from requests.exceptions import ReadTimeout, ConnectionError
    for attempt in range(retries):
        try:
            return bot.edit_message_text(text, chat_id, message_id, **kwargs)
        except (ReadTimeout, ConnectionError) as e:
            if attempt == retries - 1:
                print(f"Error editing message after {retries} attempts: {e}")
                try:
                    return bot.edit_message_text(text, chat_id, message_id, **kwargs)
                except Exception:
                    return None
            time.sleep(1 * (attempt + 1))
        except Exception as e:
            if "message is not modified" in str(e).lower():
                return True
            print(f"Unexpected error editing message: {e}")
            return None


