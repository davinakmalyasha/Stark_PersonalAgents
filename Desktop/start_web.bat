@echo off
title Stark - Web Mode
echo =============================================
echo   Stark - Web Mode (Browser)
echo =============================================
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
echo   Starting Rust API server...
start "Stark-API" cmd /c "cd /d D:\MyProject\LangChain && ""%SERVER_BIN%"""
echo.
echo   Starting Vite dev server...
cd /d D:\MyProject\LangChain\frontend
call npm run dev
