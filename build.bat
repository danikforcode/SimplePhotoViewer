@echo off
chcp 65001 >nul
cargo build --release || exit /b 1
echo.
echo Готово: target\release\simple_photo_viewer.exe
for %%A in (target\release\simple_photo_viewer.exe) do echo Размер: %%~zA байт
