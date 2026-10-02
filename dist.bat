@echo off
cd /d %~dp0
echo Building release...
cargo build --release
if errorlevel 1 (
    echo.
    echo [ERROR] cargo build --release failed. See output above.
    pause
    exit /b 1
)
if not exist dist mkdir dist
copy /Y target\release\delta-trade-observer.exe dist\ >nul
copy /Y nn-classifier\model.onnx dist\ >nul
echo.
echo [DONE] dist/ generated. Contents:
dir dist
pause
