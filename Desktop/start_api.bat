@echo off
title Stark - API Server (venv312)
echo =============================================
echo   Restarting Stark API server (port 8000)
echo =============================================

REM Kill anything already listening on port 8000
for /f "tokens=5" %%a in ('netstat -ano ^| findstr ":8000" ^| findstr "LISTENING"') do (
    echo   Killing old server PID %%a
    taskkill /F /PID %%a >nul 2>&1
)
timeout /t 2 /nobreak >nul

REM Start the API server with venv312 (has Moonshine + Kokoro).
REM api_server.py re-execs itself with venv312 even if launched otherwise.
REM Logs go to .deep_agents\api_out.log / api_err.log for debugging.
echo   Starting api_server.py (logs in .deep_agents\api_out.log) ...
start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && D:\MyProject\LangChain\venv312\Scripts\python.exe server\api_server.py >> D:\MyProject\LangChain\.deep_agents\api_out.log 2>> D:\MyProject\LangChain\.deep_agents\api_err.log"

echo   Done. Check http://127.0.0.1:8000/api/health for engine status.
echo   (Frontend changes: just refresh the browser / reopen the voice overlay)
