#!/bin/sh
# Fast checks on the build machine, without making any app package:
#   sh build/check.sh          server tests + app (wasm) compile check
#   sh build/check.sh server   server tests only
#   sh build/check.sh app      app compile check only
# Uses the Linux builder image (it has Rust, the wasm target and the desktop libraries).
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
what=${1:-all}

docker run --rm --network host --memory 4g --cpu-shares 256 \
    -v "$root:/src" \
    -v goofychat-cargo:/usr/local/cargo/registry \
    -v goofychat-target-check:/work/target \
    -e CARGO_TARGET_DIR=/work/target \
    -e WHAT="$what" \
    --entrypoint sh goofychat-builder-linux -c '
set -e
status=0
if [ "$WHAT" = all ] || [ "$WHAT" = server ]; then
    echo "=== server: cargo test"
    (cd /src/server && cargo test --quiet 2>&1) || status=1
fi
if [ "$WHAT" = all ] || [ "$WHAT" = app ]; then
    echo "=== app: cargo check (wasm32)"
    (cd /src/app && cargo check --quiet --target wasm32-unknown-unknown 2>&1) || status=1
fi
[ $status = 0 ] && echo "=== CHECK OK" || echo "=== CHECK FAILED"
exit $status
'
