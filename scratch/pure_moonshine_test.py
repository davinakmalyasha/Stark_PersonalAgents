"""PURE Moonshine test — no project code involved.

Records N seconds from your microphone and transcribes with raw
moonshine_voice (medium-streaming). If this fails to capture your voice,
the problem is the model/mic — not the project configuration.

Usage:
    python scratch/pure_moonshine_test.py [seconds] [language]
"""
import sys
import time

import numpy as np
import sounddevice as sd
import soundfile as sf

from moonshine_voice.transcriber import Transcriber
from moonshine_voice.moonshine_api import ModelArch
from moonshine_voice.download import find_model_info, download_model_from_info

SECONDS = float(sys.argv[1]) if len(sys.argv) > 1 else 8
LANG = sys.argv[2] if len(sys.argv) > 2 else "en"
RATE = 16000

print("=" * 60)
print("PURE MOONSHINE TEST (medium-streaming, no project code)")
print("=" * 60)

# 1. Model (official moonshine tooling, cached on D:)
print(f"[1/3] Resolving model for language={LANG}, arch=MEDIUM_STREAMING ...")
info = find_model_info(LANG, ModelArch.MEDIUM_STREAMING)
model_dir, _ = download_model_from_info(info, cache_root="D:/MyProject/LangChain/.deep_agents/models")
print(f"     model: {model_dir}")

# 2. Record from the mic
print(f"[2/3] Recording {SECONDS}s from your microphone — SAY SOMETHING ...")
audio = sd.rec(int(SECONDS * RATE), samplerate=RATE, channels=1, dtype="float32")
sd.wait()
audio = audio.flatten()
peak = float(np.max(np.abs(audio))) if len(audio) else 0.0
frame = 1600  # 100ms frames
loud_frames = sum(1 for i in range(0, len(audio) - frame, frame) if float(np.max(np.abs(audio[i:i+frame]))) > 0.02)
speech_ratio = loud_frames / max(1, len(audio) // frame)
print(f"     captured {len(audio)} samples, peak amplitude={peak:.3f}, active-speech ratio={speech_ratio:.0%}")
if speech_ratio < 0.05:
    print("     !! capture is basically SILENT — no voice reached the mic.")
elif peak < 0.01:
    print("     !! VERY quiet input (peak < 0.01) — check mic volume/settings")
wav_path = "D:/MyProject/LangChain/scratch/pure_test_capture.wav"
sf.write(wav_path, audio, RATE)
print(f"     saved capture to {wav_path} (open it and listen — is your voice there?)")

# 3. Pure transcription (no normalization, no ffmpeg, no project wrapper)
print("[3/3] Transcribing with pure Moonshine (two paths) ...")

# 3a. Direct in-memory floats
t0 = time.time()
tr = Transcriber(str(model_dir), model_arch=ModelArch.MEDIUM_STREAMING)
result = tr.transcribe_without_streaming(list(audio), sample_rate=RATE)
text_a = " ".join(line.text for line in result.lines).strip()
print(f"     A) in-memory floats  ({time.time()-t0:.1f}s): '{text_a}'")

# 3b. Through moonshine's own WAV loader (identical to how real files are read)
from moonshine_voice.utils import load_wav_file
t0 = time.time()
audio2, sr2 = load_wav_file(wav_path)
result2 = tr.transcribe_without_streaming(audio2, sample_rate=sr2)
text_b = " ".join(line.text for line in result2.lines).strip()
print(f"     B) load_wav_file     ({time.time()-t0:.1f}s): '{text_b}'")

print("-" * 60)
print(f"RESULT A: '{text_a}'")
print(f"RESULT B: '{text_b}'")
print("-" * 60)
text = text_a or text_b
print("Verdict:")
if text:
    print("  Moonshine captured your voice -> the model is FINE;")
    print("  the problem is in the project configuration/pipeline.")
else:
    print("  Moonshine could NOT capture your voice -> model/mic issue,")
    print("  or the recording is too quiet/short. Check scratch/pure_test_capture.wav.")
