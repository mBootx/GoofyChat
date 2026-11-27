#!/bin/sh
# Cross-builds the Windows installer into dist/GoofyChat-setup.exe. Needs only Docker.
# The installer is unsigned, like the one built on Windows.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$root/dist"

docker build --network host -t goofychat-builder "$root/build"
docker build --network host -t goofychat-builder-windows -f "$root/build/Dockerfile.windows" "$root/build"

# Limits keep the other services on the machine responsive. The Windows SDK
# downloaded by cargo-xwin is kept in its own volume.
docker run --rm --network host --memory 4g --cpu-shares 256 \
    -v "$root:/src:ro" \
    -v "$root/dist:/out" \
    -v goofychat-cargo:/usr/local/cargo/registry \
    -v goofychat-target-windows:/work/target \
    -v goofychat-xwin:/root/.cache/cargo-xwin \
    -e CARGO_TARGET_DIR=/work/target \
    -e XWIN_ACCEPT_LICENSE=1 \
    goofychat-builder-windows sh /src/build/inside-windows.sh
