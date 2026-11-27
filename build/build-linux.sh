#!/bin/sh
# Builds the Linux AppImage into dist/GoofyChat.AppImage. Needs only Docker.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$root/dist"

docker build --network host -t goofychat-builder "$root/build"
docker build --network host -t goofychat-builder-linux -f "$root/build/Dockerfile.linux" "$root/build"

# Limits keep the other services on the machine responsive. linuxdeploy is an
# AppImage itself and there is no FUSE in the container: it unpacks itself instead.
docker run --rm --network host --memory 4g --cpu-shares 256 \
    -v "$root:/src:ro" \
    -v "$root/dist:/out" \
    -v goofychat-cargo:/usr/local/cargo/registry \
    -v goofychat-target-linux:/work/target \
    -e CARGO_TARGET_DIR=/work/target \
    -e APPIMAGE_EXTRACT_AND_RUN=1 \
    goofychat-builder-linux sh /src/build/inside-linux.sh
