@echo off
title Stark - Web Mode
echo =============================================
echo   Stark - Web Mode (Browser)
echo =============================================
echo   Starting API server (venv 3.14)...
if defined VOICE_PYTHON (
  start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && %VOICE_PYTHON% server\api_server.py"
) else if exist "D:\MyProject\LangChain\venv\Scripts\python.exe" (
  start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && D:\MyProject\LangChain\venv\Scripts\python.exe server\api_server.py"
) else (
  start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && D:\MyProject\LangChain\venv312\Scripts\python.exe server\api_server.py"
)
echo.
echo   Starting Vite dev server...
cd /d D:\MyProject\LangChain\frontend
call npm run dev
