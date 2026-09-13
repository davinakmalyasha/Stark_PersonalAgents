@echo off
title VinCode — Desktop (Prod, lightweight)
echo =============================================
echo   VinCode — Desktop Prod Mode (lightweight)
echo   dist/ build + Electron. No vite, no DevTools.
echo =============================================
echo.

REM Single-server discipline: if the API port already answers, the app (or a
REM localhost server) is already up — refuse to double-launch instead of
REM warming duplicate models and letting the guard kill the loser.
REM Override with: set FORCE=1
set "STARK_PORT=8123"
if not "%FORCE%"=="1" (
  netstat -ano ^| findstr ":%STARK_PORT%" ^| findstr "LISTENING" >nul 2>&1
  if not errorlevel 1 (
    echo   Port %STARK_PORT% is already LISTENING — VinCode is probably running.
    echo   Check the tray icon (hover shows the API PID), or set FORCE=1 to proceed anyway.
    echo.
    pause
    exit /b 1
  )
)

cd /d D:\MyProject\LangChain\frontend

REM Build once if dist is missing or stale (skip with SKIP_BUILD=1).
if "%SKIP_BUILD%"=="1" goto :launch
if not exist "dist\index.html" (
  echo   dist/ missing — building once (this takes a while, then never again)...
  call npm run build
  if errorlevel 1 (
    echo   Frontend build failed — aborting.
    pause
    exit /b 1
  )
)

:launch
echo   Starting VinCode (prod, ELECTRON_PROD=1)...
set "ELECTRON_PROD=1"
if exist "node_modules\.bin\electron.cmd" (
  start "VinCode" node_modules\.bin\electron.cmd .
) else (
  echo   electron not found in node_modules — falling back to npx...
  start "VinCode" npx --no-install electron .
)
echo   Done. The tray icon owns the app; closing the window frees the renderer.
