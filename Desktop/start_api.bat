@echo off
title Stark - API Server (Rust)
set "STARK_PORT=8123"
echo =============================================
echo   Restarting Stark API server (port %STARK_PORT%)
echo =============================================

for /f "tokens=5" %%a in ('netstat -ano ^| findstr ":%STARK_PORT%" ^| findstr "LISTENING"') do (
    echo   Killing old server PID %%a
    taskkill /F /T /PID %%a >nul 2>&1
)
timeout /t 2 /nobreak >nul

type nul > "D:\MyProject\LangChain\.deep_agents\api_out.log"
type nul > "D:\MyProject\LangChain\.deep_agents\api_err.log"

set "SERVER_BIN="
if defined STARK_BACKEND_BIN (
  if exist "%STARK_BACKEND_BIN%" set "SERVER_BIN=%STARK_BACKEND_BIN%"
)
if not defined SERVER_BIN (
  if exist "D:\cargo-target\stark\debug\stark-server.exe" set "SERVER_BIN=D:\cargo-target\stark\debug\stark-server.exe"
)
if not defined SERVER_BIN (
  if exist "D:\MyProject\LangChain\backend-rs\target\debug\stark-server.exe" set "SERVER_BIN=D:\MyProject\LangChain\backend-rs\target\debug\stark-server.exe"
)

if not defined SERVER_BIN (
  echo   No Rust server binary found. Build it with: cd backend-rs ^&^& cargo build -p stark-server
  exit /b 1
)

echo   Starting stark-server (logs in .deep_agents\api_out.log) ...
start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && set STARK_PORT=%STARK_PORT% && ""%SERVER_BIN%"" >> D:\MyProject\LangChain\.deep_agents\api_out.log 2>> D:\MyProject\LangChain\.deep_agents\api_err.log"

echo   Done. Check http://127.0.0.1:%STARK_PORT%/api/health for engine status.
echo   (Frontend changes: just refresh the browser / reopen the voice overlay)
