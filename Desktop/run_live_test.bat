@echo off
title Stark Live Test — Moonshine + Pocket (double-click)
cd /d D:\MyProject\LangChain
echo ===========================================
echo   Stark Independent Live Test
echo   Mic -> Moonshine -> Pocket TTS -> Speaker
echo   Lightweight, no Electron/server
echo ===========================================
echo   Voice: alba  (change to jarvis if you want clone)
echo   Close with Ctrl+C
echo.
if exist "venv\Scripts\python.exe" (
  venv\Scripts\python.exe scratch\live_moonshine_pocket.py --voice alba
) else if exist "venv312\Scripts\python.exe" (
  venv312\Scripts\python.exe scratch\live_moonshine_pocket.py --voice alba
) else (
  python scratch\live_moonshine_pocket.py --voice alba
)
echo.
echo --- ended, press any key to close ---
pause >nul
