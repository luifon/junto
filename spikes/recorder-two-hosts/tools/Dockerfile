FROM docker:29-cli AS dockercli

FROM rust:1-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends \
      libssl-dev libx11-dev libxext-dev libgl1-mesa-dev libglib2.0-dev libva-dev libdrm-dev \
      pkg-config cmake protobuf-compiler \
      python3 python3-numpy ffmpeg curl ca-certificates netcat-openbsd procps \
    && curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key -o /etc/apt/trusted.gpg.d/apt.llvm.org.asc \
    && echo "deb http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-21 main" > /etc/apt/sources.list.d/llvm.list \
    && apt-get update && apt-get install -y --no-install-recommends clang-21 lld-21 \
    && rm -rf /var/lib/apt/lists/*
ENV CC=clang-21 CXX=clang++-21
COPY --from=dockercli /usr/local/bin/docker /usr/local/bin/docker
