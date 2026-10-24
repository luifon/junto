# syntax=docker/dockerfile:1.28.0@sha256:bb22d9815c728170f72750f4e5b0d672e06176142e1d602c7e66c050100b7e5b
# Transcription worker of Junto. Build from workers/transcription:
#   docker build -f docker/Dockerfile --build-arg VARIANT=cpu -t junto-transcription-worker:cpu .
#   docker build -f docker/Dockerfile --build-arg VARIANT=gpu -t junto-transcription-worker:gpu .
# The last stage is the worker, so a build without --target gives the worker image.
# The stage "test" adds the test tools and is built only with --target test.
# Run with --network none. See README.md.

ARG PYTHON_IMAGE=python:3.12-slim-bookworm@sha256:34386ef0cb081344d7ec1c103ba398e6e9f64e9ab3a1509accc92a4e24a07258
ARG UV_IMAGE=ghcr.io/astral-sh/uv:0.11.13@sha256:841c8e6fe30a8b07b4478d12d0c608cba6de66102d29d65d1cc423af86051563

FROM ${UV_IMAGE} AS uv

FROM ${PYTHON_IMAGE} AS toolchain
ARG DEBIAN_SNAPSHOT=20261005T000000Z
RUN rm -f /etc/apt/sources.list.d/* \
 && echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT} bookworm main" > /etc/apt/sources.list \
 && echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT} bookworm-security main" >> /etc/apt/sources.list \
 && printf 'Package: *\nPin: origin snapshot.debian.org\nPin-Priority: 1001\n' > /etc/apt/preferences.d/junto-snapshot \
 && apt-get update \
 && apt-get install -y --no-install-recommends --allow-downgrades build-essential pkg-config nasm xz-utils curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# FFmpeg under LGPL-2.1-or-later: no GPL or version-3 part, no external codec library,
# and only the audio decoders and containers that the worker reads.
FROM toolchain AS ffmpeg
ARG FFMPEG_VERSION=9.0.2
ARG FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
WORKDIR /build
RUN curl -fsSL -o ffmpeg.tar.xz "https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz" \
 && echo "${FFMPEG_SHA256}  ffmpeg.tar.xz" | sha256sum -c - \
 && tar -xf ffmpeg.tar.xz --strip-components=1 \
 && ./configure \
      --prefix=/opt/ffmpeg \
      --enable-shared --disable-static \
      --disable-programs --disable-doc --disable-debug \
      --disable-autodetect --disable-network \
      --disable-everything \
      --enable-decoder=opus,vorbis,flac,pcm_s16le,pcm_s24le,pcm_s32le,pcm_f32le,pcm_u8 \
      --enable-parser=opus,vorbis,flac \
      --enable-demuxer=ogg,wav,flac,matroska \
      --enable-protocol=file \
      --enable-filter=abuffer,abuffersink,aformat,aresample,anull \
 && make -j"$(nproc)" \
 && make install \
 && install -D -m 0644 COPYING.LGPLv2.1 /opt/ffmpeg/share/licenses/ffmpeg/COPYING.LGPLv2.1 \
 && install -D -m 0644 LICENSE.md /opt/ffmpeg/share/licenses/ffmpeg/LICENSE.md \
 && /bin/sh -c 'grep -q "^#define CONFIG_GPL 0" config.h && grep -q "^#define CONFIG_VERSION3 0" config.h && grep -q "^#define CONFIG_NONFREE 0" config.h'

# Python packages from the lock file, with hashes. PyAV is compiled against the FFmpeg above,
# because its binary package contains GPL libraries.
FROM toolchain AS build
ARG VARIANT=cpu
COPY --from=uv /uv /usr/local/bin/uv
COPY --from=ffmpeg /opt/ffmpeg /opt/ffmpeg
ENV PKG_CONFIG_PATH=/opt/ffmpeg/lib/pkgconfig \
    LD_LIBRARY_PATH=/opt/ffmpeg/lib \
    UV_LINK_MODE=copy \
    UV_COMPILE_BYTECODE=1 \
    UV_PYTHON_DOWNLOADS=never \
    UV_NO_CACHE=1 \
    UV_PROJECT_ENVIRONMENT=/app/.venv
WORKDIR /app
COPY pyproject.toml uv.lock README.md ./
RUN case "$VARIANT" in cpu) extra="" ;; gpu) extra="--extra gpu" ;; *) echo "VARIANT must be cpu or gpu" >&2; exit 1 ;; esac \
 && uv sync --frozen --no-dev --no-install-project --no-binary-package av $extra
COPY src ./src
RUN case "$VARIANT" in gpu) extra="--extra gpu" ;; *) extra="" ;; esac \
 && uv sync --frozen --no-dev --no-editable --no-binary-package av $extra

FROM build AS build-test
ARG VARIANT=cpu
RUN case "$VARIANT" in gpu) extra="--extra gpu" ;; *) extra="" ;; esac \
 && uv sync --frozen --no-editable --no-binary-package av $extra

FROM ${PYTHON_IMAGE} AS runtime
ARG VARIANT=cpu
LABEL org.opencontainers.image.title="junto-transcription-worker" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.description="Transcription worker of Junto (${VARIANT}). Runs without network access."
COPY --from=ffmpeg /opt/ffmpeg/lib /opt/ffmpeg/lib
COPY --from=ffmpeg /opt/ffmpeg/share/licenses /opt/ffmpeg/share/licenses
COPY --from=build /app/.venv /app/.venv
COPY THIRD-PARTY.md /app/THIRD-PARTY.md
RUN useradd --uid 10001 --user-group --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin junto
ENV PATH=/app/.venv/bin:$PATH \
    LD_LIBRARY_PATH=/opt/ffmpeg/lib:/app/.venv/lib/python3.12/site-packages/nvidia/cublas/lib:/app/.venv/lib/python3.12/site-packages/nvidia/cudnn/lib \
    ORT_DISABLE_TELEMETRY=1 \
    HF_HUB_DISABLE_TELEMETRY=1 \
    HF_HUB_OFFLINE=1 \
    DO_NOT_TRACK=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    JUNTO_TRANSCRIBE_MODELS=/models
USER junto
WORKDIR /work
ENTRYPOINT ["junto-transcribe"]

# The worker image with the test tools and the tests. Not for deployment.
FROM runtime AS test
COPY --from=build-test /app/.venv /app/.venv
COPY pyproject.toml README.md /app/
COPY docker /app/docker
COPY tests /app/tests
WORKDIR /app
ENTRYPOINT ["python", "-m", "pytest", "-p", "no:cacheprovider"]

# The image for deployment. It must stay the last stage.
FROM runtime AS worker
