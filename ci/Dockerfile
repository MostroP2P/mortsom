# Mortsom runner image (spec Phase 6.1): every tool the coordinator shells
# out to, at pinned versions. Linux clients need a virtual display (Xvfb)
# and AT-SPI; Web clients need Chromium and ChromeDriver; the stack needs the
# Docker socket (`-v /var/run/docker.sock:/var/run/docker.sock`).
#
# Nothing in this image is Mortsom logic; it is the host contract that
# `mortsom doctor` verifies.

FROM ubuntu:24.04

ARG RUST_VERSION=1.94.0
ARG FLUTTER_VERSION=3.41.9
ARG DOCKER_CLI_VERSION=28.3.3

ENV DEBIAN_FRONTEND=noninteractive \
    FLUTTER_HOME=/opt/flutter \
    CARGO_HOME=/opt/cargo \
    RUSTUP_HOME=/opt/rustup \
    PATH=/opt/cargo/bin:/opt/flutter/bin:/usr/local/bin:/usr/bin:/bin

RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl git unzip xz-utils zip \
      pkg-config build-essential clang cmake ninja-build protobuf-compiler \
      libgtk-3-dev libsecret-1-dev \
      xvfb dbus-daemon gnome-keyring at-spi2-core imagemagick \
      chromium chromium-driver python3 \
    && apt-get clean

# Rust, pinned.
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain ${RUST_VERSION} \
    && rustup component add rustfmt clippy \
    && rustup target add wasm32-unknown-unknown \
    && cargo install wasm-pack

# Flutter, pinned, for building the app under test on Linux and Web.
RUN git clone --depth 1 --branch ${FLUTTER_VERSION} https://github.com/flutter/flutter.git ${FLUTTER_HOME} \
    && flutter --version && flutter precache --linux --web

# Docker CLI only; the daemon is the host's, reached through the socket.
RUN curl -fsSL -o /tmp/docker.tgz https://download.docker.com/linux/static/stable/x86_64/docker-${DOCKER_CLI_VERSION}.tgz \
    && tar -xzf /tmp/docker.tgz -C /tmp \
    && mv /tmp/docker/docker /usr/local/bin/docker

WORKDIR /work
CMD ["bash"]
