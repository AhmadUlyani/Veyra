@echo off
rem Double-click to rebuild Veyra: closes the running copy, builds, then starts it again.
cd /d "%~dp0"
title Rebuild Veyra

echo Closing Veyra if it is running...
if exist "target\release\veyra.exe" "target\release\veyra.exe" --quit
rem give it a moment to save and close, then make sure it is gone
for /l %%i in (1,1,10) do (
    tasklist /fi "imagename eq veyra.exe" 2>nul | find /i "veyra.exe" >nul || goto closed
    timeout /t 1 /nobreak >nul
)
taskkill /im veyra.exe /f >nul 2>&1
timeout /t 1 /nobreak >nul
:closed

echo Building...
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0build.ps1"
if errorlevel 1 (
    echo.
    echo BUILD FAILED - see the messages above.
    pause
    exit /b 1
)

echo Starting Veyra...
start "" "%~dp0target\release\veyra.exe"
echo Done.
timeout /t 2 >nul
