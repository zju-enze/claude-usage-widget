@echo off
REM Claude-Code-Usage-Monitor 后台守护 + 悬浮窗
REM 双击启动，开始常驻监控；最小化托盘

setlocal
set "SCRIPT_DIR=%~dp0"
set "STATE_DIR=%USERPROFILE%\.claude-monitor\state"
set "LOG_FILE=%USERPROFILE%\.claude-monitor\monitor.log"

if not exist "%STATE_DIR%" mkdir "%STATE_DIR%"

echo [%date% %time%] Starting claude-monitor (--write-state loop) >> "%LOG_FILE%"

:loop
claude-monitor --write-state --refresh-rate 5 --no-clear >> "%LOG_FILE%" 2>&1
echo [%date% %time%] monitor exited, restarting in 5s >> "%LOG_FILE%"
timeout /t 5 /nobreak >nul
goto loop
