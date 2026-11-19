@echo off
setlocal
title Stark - Web Mode
for %%I in ("%~dp0..") do set "ROOT=%%~fI"
if not defined STARK_HARNESS_ROOT set "STARK_HARNESS_ROOT=%ROOT%"
set "STARK_PORT=8124"
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
  if exist "%ROOT%\backend-rs\target\debug\stark-server.exe" set "SERVER_BIN=%ROOT%\backend-rs\target\debug\stark-server.exe"
)
if not defined SERVER_BIN (
  echo   No Rust server binary found. Build it with: cd backend-rs ^&^& cargo build -p stark-server
  exit /b 1
)
echo   Starting Rust API server...
start "Stark-API" /D "%ROOT%" cmd /c "set STARK_PORT=8124 && ""%SERVER_BIN%"""
echo.
echo   Starting Vite dev server...
cd /d "%ROOT%\frontend"
call npm run dev
