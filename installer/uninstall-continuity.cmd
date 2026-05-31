@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall-continuity.ps1"
exit /b %ERRORLEVEL%
