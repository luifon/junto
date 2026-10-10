"""Voice vectors from the speaker-embedding model, on the CPU."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import sherpa_onnx
from numpy.typing import NDArray

from junto_transcribe.clips import SAMPLE_RATE
from junto_transcribe.engines import Samples


class SherpaEmbedder:
    def __init__(self, model_path: Path, threads: int) -> None:
        config = sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=str(model_path), num_threads=threads, provider="cpu")
        if not config.validate():
            raise RuntimeError("the speaker-embedding model configuration is not valid")
        self._extractor = sherpa_onnx.SpeakerEmbeddingExtractor(config)

    def embed(self, samples: Samples) -> NDArray[np.float32] | None:
        stream = self._extractor.create_stream()
        stream.accept_waveform(SAMPLE_RATE, samples)
        stream.input_finished()
        if not self._extractor.is_ready(stream):
            return None
        return np.asarray(self._extractor.compute(stream), dtype=np.float32)
