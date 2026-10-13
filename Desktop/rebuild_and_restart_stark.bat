@echo off
title Rebuild Stark Frontend
echo Rebuilding frontend/dist so Stark (installed) picks up latest UI...
cd /d "%~dp0..\frontend"
call node_modules\.bin\tsc.cmd -b
if errorlevel 1 (
  echo TypeScript build failed.
  pause
  exit /b 1
)
call node_modules\.bin\vite.cmd build
if errorlevel 1 (
  echo Build failed.
  pause
  exit /b 1
)
echo Build OK. Restarting Stark...
taskkill /F /IM Stark.exe 2>nul
timeout /t 2 /nobreak >nul
start "" "%LOCALAPPDATA%\Programs\Stark\Stark.exe"
echo Done. Stark restarted.
