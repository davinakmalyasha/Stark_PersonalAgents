@echo off
title Stark - Desktop (Prod, lightweight)
echo =============================================
echo   Stark - Desktop Prod Mode (lightweight)
echo   dist/ build + Electron. No vite, no DevTools.
echo =============================================
echo.

REM Single-server discipline: if the API port already answers, the app (or a
REM localhost server) is already up - refuse to double-launch instead of
REM warming duplicate models and letting the guard kill the loser.
REM Override with: set FORCE=1
set "STARK_PORT=8123"
if not "%FORCE%"=="1" (
  netstat -ano ^| findstr ":%STARK_PORT%" ^| findstr "LISTENING" >nul 2>&1
  if not errorlevel 1 (
    echo   Port %STARK_PORT% is already LISTENING - Stark is probably running.
    echo   Check the tray icon, or set FORCE=1 to proceed anyway.
    echo.
    pause
    exit /b 1
  )
)

cd /d "%~dp0..\frontend"

REM Build frontend dist so the installed Stark + this launcher always serve current code.
REM Set SKIP_BUILD=1 to skip (hot reload), or it auto-rebuilds if src newer than dist.
if "%SKIP_BUILD%"=="1" goto :launch
echo   Building frontend dist (so Stark picks up latest UI)...
call node_modules\.bin\tsc.cmd -b
if errorlevel 1 (
  echo   TypeScript build failed - aborting.
  pause
  exit /b 1
)
call node_modules\.bin\vite.cmd build
if errorlevel 1 (
  echo   Frontend build failed - aborting.
  pause
  exit /b 1
)

:launch
echo   Starting Stark (prod, ELECTRON_PROD=1)...
set "ELECTRON_PROD=1"
if exist "node_modules\.bin\electron.cmd" (
  start "Stark" "%~dp0..\frontend\node_modules\.bin\electron.cmd" .
) else (
  echo   electron not found in node_modules - falling back to npx...
  start "Stark" npx --no-install electron .
)
echo   Done. The tray icon owns the app; closing the window frees the renderer.
