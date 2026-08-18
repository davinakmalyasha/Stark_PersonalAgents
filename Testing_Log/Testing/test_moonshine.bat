@echo off
title Pure Moonshine Test
echo ============================================
echo   PURE MOONSHINE TEST - just speak!
echo ============================================
echo.
echo   When it says "SAY SOMETHING", talk into
echo   your microphone until the timer finishes.
echo   (a few seconds)
echo.
echo   Then it transcribes and shows the result.
echo   The recording is saved to:
echo   scratch\pure_test_capture.wav
echo.
cd /d D:\MyProject\LangChain
D:\MyProject\LangChain\venv312\Scripts\python.exe scratch\pure_moonshine_test.py 8
echo.
pause
