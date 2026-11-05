@echo off
rem Double-click to restart Veyra with logging on. Reproduce the problem, then tell Claude
rem roughly when it happened; the log is at %TEMP%\veyra.log (times are in milliseconds).
cd /d "%~dp0.."
if exist "target\release\veyra.exe" "target\release\veyra.exe" --quit
for /l %%i in (1,1,10) do (
    tasklist /fi "imagename eq veyra.exe" 2>nul | find /i "veyra.exe" >nul || goto closed
    timeout /t 1 /nobreak >nul
)
taskkill /im veyra.exe /f >nul 2>&1
:closed
del "%TEMP%\veyra.log" >nul 2>&1
set VEYRA_DEBUG=1
start "" "target\release\veyra.exe"
echo Veyra is running with logging on. Log: %TEMP%\veyra.log
timeout /t 4 >nul
