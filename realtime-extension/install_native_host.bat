@echo off
REM ============================================================
REM install_native_host.bat - Register Native Messaging Host
REM
REM What it does:
REM   1. Regenerates com.video.analyzer.json with the launcher path
REM      pointing to THIS folder (works at any extraction location)
REM   2. Writes registry entries (HKCU, no admin needed) for both
REM      Chrome and Edge
REM
REM After running: the browser extension can auto start/stop the
REM local Rust backend (image-scheduler-rs.exe).
REM
REM NOTE: If the extension ID in chrome://extensions differs from
REM ofjehhcfkeffhlahceameagmmacjobni, edit com.video.analyzer.json
REM and update allowed_origins to your actual extension ID.
REM ============================================================
setlocal

set "HOST_NAME=com.video.analyzer"
set "SCRIPT_DIR=%~dp0"
set "JSON_PATH=%SCRIPT_DIR%com.video.analyzer.json"
set "LAUNCHER_PATH=%SCRIPT_DIR%native_launcher.bat"

REM --- 1. Regenerate JSON with correct absolute path ---
(
echo {
echo   "name": "com.video.analyzer",
echo   "description": "Video Analyzer Native Host - manages Rust backend service",
echo   "path": "%LAUNCHER_PATH:\=\\%",
echo   "type": "stdio",
echo   "allowed_origins": [
echo     "chrome-extension://ofjehhcfkeffhlahceameagmmacjobni/"
echo   ]
echo }
) > "%JSON_PATH%"

REM --- 2. Write registry entries (Chrome + Edge) ---
reg add "HKCU\Software\Google\Chrome\NativeMessagingHosts\%HOST_NAME%" /ve /t REG_SZ /d "%JSON_PATH%" /f >nul 2>&1
reg add "HKCU\Software\Microsoft\Edge\NativeMessagingHosts\%HOST_NAME%" /ve /t REG_SZ /d "%JSON_PATH%" /f >nul 2>&1

echo.
echo [OK] Native Host registered!
echo   JSON: %JSON_PATH%
echo.
echo [Hint] If the extension ID shown in chrome://extensions is not
echo   ofjehhcfkeffhlahceameagmmacjobni, open the JSON above and
echo   update allowed_origins to match your actual ID.
echo.
pause
