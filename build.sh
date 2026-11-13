#!/bin/sh
# только для кросс-проверки на Linux (mingw); на Windows используйте build.bat
# без аргументов: сначала DLL миниатюр (декодеры — на размер), потом программа;
# с аргументами — как есть
B() {
  RUSTC_BOOTSTRAP=1 cargo build --release --target x86_64-pc-windows-gnu -Zbuild-std=std,panic_abort \
    --config 'target.x86_64-pc-windows-gnu.linker="x86_64-w64-mingw32-gcc"' "$@"
}
if [ $# -eq 0 ]; then
  Z=""
  for p in rav1d heic-decoder rusty_h265 rusty_h265-accel image-webp; do
    Z="$Z --config profile.release.package.$p.opt-level='z'"
  done
  # shellcheck disable=SC2086
  B -p spv_thumbs --target-dir target/thumbs $Z &&
    SPV_THUMBS_DLL="$(pwd)/target/thumbs/x86_64-pc-windows-gnu/release/spv_thumbs.dll" B -p simple_photo_viewer
else
  B "$@"
fi
