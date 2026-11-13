@echo off
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0.project\docs\bootstrap.ps1" %*
exit /b %errorlevel%
