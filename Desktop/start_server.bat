@echo off
setlocal
title Stark - Server Mode (Mobile / Remote)
for %%I in ("%~dp0..") do set "ROOT=%%~fI"
if not defined STARK_HARNESS_ROOT set "STARK_HARNESS_ROOT=%ROOT%"
echo =============================================
echo   Stark - Server Mode
echo   UI + API on one origin, reachable by phone
echo =============================================
echo.
echo   Building frontend (skip with SKIP_BUILD=1)...
cd /d "%ROOT%\frontend"
if "%SKIP_BUILD%"=="1" goto :serve
call npm run build
if errorlevel 1 (
  echo   Frontend build failed — serving API only.
)
:serve
echo.
echo   Starting Rust server on all interfaces (pairing token required for phones)...
cd /d "%ROOT%"
set STARK_HOST=0.0.0.0

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

"%SERVER_BIN%"
