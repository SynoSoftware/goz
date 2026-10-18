@echo off
setlocal
title Build the gozler installer
pwsh.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0installer\Build.ps1" %*
if errorlevel 1 (
    echo.
    echo Installer build failed. The error above explains what to fix.
    pause
    exit /b 1
)
start "" "%~dp0target\installer"
