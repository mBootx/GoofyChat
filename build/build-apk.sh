#!/bin/sh
# Builds the signed APK into dist/GoofyChat.apk. Needs only Docker.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
keys=${KEYS_DIR:-$HOME/goofychat-keys}
mkdir -p "$keys" "$root/dist"
chmod 700 "$keys"

docker build --network host -t goofychat-builder "$root/build"

# Limits keep the other services on the machine responsive.
docker run --rm --network host --memory 4g --cpu-shares 256 \
    -v "$root:/src:ro" \
    -v "$root/dist:/out" \
    -v "$keys:/keys" \
    -v goofychat-cargo:/usr/local/cargo/registry \
    -v goofychat-gradle:/root/.gradle \
    -v goofychat-target:/work/target \
    -e CARGO_TARGET_DIR=/work/target \
    -e GRADLE_OPTS='-Dorg.gradle.daemon=false -Dorg.gradle.jvmargs=-Xmx1536m' \
    goofychat-builder sh /src/build/inside.sh
