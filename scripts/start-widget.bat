@echo off
REM 启动 Claude Usage Widget 悬浮窗
cd /d "%~dp0\.."
call npm run tauri dev
