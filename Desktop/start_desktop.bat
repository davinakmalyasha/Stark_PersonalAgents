@echo off
title Stark - Desktop (Dev)
echo =============================================
echo   Stark - Desktop Mode (dev, live reload)
echo =============================================
echo.
echo Starting Stark App...
cd /d "%~dp0..\frontend"
call npm run electron:dev
