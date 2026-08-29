@echo off
title Stark - API Server (venv 3.14)
set "STARK_PORT=8123"
echo =============================================
echo   Restarting Stark API server (port %STARK_PORT%)
echo =============================================

REM Kill anything already listening on the Stark port (tree-kill: the old
REM `start "Stark-API" cmd /c` leaves a detached cmd handle that plain
REM taskkill /F /PID misses — /T kills the whole tree).
for /f "tokens=5" %%a in ('netstat -ano ^| findstr ":%STARK_PORT%" ^| findstr "LISTENING"') do (
    echo   Killing old server PID %%a
    taskkill /F /T /PID %%a >nul 2>&1
)
REM Also kill ANY python running api_server.py (not just port holder) so a
REM duplicate model can never stay resident — the root cause of 2xRAM/CPU
REM + choppy TTS + red-log storm.
for /f "tokens=1 delims= " %%p in ('wmic process where "name='python.exe'" get processid 2^>nul') do (
    echo   Checking PID %%p
)
for /f "tokens=*" %%l in ('wmic process where "name='python.exe'" get processid^,commandline /format:csv 2^>nul ^| findstr /i "api_server.py"') do (
    for /f "tokens=2 delims=," %%p in ("%%l") do (
        echo   Killing stale api_server PID %%p
        taskkill /F /T /PID %%p >nul 2>&1
    )
)
taskkill /F /IM python.exe /FI "WINDOWTITLE eq Stark-API" >nul 2>&1
timeout /t 2 /nobreak >nul

REM Log rotation: truncate (not append) on every start so the logs can never
REM grow unbounded — access-log flood / flush spam would otherwise bloat them.
type nul > "D:\MyProject\LangChain\.deep_agents\api_out.log"
type nul > "D:\MyProject\LangChain\.deep_agents\api_err.log"

REM Start the API server with venv (Python 3.14). VOICE_PYTHON env overrides for legacy voice (venv312).
REM Logs go to .deep_agents\api_out.log / api_err.log for debugging.
echo   Starting api_server.py (logs in .deep_agents\api_out.log) ...
if defined VOICE_PYTHON (
  start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && set STARK_PORT=%STARK_PORT% && %VOICE_PYTHON% server\api_server.py >> D:\MyProject\LangChain\.deep_agents\api_out.log 2>> D:\MyProject\LangChain\.deep_agents\api_err.log"
) else if exist "D:\MyProject\LangChain\venv\Scripts\python.exe" (
  start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && set STARK_PORT=%STARK_PORT% && D:\MyProject\LangChain\venv\Scripts\python.exe server\api_server.py >> D:\MyProject\LangChain\.deep_agents\api_out.log 2>> D:\MyProject\LangChain\.deep_agents\api_err.log"
) else (
  start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && set STARK_PORT=%STARK_PORT% && D:\MyProject\LangChain\venv312\Scripts\python.exe server\api_server.py >> D:\MyProject\LangChain\.deep_agents\api_out.log 2>> D:\MyProject\LangChain\.deep_agents\api_err.log"
)

echo   Done. Check http://127.0.0.1:%STARK_PORT%/api/health for engine status.
echo   (Frontend changes: just refresh the browser / reopen the voice overlay)
