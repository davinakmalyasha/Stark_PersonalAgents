@echo off
title Stark - Server Mode (Mobile / Remote)
echo =============================================
echo   Stark - Server Mode
echo   UI + API on one origin, reachable by phone
echo =============================================
echo.
echo   Building frontend (skip with SKIP_BUILD=1)...
cd /d D:\MyProject\LangChain\frontend
if "%SKIP_BUILD%"=="1" goto :serve
call npm run build
if errorlevel 1 (
  echo   Frontend build failed — serving API only.
)
:serve
echo.
echo   Starting server on all interfaces (pairing token required for phones)...
cd /d D:\MyProject\LangChain
set STARK_BIND=0.0.0.0
D:\MyProject\LangChain\venv312\Scripts\python.exe server\api_server.py
