@echo off
title Stark - Web Mode
echo =============================================
echo   Stark - Web Mode (Browser)
echo =============================================
echo   Starting API server (venv312)...
start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && D:\MyProject\LangChain\venv312\Scripts\python.exe server\api_server.py"
echo.
echo   Starting Vite dev server...
cd /d D:\MyProject\LangChain\frontend
call npm run dev
