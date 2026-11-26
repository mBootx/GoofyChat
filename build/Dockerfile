# Toolchain image that builds the Android APK. Run through build/build-apk.sh.
FROM rust:1-bookworm

ENV ANDROID_HOME=/opt/android \
    NDK_HOME=/opt/android/ndk/27.2.12479018 \
    JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64

RUN apt-get update && apt-get install -y --no-install-recommends openjdk-17-jdk-headless unzip python3

RUN curl -fsSL -o /tmp/tools.zip https://dl.google.com/android/repository/commandlinetools-linux-13114758_latest.zip \
    && mkdir -p $ANDROID_HOME/cmdline-tools \
    && unzip -q /tmp/tools.zip -d $ANDROID_HOME/cmdline-tools \
    && mv $ANDROID_HOME/cmdline-tools/cmdline-tools $ANDROID_HOME/cmdline-tools/latest \
    && rm /tmp/tools.zip

RUN yes | $ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager --licenses > /dev/null \
    && $ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager \
        'platform-tools' 'platforms;android-36' 'build-tools;35.0.0' 'ndk;27.2.12479018' > /dev/null

RUN rustup target add wasm32-unknown-unknown aarch64-linux-android armv7-linux-androideabi
RUN cargo install tauri-cli --version '^2' --locked && cargo install trunk --locked
