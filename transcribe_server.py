"""
Minimal transcription server — runs on port 8002 to avoid port 8000 conflicts.
Just handles POST /api/transcribe using Groq Whisper.
"""
import os
import json
from fastapi import FastAPI, UploadFile, File
from fastapi.middleware.cors import CORSMiddleware
from dotenv import load_dotenv
import uvicorn

load_dotenv(override=True)

app = FastAPI()
app.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["*"], allow_headers=["*"])

@app.post("/api/transcribe")
async def transcribe_audio(file: UploadFile = File(...)):
    GROQ_API_KEY = os.getenv("GROQ_API_KEY") or ""
    if not GROQ_API_KEY:
        keys_str = os.getenv("GROQ_API_KEYS", "")
        if keys_str:
            GROQ_API_KEY = keys_str.split(",")[0].strip()
    if not GROQ_API_KEY:
        return {"error": "Groq API key not configured"}, 400

    audio_bytes = await file.read()
    if not audio_bytes:
        return {"error": "Empty audio file"}, 400

    try:
        import httpx
        files = {
            "file": (file.filename or "audio.webm", audio_bytes, file.content_type or "audio/webm"),
            "model": (None, "whisper-large-v3-turbo"),
        }
        headers = {"Authorization": f"Bearer {GROQ_API_KEY}"}
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.post(
                "https://api.groq.com/openai/v1/audio/transcriptions",
                files=files, headers=headers,
            )
        if resp.status_code != 200:
            return {"error": f"Groq API error: {resp.status_code}"}, 502
        result = resp.json()
        text = result.get("text", "").strip()
        return {"text": text}, 200
    except Exception as e:
        return {"error": str(e)}, 500

if __name__ == "__main__":
    print("[TranscribeServer] Starting on port 8002...")
    uvicorn.run(app, host="127.0.0.1", port=8003)
