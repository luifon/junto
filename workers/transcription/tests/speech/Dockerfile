# Text-to-speech image for the model tests. A test tool only: nothing in it is part of the worker.
# Piper 1.2.0 is MIT; it links espeak-ng (GPL-3.0). Later Piper versions are GPL-3.0 and are not used.
FROM python:3.11-slim-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends ffmpeg ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir piper-tts==1.2.0 onnxruntime==1.28.0 numpy soundfile
ENV ORT_DISABLE_TELEMETRY=1
ENV PYTHONUNBUFFERED=1
RUN useradd --uid 10001 --create-home speech
USER speech
WORKDIR /work
