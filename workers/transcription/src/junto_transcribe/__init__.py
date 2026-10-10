"""Transcription worker of Junto."""

from junto_transcribe.runtime import apply_telemetry_switches

__version__ = "0.1.0"

# Before any module of this package imports a model library.
apply_telemetry_switches()
