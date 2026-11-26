#!/bin/sh
# Runs inside the builder container. /src is read-only, /out receives the APK.
set -eu

rm -rf /work/project
mkdir -p /work/project
cp -r /src/app /src/proto /work/project/
cd /work/project/app

trunk build --release
cargo tauri icon icon.svg
cargo tauri android init --ci
python3 android/patch.py "$PWD"
cargo tauri android build --apk --target aarch64 armv7

unsigned=$(find src-tauri/gen/android/app/build/outputs/apk -name '*-release-unsigned.apk' | head -n 1)
tools=$ANDROID_HOME/build-tools/35.0.0

# The signing key is created once and kept on the build server. Android only
# installs an update signed with the same key, so back it up.
if [ ! -f /keys/release.jks ]; then
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > /keys/password
    keytool -genkeypair -keystore /keys/release.jks -alias goofychat -keyalg RSA -keysize 3072 \
        -validity 10000 -dname 'CN=Goofy Chat' -storepass:file /keys/password -keypass:file /keys/password
    chmod 600 /keys/release.jks /keys/password
fi

"$tools/zipalign" -f -p 4 "$unsigned" /work/aligned.apk
"$tools/apksigner" sign --ks /keys/release.jks --ks-key-alias goofychat \
    --ks-pass file:/keys/password --out /out/GoofyChat.apk /work/aligned.apk
"$tools/apksigner" verify /out/GoofyChat.apk
rm -f /out/GoofyChat.apk.idsig
ls -l /out/GoofyChat.apk
