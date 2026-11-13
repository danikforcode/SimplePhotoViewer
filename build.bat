@echo off
chcp 65001 >nul
rem 1) DLL миниатюр для Проводника (свои декодеры — на размер: миниатюры
rem    Проводник кэширует, скорость там не главное), 2) сама программа
rem    (DLL вшивается в exe сжатой)
set Z=--config profile.release.package.rav1d.opt-level='z' --config profile.release.package.heic-decoder.opt-level='z' --config profile.release.package.rusty_h265.opt-level='z' --config profile.release.package.rusty_h265-accel.opt-level='z' --config profile.release.package.image-webp.opt-level='z'
cargo build --release -p spv_thumbs --target-dir target\thumbs %Z% || exit /b 1
set SPV_THUMBS_DLL=%~dp0target\thumbs\release\spv_thumbs.dll
cargo build --release -p simple_photo_viewer || exit /b 1
echo.
echo Готово: target\release\simple_photo_viewer.exe
for %%A in (target\release\simple_photo_viewer.exe) do echo Размер: %%~zA байт
