# Mortsom runner image (spec Phase 6.1): every tool the coordinator shells
# out to, at pinned versions. Emulators need /dev/kvm passed through
# (`docker run --device /dev/kvm ...`); the run relay needs the Docker
# socket (`-v /var/run/docker.sock:/var/run/docker.sock`).
#
# Nothing in this image is Mortsom logic; it is the host contract that
# `mortsom doctor` verifies.

FROM ubuntu:24.04

ARG RUST_VERSION=1.94.0
ARG JDK_VERSION=17
ARG CMDLINE_TOOLS_VERSION=13114758
ARG ANDROID_PLATFORM=android-35
ARG BUILD_TOOLS=36.0.0
ARG SYSTEM_IMAGE="system-images;android-35;google_apis;x86_64"
ARG NODE_MAJOR=24
ARG APPIUM_VERSION=3.6.0
ARG UIAUTOMATOR2_VERSION=8.4.0
ARG FLUTTER_VERSION=3.41.9
ARG DOCKER_CLI_VERSION=28.3.3

ENV DEBIAN_FRONTEND=noninteractive \
    ANDROID_HOME=/opt/android-sdk \
    ANDROID_SDK_ROOT=/opt/android-sdk \
    ANDROID_AVD_HOME=/root/.android/avd \
    FLUTTER_HOME=/opt/flutter \
    CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:/opt/flutter/bin:/opt/android-sdk/cmdline-tools/latest/bin:/opt/android-sdk/platform-tools:/opt/android-sdk/emulator:/usr/local/bin:/usr/bin:/bin

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git unzip xz-utils zip \
      openjdk-${JDK_VERSION}-jdk-headless \
      libglu1-mesa libpulse0 libnss3 libxcomposite1 libxcursor1 libxi6 libxtst6 libasound2t64 \
      pkg-config build-essential \
    && apt-get clean

# Rust, pinned.
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain ${RUST_VERSION} \
    && rustup component add rustfmt clippy

# Android command line tools, platform tools, emulator, system image.
RUN mkdir -p ${ANDROID_HOME}/cmdline-tools \
    && curl -fsSL -o /tmp/cmdline-tools.zip \
       https://dl.google.com/android/repository/commandlinetools-linux-${CMDLINE_TOOLS_VERSION}_latest.zip \
    && unzip -q /tmp/cmdline-tools.zip -d ${ANDROID_HOME}/cmdline-tools \
    && mv ${ANDROID_HOME}/cmdline-tools/cmdline-tools ${ANDROID_HOME}/cmdline-tools/latest \
    && yes | sdkmanager --licenses > /dev/null \
    && sdkmanager "platform-tools" "emulator" "platforms;${ANDROID_PLATFORM}" \
       "build-tools;${BUILD_TOOLS}" "${SYSTEM_IMAGE}"

# Node and Appium with the UiAutomator2 driver, pinned.
RUN curl -fsSL https://deb.nodesource.com/setup_${NODE_MAJOR}.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && apt-get clean \
    && npm install -g appium@${APPIUM_VERSION} \
    && appium driver install uiautomator2@${UIAUTOMATOR2_VERSION}

# Flutter, pinned, for building the app under test.
RUN git clone --depth 1 --branch ${FLUTTER_VERSION} https://github.com/flutter/flutter.git ${FLUTTER_HOME} \
    && flutter --version && flutter precache --android

# Docker CLI only; the daemon is the host's, reached through the socket.
RUN curl -fsSL -o /tmp/docker.tgz https://download.docker.com/linux/static/stable/x86_64/docker-${DOCKER_CLI_VERSION}.tgz \
    && tar -xzf /tmp/docker.tgz -C /tmp \
    && mv /tmp/docker/docker /usr/local/bin/docker

WORKDIR /work
CMD ["bash"]
