# Text-to-speech image for the model tests. A test tool only: nothing in it is part of the worker.
# Piper 1.2.0 is MIT; it links espeak-ng (GPL-3.0). Later Piper versions are GPL-3.0 and are not used.
# The base image is named by digest and every Python package by its version. The Debian
# packages come from a dated snapshot of the Debian archive, the one of the server image.
FROM python:3.11-slim-bookworm@sha256:0a310eeecf4e1f5a0743f9a6520c90c88d089c903ca5fd283f501e3a805f5f89
ARG DEBIAN_SNAPSHOT=20261005T000000Z
RUN rm -f /etc/apt/sources.list.d/* \
 && echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT} bookworm main" > /etc/apt/sources.list \
 && echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT} bookworm-security main" >> /etc/apt/sources.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir piper-tts==1.2.0 onnxruntime==1.28.0 numpy==2.4.6 soundfile==0.14.0 \
    piper-phonemize==1.1.0 cffi==2.1.1 flatbuffers==25.12.19 packaging==26.3 protobuf==7.36.2 \
    pycparser==3.11 typing_extensions==4.16.0
ENV ORT_DISABLE_TELEMETRY=1
ENV PYTHONUNBUFFERED=1
RUN useradd --uid 10001 --create-home speech
USER speech
WORKDIR /work
