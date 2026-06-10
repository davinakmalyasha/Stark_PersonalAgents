@echo off
title Antigravity 2.0 — Web Mode
echo =============================================
echo   Antigravity 2.0 — Web Mode (Browser)
echo =============================================
echo.
echo Starting API server...
start "Antigravity-API" cmd /c "cd /d D:\MyProject\LangChain && python api_server.py"
echo.
echo Starting Vite dev server...
cd /d D:\MyProject\LangChain\frontend
call npm run dev
