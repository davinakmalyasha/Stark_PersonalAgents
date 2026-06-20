import os
import asyncio

def transcribe_audio(file_path: str) -> str:
    """Uses Google's Free Web STT to transcribe an audio file."""
    try:
        import speech_recognition as sr
        from pydub import AudioSegment
        # Convert audio to wav for SpeechRecognition
        wav_path = file_path + ".wav"
        audio = AudioSegment.from_file(file_path)
        audio.export(wav_path, format="wav")
        
        recognizer = sr.Recognizer()
        with sr.AudioFile(wav_path) as source:
            audio_data = recognizer.record(source)
            text = recognizer.recognize_google(audio_data)
            
        try: os.remove(wav_path)
        except: pass
        
        return text
    except sr.UnknownValueError:
        return "Error: Could not understand audio."
    except sr.RequestError as e:
        return f"Error: STT Service request failed; {e}"
    except Exception as e:
        print("Transcription error:", e)
        return f"Error transcribing audio: {e}"

def generate_tts_sync(text: str, output_path: str, voice="en-US-ChristopherNeural"):
    """
    Generates TTS using edge-tts. 
    It runs an asyncio subprocess to call the edge-tts CLI, 
    as the python package is highly asynchronous and we want a sync wrapper.
    """
    try:
        os.system(f'edge-tts --voice {voice} --text "{text}" --write-media "{output_path}"')
        return output_path
    except Exception as e:
        print("TTS Generation error:", e)
        return None
