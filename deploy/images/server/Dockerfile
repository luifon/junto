# syntax=docker/dockerfile:1.28.0@sha256:bb22d9815c728170f72750f4e5b0d672e06176142e1d602c7e66c050100b7e5b
# The Junto server (ADR 0014). Build from the repository root:
#   docker build -f deploy/images/server/Dockerfile -t junto-server .
# The last stage is the server image.

ARG RUST_IMAGE=rust:1.98.1-slim-trixie@sha256:4cd829461bd5c4d511c32e269da9cb8929223b666519d8004e35fc8d1d771ab7
ARG NODE_IMAGE=node:22.23.3-trixie-slim@sha256:154ba2f4d6fec323d28e4f4bb86bba4677f1223391a1979cf521304e03a98dfa
ARG DEBIAN_IMAGE=debian:trixie-20261005-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f

# The server binary, from Cargo.lock.
FROM ${RUST_IMAGE} AS server-build
# Optional: the number of parallel compile jobs, for a build host with little memory.
ARG CARGO_BUILD_JOBS
ENV CARGO_PROFILE_RELEASE_STRIP=symbols \
    CARGO_TERM_COLOR=never
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --locked --release -p junto-server --bin junto-server \
 && install -D -m 0755 target/release/junto-server /out/junto-server

# The recording layout page (ADR 0609), from package-lock.json. No install script runs.
FROM ${NODE_IMAGE} AS layout-build
WORKDIR /src/recording-layout
COPY recording-layout/package.json recording-layout/package-lock.json ./
RUN npm ci --ignore-scripts --omit=dev --no-audit --no-fund
COPY recording-layout/build.mjs recording-layout/index.html recording-layout/layout.js \
     recording-layout/messages.js recording-layout/theme.css ./
RUN node build.mjs

# ffmpeg and ffprobe, which the server runs to join and check the files of a recording
# (ADR 0606). An LGPL build of the release that the transcription worker pins, with every
# native component and no external library. The compiler comes from a dated Debian snapshot.
FROM ${DEBIAN_IMAGE} AS ffmpeg-build
ARG DEBIAN_SNAPSHOT=20261005T000000Z
ARG FFMPEG_VERSION=9.0.2
ARG FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
RUN rm -f /etc/apt/sources.list.d/* \
 && echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT} trixie main" > /etc/apt/sources.list \
 && echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT} trixie-security main" >> /etc/apt/sources.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends build-essential pkg-config nasm xz-utils curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /build
RUN curl -fsSL -o ffmpeg.tar.xz "https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz" \
 && echo "${FFMPEG_SHA256}  ffmpeg.tar.xz" | sha256sum -c - \
 && tar -xf ffmpeg.tar.xz --strip-components=1 \
 && ./configure \
      --prefix=/opt/ffmpeg \
      --enable-static --disable-shared \
      --disable-doc --disable-debug --disable-ffplay \
      --disable-autodetect --disable-network --disable-devices \
      --disable-encoders --enable-encoder=pcm_s16le,wrapped_avframe \
      --disable-muxers --enable-muxer=mp4,null,pcm_s16le \
 && make -j"$(nproc)" \
 && make install \
 && install -D -m 0644 COPYING.LGPLv2.1 /opt/ffmpeg/share/licenses/ffmpeg/COPYING.LGPLv2.1 \
 && install -D -m 0644 LICENSE.md /opt/ffmpeg/share/licenses/ffmpeg/LICENSE.md \
 && grep -q "^#define CONFIG_GPL 0" config.h \
 && grep -q "^#define CONFIG_VERSION3 0" config.h \
 && grep -q "^#define CONFIG_NONFREE 0" config.h
# The build fails when a component that the server calls is absent.
RUN set -e; ffmpeg=/opt/ffmpeg/bin/ffmpeg; \
    has() { "$ffmpeg" -hide_banner "-$1" | awk -v name="$2" '$2 == name { found = 1 } END { exit !found }' \
            || { echo "ffmpeg has no $1 entry $2" >&2; exit 1; }; }; \
    for name in signalstats silencedetect volumedetect scale metadata aresample; do has filters "$name"; done; \
    for name in h264 aac opus; do has decoders "$name"; done; \
    for name in mp4 null s16le; do has muxers "$name"; done; \
    for name in mpegts ogg mov,mp4,m4a,3gp,3g2,mj2; do has demuxers "$name"; done; \
    /opt/ffmpeg/bin/ffprobe -hide_banner -version > /dev/null

FROM ${DEBIAN_IMAGE} AS server
# The recorder image runs as the user 1001, and the server writes into the directories that
# the recorder creates (ADR 0014).
ARG JUNTO_UID=1001
ARG JUNTO_GID=1001
LABEL org.opencontainers.image.title="junto-server" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.description="The Junto server with ffmpeg, ffprobe and the recording layout page."
RUN groupadd --gid "${JUNTO_GID}" junto \
 && useradd --uid "${JUNTO_UID}" --gid "${JUNTO_GID}" --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin junto \
 && install -d -m 0755 /var/lib/junto \
 && install -d -o junto -g junto -m 0750 /var/lib/junto/attachments /var/lib/junto/recordings /var/lib/junto/spool \
      /var/lib/junto/exports
COPY --from=ffmpeg-build /opt/ffmpeg/bin/ffmpeg /opt/ffmpeg/bin/ffprobe /usr/local/bin/
COPY --from=ffmpeg-build /opt/ffmpeg/share/licenses /usr/share/licenses
COPY --from=layout-build /src/recording-layout/dist /usr/share/junto/recording-layout
COPY --from=server-build /out/junto-server /usr/local/bin/junto-server
COPY --chmod=0755 deploy/images/server/junto-transcribe-handover deploy/images/server/junto-volumes \
     /usr/local/bin/
ENV JUNTO_LISTEN_ADDRESS=0.0.0.0:8080 \
    JUNTO_RECORDER_LAYOUT_DIR=/usr/share/junto/recording-layout
USER junto
WORKDIR /var/lib/junto
EXPOSE 8080
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=5s --start-period=60s --retries=3 CMD ["junto-server", "health"]
ENTRYPOINT ["junto-server"]
