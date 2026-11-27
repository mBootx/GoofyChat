#!/bin/sh
# Runs inside the Windows builder container. /src is read-only, /out receives the installer.
set -eu

rm -rf /work/project-windows
mkdir -p /work/project-windows
cp -r /src/app /src/proto /work/project-windows/
cd /work/project-windows/app

trunk build --release
cargo tauri icon icon.svg
cargo tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis

setup=$(find "$CARGO_TARGET_DIR/x86_64-pc-windows-msvc/release/bundle/nsis" -name '*-setup.exe' | head -n 1)
if [ -z "$setup" ]; then
    echo "No installer found." >&2
    exit 1
fi
cp "$setup" /out/GoofyChat-setup.exe
ls -l /out/GoofyChat-setup.exe
