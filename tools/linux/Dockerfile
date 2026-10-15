FROM rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730

# Every package comes from one dated snapshot of the Debian archives. A snapshot keeps every
# version of a package; the archives themselves remove a version when an update replaces
# it, and a build with exact versions then fails.
RUN rm -f /etc/apt/sources.list.d/debian.sources \
    && printf 'deb [check-valid-until=no] http://snapshot.debian.org/archive/%s %s main\n' \
        debian/20261010T000000Z bookworm \
        debian/20261010T000000Z bookworm-updates \
        debian/20261010T000000Z bookworm-backports \
        debian-security/20261010T000000Z bookworm-security \
        > /etc/apt/sources.list \
    && printf 'Acquire::Retries "5";\n' > /etc/apt/apt.conf.d/80-retries
# git: cargo deny fetches its advisory database with the git program.
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl=7.88.1-10+deb12u15 \
    ca-certificates=20250419~deb12u1 \
    git=1:2.39.5-0+deb12u3 \
    && rm -rf /var/lib/apt/lists

# The call library needs clang 21 or later. The compiler is Debian's build for this release,
# one version for both architectures.
RUN apt-get update && apt-get install -y --no-install-recommends \
    clang-22=1:22.1.8-1~deb12u1 lld-22=1:22.1.8-1~deb12u1 libclang-22-dev=1:22.1.8-1~deb12u1 \
    llvm-22=1:22.1.8-1~deb12u1 libclang-common-22-dev=1:22.1.8-1~deb12u1 libclang-cpp22=1:22.1.8-1~deb12u1 \
    libclang1-22=1:22.1.8-1~deb12u1 libllvm22=1:22.1.8-1~deb12u1 llvm-22-linker-tools=1:22.1.8-1~deb12u1 \
    llvm-22-runtime=1:22.1.8-1~deb12u1 \
    && rm -rf /var/lib/apt/lists
RUN apt-get update && apt-get install -y --no-install-recommends \
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
    xserver-xorg-core=2:21.1.7-3+deb12u13 xserver-xorg-video-dummy=1:0.4.0-1 \
    xdg-desktop-portal=1.16.0-2 swaybg=1.2.0-1 \
    && rm -rf /var/lib/apt/lists
# The Mesa 22 software Vulkan driver stays beside Mesa 25 for the old-driver window check.
RUN mkdir -p /opt/junto-linux/mesa22 \
    && cp /usr/lib/*-linux-gnu/libvulkan_lvp.so /opt/junto-linux/mesa22/ \
    && sed 's|"library_path": *"[^"]*"|"library_path": "/opt/junto-linux/mesa22/libvulkan_lvp.so"|' \
        /usr/share/vulkan/icd.d/lvp_icd.*.json > /opt/junto-linux/mesa22/lvp_icd.json \
    && grep -q /opt/junto-linux/mesa22/libvulkan_lvp.so /opt/junto-linux/mesa22/lvp_icd.json
RUN apt-get update && apt-get install -y --no-install-recommends -t bookworm-backports \
    mesa-vulkan-drivers=25.0.7-2~bpo12+1 libgl1-mesa-dri=25.0.7-2~bpo12+1 \
    libegl-mesa0=25.0.7-2~bpo12+1 libglx-mesa0=25.0.7-2~bpo12+1 \
    libgbm-dev=25.0.7-2~bpo12+1 libdrm-dev=2.4.123-1~bpo12+1 \
    && rm -rf /var/lib/apt/lists
RUN rustup component add clippy rustfmt
RUN apt-get update && apt-get install -y --no-install-recommends \
    meson=1.0.1-5 ninja-build=1.11.1-2~deb12u1 libinih-dev=55-1 \
    libsystemd-dev=252.39-1~deb12u2 libwayland-dev=1.21.0-1 \
    wayland-protocols=1.31-1 \
    && rm -rf /var/lib/apt/lists
COPY install-portal.sh /opt/junto-linux/
RUN sh /opt/junto-linux/install-portal.sh
# The AppImage of the Linux packages (ADR 0754): the SquashFS tools of the snapshot, and the
# type 2 runtime of the AppImage project from one dated release, checked by its SHA-256.
RUN apt-get update && apt-get install -y --no-install-recommends \
    squashfs-tools=1:4.5.1-1 \
    && rm -rf /var/lib/apt/lists
RUN case "$(uname -m)" in \
        x86_64) sum=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;; \
        aarch64) sum=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;; \
        *) echo 'no AppImage runtime for this architecture' >&2; exit 1 ;; \
    esac \
    && curl -fsSL --retry 5 -o /opt/junto-linux/appimage-runtime \
        "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$(uname -m)" \
    && echo "$sum  /opt/junto-linux/appimage-runtime" | sha256sum -c - \
    && chmod 644 /opt/junto-linux/appimage-runtime
ENV CC=clang-22 CXX=clang++-22 LIBCLANG_PATH=/usr/lib/llvm-22/lib
ENV CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/target
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=clang-22
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang-22
ENV RUSTFLAGS="-C link-arg=-fuse-ld=lld-22"
COPY session.sh run-as-user.sh docker-control.py /opt/junto-linux/
COPY docker /usr/local/bin/docker
ARG BUILD_UID=1000
ARG BUILD_GID=1000
RUN if ! getent group "$BUILD_GID" >/dev/null; then groupadd --gid "$BUILD_GID" build; fi \
    && if [ "$BUILD_UID" -ne 0 ]; then useradd --create-home --uid "$BUILD_UID" --gid "$BUILD_GID" build; fi \
    && mkdir -p /target /usr/local/cargo/registry \
    && chown "$BUILD_UID:$BUILD_GID" /target /usr/local/cargo/registry
USER ${BUILD_UID}:${BUILD_GID}
WORKDIR /src
