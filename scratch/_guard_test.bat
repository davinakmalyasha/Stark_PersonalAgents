@echo off
if not "%FORCE%"=="1" (
  echo check the icon (hover shows the pid), or set FORCE=1
  if not errorlevel 1 (
    echo inner ok
  )
)
echo done
