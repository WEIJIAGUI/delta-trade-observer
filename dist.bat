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
REM 按钮布局检测器（可兑换/不可兑换场景判定）随包分发到 dist/nn-classifier/（Rust 端搜索路径之一）。
if exist nn-classifier\buttons.onnx (
    if not exist dist\nn-classifier mkdir dist\nn-classifier
    copy /Y nn-classifier\buttons.onnx dist\nn-classifier\ >nul
)
REM 子弹数量整体识别模型（0~200 整块区域分类）随包分发到 dist/nn-classifier/（Rust 端搜索路径之一）。
if exist nn-classifier\qty.onnx (
    if not exist dist\nn-classifier mkdir dist\nn-classifier
    copy /Y nn-classifier\qty.onnx dist\nn-classifier\ >nul
)
echo.
echo [DONE] dist/ generated. Contents:
dir dist
pause
