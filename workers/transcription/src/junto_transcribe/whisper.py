"""Whisper on CTranslate2: each speech clip is transcribed on its own, with its own language."""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import Any

from faster_whisper import BatchedInferencePipeline, WhisperModel

from junto_transcribe.clips import SAMPLE_RATE, Span
from junto_transcribe.engines import EngineFatalError, Recognition, Samples, Word
from junto_transcribe.glossary import counted_text

BEAM_SIZE = 5
_FATAL_MARKERS = ("CUDA", "cuDNN", "cuBLAS", "out of memory")


class _ClipPipeline(BatchedInferencePipeline):  # type: ignore[misc]
    """Records the language that the model detected for each clip."""

    def __init__(self, model: WhisperModel) -> None:
        super().__init__(model)
        self.detected: list[str] = []

    def generate_segment_batched(self, features: Any, tokenizer: Any, options: Any) -> Any:
        encoder_output, output = super().generate_segment_batched(features, tokenizer, options)
        if options.multilingual:
            for candidates in self.model.model.detect_language(encoder_output):
                self.detected.append(candidates[0][0][2:-2])
        return encoder_output, output


class WhisperRecognizer:
    def __init__(self, model_dir: Path, device: str, compute_type: str, threads: int, batch_size: int) -> None:
        try:
            self._model = WhisperModel(
                str(model_dir),
                device=device,
                compute_type=compute_type,
                cpu_threads=threads,
                local_files_only=True,
            )
        except (RuntimeError, ValueError, OSError) as error:
            # The text of the library's error can contain a path, so only its type is passed on.
            raise EngineFatalError(f"whisper_load_failed on {device} ({type(error).__name__})") from error
        self._batch_size = batch_size
        self._languages = frozenset(self._model.supported_languages)
        special = self._model.hf_tokenizer.get_added_tokens_decoder()
        self._control_ids = frozenset(special)
        self._control_texts = tuple(str(token.content) for token in special.values())

    @property
    def languages(self) -> frozenset[str]:
        return self._languages

    def count_tokens(self, text: str) -> int:
        return len(self._model.hf_tokenizer.encode(text, add_special_tokens=False).ids)

    @property
    def control_tokens(self) -> tuple[str, ...]:
        """The written forms of the special tokens of the loaded model."""
        return self._control_texts

    def control_token_in(self, text: str) -> bool:
        if any(form in text for form in self._control_texts):
            return True
        encoded = self._model.hf_tokenizer.encode(counted_text(text), add_special_tokens=False).ids
        return any(token in self._control_ids for token in encoded)

    def recognize(
        self, samples: Samples, clips: Sequence[Span], language: str | None, glossary: str | None
    ) -> list[Recognition | None]:
        if not clips:
            return []
        # A new pipeline for every call: the library keeps the time of the last speech in the pipeline
        # and resets it only when a call ends without an error.
        pipeline = _ClipPipeline(self._model)
        try:
            segments, _ = pipeline.transcribe(
                samples,
                language=language,
                multilingual=language is None,
                clip_timestamps=[{"start": clip.start / SAMPLE_RATE, "end": clip.end / SAMPLE_RATE} for clip in clips],
                vad_filter=False,
                batch_size=self._batch_size,
                beam_size=BEAM_SIZE,
                temperature=0.0,
                hotwords=glossary,
                without_timestamps=True,
                word_timestamps=True,
            )
            found = list(segments)
        except RuntimeError as error:
            fatal = [marker for marker in _FATAL_MARKERS if marker in str(error)]
            if fatal:
                raise EngineFatalError(f"Whisper failed on the device ({', '.join(fatal)})") from error
            raise
        if len(found) != len(clips):
            raise RuntimeError(f"Whisper returned {len(found)} results for {len(clips)} clips")
        languages = pipeline.detected if language is None else [language] * len(clips)
        if len(languages) != len(clips):
            raise RuntimeError(f"Whisper detected {len(languages)} languages for {len(clips)} clips")

        results: list[Recognition | None] = []
        for clip, segment, clip_language in zip(clips, found, languages, strict=True):
            text = segment.text.strip()
            if not text:
                results.append(None)
                continue
            words = tuple(
                Word(_inside(clip, word.start), _inside(clip, word.end), word.word)
                for word in segment.words or ()
                if word.word
            )
            results.append(
                Recognition(
                    start=_inside(clip, segment.start),
                    end=_inside(clip, segment.end),
                    text=text,
                    language=clip_language,
                    words=words or None,
                )
            )
        return results


def _inside(clip: Span, seconds: float) -> int:
    return min(max(round(seconds * SAMPLE_RATE), clip.start), clip.end)
