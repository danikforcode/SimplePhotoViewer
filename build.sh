#!/bin/sh
# только для кросс-проверки на Linux (mingw); на Windows используйте build.bat
RUSTC_BOOTSTRAP=1 cargo build --release --target x86_64-pc-windows-gnu -Zbuild-std=std,panic_abort \
  --config 'target.x86_64-pc-windows-gnu.linker="x86_64-w64-mingw32-gcc"' "$@"
