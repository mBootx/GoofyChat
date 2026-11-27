#!/bin/sh
# Runs inside the Linux builder container. /src is read-only, /out receives the AppImage.
set -eu

rm -rf /work/project-linux
mkdir -p /work/project-linux
cp -r /src/app /src/proto /work/project-linux/
cd /work/project-linux/app

trunk build --release
cargo tauri icon icon.svg
# --verbose shows linuxdeploy's own messages when packaging fails.
cargo tauri build --bundles appimage --verbose

image=$(find "$CARGO_TARGET_DIR/release/bundle/appimage" -name '*.AppImage' | head -n 1)
if [ -z "$image" ]; then
    echo "No AppImage found." >&2
    exit 1
fi
cp "$image" /out/GoofyChat.AppImage
# New dependencies change the lock file; hand it back so it can be kept.
cmp -s src-tauri/Cargo.lock /src/app/src-tauri/Cargo.lock || cp src-tauri/Cargo.lock /out/Cargo.lock.new
chmod 755 /out/GoofyChat.AppImage
ls -l /out/GoofyChat.AppImage
