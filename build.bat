@echo off
chcp 65001 >nul
rem 1) DLL миниатюр для Проводника, 2) сама программа (DLL вшивается в exe)
cargo build --release -p spv_thumbs || exit /b 1
cargo build --release -p simple_photo_viewer || exit /b 1
echo.
echo Готово: target\release\simple_photo_viewer.exe
for %%A in (target\release\simple_photo_viewer.exe) do echo Размер: %%~zA байт
