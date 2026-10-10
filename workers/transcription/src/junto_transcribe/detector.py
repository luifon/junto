"""Silero voice detector: one speech probability per 512-sample frame."""

from __future__ import annotations

from pathlib import Path

import numpy as np
from faster_whisper.vad import SileroVADModel

from junto_transcribe.clips import FRAME_SAMPLES
from junto_transcribe.engines import Samples

_CONTEXT_SAMPLES = 64
_STATE_SIZE = 128
_FRAMES_PER_RUN = 10_000


class _Stream:
    """The model's input and state for one audio stream, as `SileroVADModel.__call__` builds them
    for a whole array: each frame is preceded by the last 64 samples of the frame before it, and
    the recurrent state goes from one run to the next."""

    def __init__(self, session: object) -> None:
        self._session = session
        self._h = np.zeros((1, 1, _STATE_SIZE), dtype=np.float32)
        self._c = np.zeros((1, 1, _STATE_SIZE), dtype=np.float32)
        self._context = np.zeros(_CONTEXT_SAMPLES, dtype=np.float32)

    def probabilities(self, samples: Samples, last: bool) -> list[float]:
        if last:
            samples = np.pad(samples, (0, FRAME_SAMPLES - len(samples) % FRAME_SAMPLES))
        elif len(samples) % FRAME_SAMPLES:
            raise ValueError("a piece that is not the last one must have whole frames")
        if not len(samples):
            return []
        frames = samples.reshape(-1, FRAME_SAMPLES)
        contexts = np.concatenate((self._context[np.newaxis], frames[:-1, -_CONTEXT_SAMPLES:]))
        inputs = np.concatenate((contexts, frames), axis=1).astype(np.float32, copy=False)
        self._context = frames[-1, -_CONTEXT_SAMPLES:].copy()
        outputs = []
        for start in range(0, len(inputs), _FRAMES_PER_RUN):
            output, self._h, self._c = self._session.run(  # type: ignore[attr-defined]
                None, {"input": inputs[start : start + _FRAMES_PER_RUN], "h": self._h, "c": self._c}
            )
            outputs.append(output)
        result: list[float] = np.concatenate(outputs, axis=0).reshape(-1).tolist()
        return result


class SileroDetector:
    def __init__(self, model_path: Path) -> None:
        self._model = SileroVADModel(str(model_path))

    def stream(self) -> _Stream:
        return _Stream(self._model.session)
