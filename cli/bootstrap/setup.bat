@echo off
powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%~dp0.project\setup\bootstrap.ps1" %*
exit /b %errorlevel%
