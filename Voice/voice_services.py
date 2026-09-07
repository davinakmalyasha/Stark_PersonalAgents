import os
import asyncio
import sys

# Make the project root importable (audio_engine lives there)
_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)


def transcribe_audio(file_path: str) -> str:
    """Transcribe an audio file with the on-device Moonshine engine."""
    try:
        import audio_engine
        text = audio_engine.transcribe_file(file_path)
        if text:
            return text
        return "Error: Could not transcribe audio."
    except Exception as e:
        print("[voice_services] Moonshine ASR unavailable:", e)
        return f"Error transcribing audio: {e}"


def generate_tts_sync(text: str, output_path: str, voice="alba"):
    """
    Generates TTS with the on-device Pocket TTS engine (Kyutai).
    'jarvis' clones .deep_agents/tts_ref/jarvis.mp3 (requires HF_TOKEN +
    accepted gated-model terms); other names are catalog voices (alba, anna,
    michael, ...) or local audio/safetensors paths.
    """
    try:
        import audio_engine
        if audio_engine.synthesize_tts(text, output_path, voice=voice):
            return output_path
        print("[voice_services] Pocket TTS failed")
        return None
    except Exception as e:
        print("[voice_services] Pocket TTS unavailable:", e)
        return None
