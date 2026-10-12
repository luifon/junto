FROM rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730

RUN apt-get update && apt-get install -y --no-install-recommends \
    curl=7.88.1-10+deb12u15 \
    ca-certificates=20250419~deb12u1 \
    gnupg=2.2.40-1.1+deb12u2 \
    && curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key -o /tmp/junto-linux1-llvm.key \
    && echo '8b2a587ffd672c4687e7581dad4b2f6c1bb2ad6b480cd9771ba2ff48e0b8c75d  /tmp/junto-linux1-llvm.key' | sha256sum -c - \
    && gpg --dearmor -o /usr/share/keyrings/llvm.gpg /tmp/junto-linux1-llvm.key \
    && echo 'deb [signed-by=/usr/share/keyrings/llvm.gpg] https://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-21 main' > /etc/apt/sources.list.d/llvm.list \
    && rm /tmp/junto-linux1-llvm.key

ARG LLVM_VERSION=1:21.1.5~++20251023083151+45afac62e373-1~exp1~20251023083333.51
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang-21=${LLVM_VERSION} libclang-21-dev=${LLVM_VERSION} lld-21=${LLVM_VERSION} \
    pkg-config=1.8.1-1 cmake=3.25.1-1 \
    libfontconfig1-dev=2.14.1-4 libxcb1-dev=1.15-1 \
    libxkbcommon-dev=1.5.0-1 libxkbcommon-x11-dev=1.5.0-1 \
    libasound2-dev=1.2.8-1+b1 libglib2.0-dev=2.74.6-2+deb12u9 \
    libpipewire-0.3-dev=0.3.65-3+deb12u1 libspa-0.2-dev=0.3.65-3+deb12u1 \
    libva-dev=2.17.0-1 libdrm-dev=2.4.114-1+b1 libgbm-dev=22.3.6-1+deb12u2 \
    libx11-dev=2:1.8.4-2+deb12u2 libxrandr-dev=2:1.5.2-2+b1 \
    libxcomposite-dev=1:0.4.5-1 libxdamage-dev=1:1.1.6-1 libxfixes-dev=1:6.0.0-2 \
    libxtst-dev=2:1.2.3-1.1 \
    xvfb=2:21.1.7-3+deb12u13 xauth=1:1.1.2-1 x11-utils=7.7+5 xdotool=1:3.20160805.1-5 \
    dbus-x11=1.14.10-1~deb12u1 \
    pipewire=0.3.65-3+deb12u1 pipewire-bin=0.3.65-3+deb12u1 \
    pipewire-pulse=0.3.65-3+deb12u1 wireplumber=0.4.13-1 pulseaudio-utils=16.1+dfsg1-2+b1 \
    mesa-vulkan-drivers=22.3.6-1+deb12u2 libegl1=1.6.0-1 libgl1-mesa-dri=22.3.6-1+deb12u2 \
    sway=1.7-6 grim=1.4.0+ds-2 fonts-noto-color-emoji=2.042-0+deb12u1 \
    python3=3.11.2-1+b1 python3-pil=9.4.0-1.1+deb12u1 \
    postgresql-client=15+248+deb12u1 procps=2:4.0.2-3 jq=1.6-2.1+deb12u2 \
    && rm -rf /var/lib/apt/lists

RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg=7:5.1.9-0+deb12u1 gdb=13.1-3 openbox=3.6.1-10 \
    && rm -rf /var/lib/apt/lists
RUN rustup component add clippy rustfmt
ENV CC=clang-21 CXX=clang++-21 LIBCLANG_PATH=/usr/lib/llvm-21/lib
ENV CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/target
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=clang-21
ENV RUSTFLAGS="-C link-arg=-fuse-ld=lld-21"
COPY session.sh docker-control.py /opt/junto-linux/
COPY docker /usr/local/bin/docker
RUN useradd --create-home --uid 1000 build \
    && mkdir -p /target /usr/local/cargo/registry \
    && chown build:build /target /usr/local/cargo/registry
USER build
WORKDIR /src
