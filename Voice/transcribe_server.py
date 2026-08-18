"""
Minimal transcription server — runs on port 8002/8003.
POST /api/transcribe using the on-device Moonshine engine (free, CPU).
"""
import os
import sys
import tempfile
import uuid
import asyncio
from fastapi import FastAPI, UploadFile, File
from fastapi.middleware.cors import CORSMiddleware
from dotenv import load_dotenv
import uvicorn

load_dotenv(override=True)

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

app = FastAPI()
app.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"])

@app.post("/api/transcribe")
async def transcribe_audio(file: UploadFile = File(...)):
    tmp_path = None
    try:
        import audio_engine
        audio_bytes = await file.read()
        if not audio_bytes:
            return {"error": "Empty audio file"}, 400
        tmp_path = os.path.join(tempfile.gettempdir(), f"stt_{uuid.uuid4().hex}.webm")
        with open(tmp_path, "wb") as f:
            f.write(audio_bytes)
        text = (await asyncio.to_thread(audio_engine.transcribe_file, tmp_path) or "").strip()
        if not text:
            return {"error": "Moonshine could not transcribe the audio"}, 422
        return {"text": text, "engine": "moonshine"}
    except Exception as e:
        print(f"[TranscribeServer] Moonshine ASR unavailable: {e}")
        return {"error": f"Moonshine ASR unavailable: {e}"}, 503
    finally:
        if tmp_path and os.path.exists(tmp_path):
            try:
                os.remove(tmp_path)
            except OSError:
                pass

if __name__ == "__main__":
    print("[TranscribeServer] Starting on port 8002...")
    uvicorn.run(app, host="127.0.0.1", port=8003)
