# The Linux build host of scripts/release/local-release.sh: the oldest Linux the shell supports
# (Ubuntu 22.04, glibc 2.35, WebKitGTK 4.1; ADR 0001), as a container. Built once per architecture
# (`--platform linux/arm64` natively on Apple silicon, `linux/amd64` emulated). The build context is
# a directory holding only this file and the repository's rust-toolchain.toml, so the image carries
# the exact toolchain every build uses. `local-release.sh build --refresh-image` rebuilds it from a
# fresh ubuntu:22.04 with the current package versions, as a fresh install would have them.
FROM ubuntu:22.04

ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive

# The build and smoke-test packages, plus a C toolchain, curl,
# python3 (the fake worker), binutils (readelf, objdump) and procps (pgrep).
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl build-essential pkg-config binutils procps python3 \
      libwebkit2gtk-4.1-dev libgtk-3-dev xvfb xauth dbus \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
COPY rust-toolchain.toml /opt/toolchain/
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal --default-toolchain none \
 && cd /opt/toolchain \
 && rustup toolchain install --no-self-update \
 && case "$TARGETARCH" in amd64) arch=x86_64 ;; arm64) arch=aarch64 ;; *) exit 1 ;; esac \
 && rustup target add "$arch-unknown-linux-musl" \
 && rustc --version

# Everything runs as an ordinary user. As root, WebKit's bubblewrap sandbox
# would need CAP_SYS_ADMIN (Docker drops it); as a user it takes user namespaces instead, which
# local-release.sh allows with seccomp=unconfined. A passwd entry, because dbus-daemon needs one.
# /target and the registry exist here, owned by the user, so new named volumes inherit that.
RUN useradd -m -u 1000 builder \
 && mkdir -p /target /opt/cargo/registry \
 && chown -R builder:builder /opt/cargo /opt/rustup /target
USER builder
