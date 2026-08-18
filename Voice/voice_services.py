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


def generate_tts_sync(text: str, output_path: str, voice="en-US-ChristopherNeural"):
    """
    Generates TTS with the on-device Kokoro engine (JARVIS clone when the
    reference audio exists, otherwise the male am_michael voice).
    """
    try:
        import audio_engine
        local_voice = "jarvis" if os.path.exists(os.path.join(_ROOT, ".deep_agents", "tts_ref", "jarvis.wav")) else "am_michael"
        wav_path = os.path.splitext(output_path)[0] + ".wav"
        if audio_engine.synthesize_tts(text, wav_path, voice=local_voice):
            return wav_path
        print("[voice_services] Kokoro TTS failed")
        return None
    except Exception as e:
        print("[voice_services] Kokoro TTS unavailable:", e)
        return None
