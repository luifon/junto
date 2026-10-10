"""One job: every segment through voice detection, Whisper and speaker separation, on one timeline."""

from __future__ import annotations

import time
from collections import defaultdict
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, field
from typing import BinaryIO, Literal

import numpy as np

from junto_transcribe.clips import (
    FRAME_SAMPLES,
    Clip,
    ClipBuilder,
    ClipRules,
    Span,
    SpanTracker,
    ms_from_samples,
    samples_from_ms,
)
from junto_transcribe.contract import Job, JobError, Segment
from junto_transcribe.engines import (
    AudioDamagedError,
    AudioDecodeError,
    AudioFiles,
    AudioMissingError,
    AudioReader,
    AudioRefusedError,
    EngineFatalError,
    Recognizer,
    Samples,
    SpeakerEmbedder,
    SpeechDetector,
)
from junto_transcribe.glossary import TOKEN_LIMIT, GlossaryPlan, plan_glossary
from junto_transcribe.limits import Limits
from junto_transcribe.speakers import (
    SpeakerRules,
    Window,
    cluster_speakers,
    speaker_of,
    speaker_turns,
    windows_for_span,
)

SegmentStatus = Literal["ok", "empty", "damaged", "failed"]
SpeakerStatus = Literal["not_applicable", "not_requested", "separated", "unassigned", "failed"]

# Fixed texts: a reason never contains a value of the job or the text of an exception.
REASONS = {
    "file_missing": "the audio file does not exist",
    "not_a_regular_file": "the path is not a regular file inside the audio root, or a part of it is a link",
    "decode_failed": "nothing could be decoded from the audio file",
    "not_one_audio_stream": "the file does not have exactly one audio stream",
    "timestamp_backward": "a packet timestamp is earlier than the audio before it; the lines cover the audio before it",
    "decode_incomplete": "decoding stopped before the end of the file; the lines cover the audio before that point",
    "timestamp_gap": "a gap between packet timestamps is over the limit; the lines cover the audio before the gap",
    "segment_duration_limit": "the audio is longer than the limit for one segment; the lines cover the audio up to it",
    "job_duration_limit": "the audio of the job is longer than the limit for one job",
    "time_limit": "the processing time limit of the job was reached",
    "processing_failed": "a processing step failed",
}
SPEAKER_REASONS = {
    "segment_failed": "the segment failed",
    "embedding_failed": "the speaker-embedding model failed",
    "clustering_failed": "the clustering of voices failed",
    "no_voice_window": "a line has no speech that is long enough for a voice vector",
    "speaker_window_limit": "the limit of voice windows of the job was reached; later lines have no label",
    "time_limit": "the processing time limit of the job was reached before the speakers were separated",
}

STAGES = ("decode", "voice_detection", "transcription", "speaker_embeddings", "speaker_clustering")


@dataclass(frozen=True)
class Settings:
    clip_rules: ClipRules = field(default_factory=ClipRules)
    speaker_rules: SpeakerRules = field(default_factory=SpeakerRules)
    limits: Limits = field(default_factory=Limits)
    glossary_token_limit: int = TOKEN_LIMIT
    chunk_ms: int = 1_800_000
    clips_per_call: int = 64

    def __post_init__(self) -> None:
        if samples_from_ms(self.chunk_ms) < FRAME_SAMPLES:
            raise ValueError("chunk_ms must cover at least one detector frame")
        if self.clips_per_call < 1:
            raise ValueError("clips_per_call must be at least 1")


@dataclass(frozen=True)
class Engines:
    reader: AudioReader
    detector: SpeechDetector
    recognizer: Recognizer
    embedder: SpeakerEmbedder | None


@dataclass(frozen=True)
class LineWord:
    start_ms: int
    end_ms: int
    text: str


@dataclass
class Line:
    segment_index: int
    order: int
    start_sample: int
    end_sample: int
    start_ms: int
    end_ms: int
    text: str
    language: str
    words: tuple[LineWord, ...] | None
    speaker: str | None = None


@dataclass
class SegmentResult:
    id: str
    status: SegmentStatus = "empty"
    reason_code: str | None = None
    reason_message: str | None = None
    audio_ms: int = 0
    processed_ms: int = 0
    speech_ms: int = 0
    clips: int = 0
    lines: int = 0
    speakers: SpeakerStatus = "not_applicable"
    speakers_reason_code: str | None = None

    @property
    def speakers_reason_message(self) -> str | None:
        return None if self.speakers_reason_code is None else SPEAKER_REASONS[self.speakers_reason_code]


@dataclass(frozen=True)
class NamedSpeaker:
    label: str
    participant_id: str


@dataclass(frozen=True)
class Outcome:
    lines: tuple[Line, ...]
    segments: tuple[SegmentResult, ...]
    glossary: GlossaryPlan
    speakers: tuple[NamedSpeaker, ...]
    stages_ms: dict[str, int]


class _Stopwatch:
    def __init__(self, clock: Callable[[], float]) -> None:
        self._clock = clock
        self.seconds: dict[str, float] = dict.fromkeys(STAGES, 0.0)

    @contextmanager
    def measure(self, stage: str) -> Iterator[None]:
        started = self._clock()
        try:
            yield
        finally:
            self.seconds[stage] += self._clock() - started


@dataclass
class _Run:
    """What one job has used of its limits."""

    clock: Callable[[], float]
    deadline: float
    audio_samples_left: int
    speaker_windows_left: int

    def out_of_time(self) -> bool:
        return self.clock() >= self.deadline


# Silence between the clips of one Whisper call. It never reaches the model: it only makes every
# clip start after a pause for the library's correction of the first word time.
CLIP_GAP_MS = 3000
_CLIP_GAP_SAMPLES = samples_from_ms(CLIP_GAP_MS)


@dataclass
class _SegmentWork:
    lines: list[Line] = field(default_factory=list)
    windows: list[Window] = field(default_factory=list)
    vectors: list[np.ndarray] = field(default_factory=list)
    queue: list[tuple[Clip, Samples]] = field(default_factory=list)
    audio_samples: int = 0
    processed_samples: int = 0
    speech_samples: int = 0
    clips: int = 0
    damage: str | None = None
    out_of_time: bool = False
    speaker_error: bool = False
    unlabelled_from: int | None = None
    unlabelled_reason: str | None = None


def _recognize_queue(
    index: int,
    segment: Segment,
    job: Job,
    glossary: str | None,
    engines: Engines,
    watch: _Stopwatch,
    run: _Run,
    work: _SegmentWork,
) -> bool:
    """Sends the waiting clips to Whisper as one group. Returns false when the time is over."""
    if not work.queue:
        return True
    if run.out_of_time():
        work.damage = work.damage or "time_limit"
        work.out_of_time = True
        return False
    group = work.queue
    work.queue = []
    pieces: list[Samples] = []
    placed: list[Span] = []
    cursor = 0
    for _, audio in group:
        pieces += [audio, np.zeros(_CLIP_GAP_SAMPLES, dtype=np.float32)]
        placed.append(Span(cursor, cursor + len(audio)))
        cursor += len(audio) + _CLIP_GAP_SAMPLES
    with watch.measure("transcription"):
        found = engines.recognizer.recognize(np.concatenate(pieces[:-1]), placed, job.language, glossary)
    for (clip, _), place, result in zip(group, placed, found, strict=True):
        if result is None:
            continue
        shift = clip.padded.start - place.start
        start = result.start + shift
        end = max(start, result.end + shift)
        words = None
        if result.words:
            words = tuple(
                LineWord(
                    segment.start_ms + ms_from_samples(word.start + shift),
                    segment.start_ms + ms_from_samples(max(word.start, word.end) + shift),
                    word.text,
                )
                for word in result.words
            )
        work.lines.append(
            Line(
                segment_index=index,
                order=len(work.lines),
                start_sample=start,
                end_sample=end,
                start_ms=segment.start_ms + ms_from_samples(start),
                end_ms=segment.start_ms + ms_from_samples(end),
                text=result.text,
                language=result.language,
                words=words,
            )
        )
    return True


def _embed_windows(
    index: int,
    number: int,
    clip: Clip,
    audio: Samples,
    audio_start: int,
    engines: Engines,
    settings: Settings,
    run: _Run,
    work: _SegmentWork,
) -> None:
    """Voice vectors of one clip. Stops, and records from where, at the window limit or the deadline."""
    if engines.embedder is None:
        raise RuntimeError("no speaker embedding model is loaded")
    for window in windows_for_span(clip.voice, settings.speaker_rules):
        reason = None
        if run.speaker_windows_left <= 0:
            reason = "speaker_window_limit"
        elif run.out_of_time():
            reason = "time_limit"
        if reason is not None:
            work.unlabelled_from = clip.voice.start
            work.unlabelled_reason = reason
            return
        run.speaker_windows_left -= 1
        vector = engines.embedder.embed(audio[window.start - audio_start : window.end - audio_start])
        if vector is None:
            continue
        work.windows.append(Window(index, number, window.start, window.end))
        work.vectors.append(np.asarray(vector, dtype=np.float32))


def _process_segment(
    index: int,
    segment: Segment,
    source: BinaryIO,
    job: Job,
    glossary: str | None,
    engines: Engines,
    settings: Settings,
    watch: _Stopwatch,
    run: _Run,
    work: _SegmentWork,
) -> None:
    """Reads the audio in blocks. The clips, the groups of clips for Whisper and the voice windows
    are the same for every block length, because the detector state, the open speech and the audio
    that a later clip needs go from one block to the next."""
    rules = settings.clip_rules
    separate = segment.kind == "shared" and job.separate_speakers
    segment_limit = samples_from_ms(settings.limits.segment_audio_ms)
    budget = min(segment_limit, run.audio_samples_left)
    budget_code = "segment_duration_limit" if segment_limit <= run.audio_samples_left else "job_duration_limit"
    block_samples = samples_from_ms(settings.chunk_ms) // FRAME_SAMPLES * FRAME_SAMPLES

    detector = engines.detector.stream()
    tracker = SpanTracker(rules)
    builder = ClipBuilder(rules)
    buffer: Samples = np.zeros(0, dtype=np.float32)
    buffer_start = 0
    position = 0
    clip_number = 0
    stopped = False

    blocks = engines.reader.blocks(source, block_samples, budget, run.out_of_time)
    while not stopped:
        if run.out_of_time():
            work.damage = "time_limit"
            work.out_of_time = True
            break
        try:
            with watch.measure("decode"):
                item = next(blocks, None)
        except AudioDamagedError as error:
            work.damage = budget_code if error.code == "audio_limit" else error.code
            work.out_of_time = error.code == "time_limit"
            break
        if item is None:
            break
        block, last = item
        work.audio_samples += len(block)
        run.audio_samples_left -= len(block)
        buffer = np.concatenate((buffer, block)) if buffer.size else block
        position += len(block)

        with watch.measure("voice_detection"):
            spans = tracker.feed(detector.probabilities(block, last))
            if last:
                spans += tracker.finish(position)
            clips = builder.feed(spans, tracker.open_start, tracker.position)
            if last:
                clips += builder.finish(position)

        for clip in clips:
            audio = buffer[clip.padded.start - buffer_start : clip.padded.end - buffer_start]
            work.speech_samples += clip.speech.length
            work.clips += 1
            work.queue.append((clip, audio.copy()))
            if separate and not work.speaker_error and work.unlabelled_from is None:
                try:
                    with watch.measure("speaker_embeddings"):
                        _embed_windows(index, clip_number, clip, buffer, buffer_start, engines, settings, run, work)
                except (EngineFatalError, MemoryError):
                    raise
                except Exception:
                    work.speaker_error = True
            clip_number += 1
            if len(work.queue) >= settings.clips_per_call and not _recognize_queue(
                index, segment, job, glossary, engines, watch, run, work
            ):
                stopped = True
                break

        keep_from = builder.needed_from(tracker.open_start, tracker.position)
        if keep_from > buffer_start:
            buffer = buffer[keep_from - buffer_start :].copy()
            buffer_start = keep_from

    if not work.out_of_time:
        _recognize_queue(index, segment, job, glossary, engines, watch, run, work)
    if work.out_of_time:
        # Lines exist up to the first clip that was not sent to Whisper, or up to the speech that had not ended.
        waiting = [clip.padded.start for clip, _ in work.queue]
        waiting.append(builder.needed_from(tracker.open_start, min(tracker.position, position)))
        work.processed_samples = min(*waiting, work.audio_samples)
        work.queue.clear()
    else:
        work.processed_samples = work.audio_samples


def _separate_speakers(
    job: Job,
    works: dict[int, _SegmentWork],
    results: list[SegmentResult],
    settings: Settings,
    watch: _Stopwatch,
    run: _Run,
) -> dict[int, dict[int, int]]:
    """Speaker number of each line (by segment index and line order), clustered per sharing participant."""
    scopes: dict[str, list[int]] = defaultdict(list)
    for index, segment in enumerate(job.segments):
        if segment.kind != "shared" or not job.separate_speakers or results[index].status == "failed":
            continue
        if works[index].speaker_error:
            _speaker_outcome(results[index], "failed", "embedding_failed")
        else:
            scopes[segment.participant.id].append(index)

    assigned: dict[int, dict[int, int]] = {}
    for indexes in scopes.values():
        if run.out_of_time():
            for index in indexes:
                works[index].vectors.clear()
                _speaker_outcome(results[index], "failed", "time_limit")
            continue
        windows = [window for index in indexes for window in works[index].windows]
        try:
            with watch.measure("speaker_clustering"):
                vectors = [vector for index in indexes for vector in works[index].vectors]
                stacked = np.stack(vectors) if vectors else np.zeros((0, 1), dtype=np.float32)
                del vectors
                for index in indexes:
                    works[index].vectors.clear()
                labels = cluster_speakers(stacked, settings.speaker_rules)
                del stacked
                turns = speaker_turns(windows, labels.tolist())
        except MemoryError:
            raise
        except Exception:
            for index in indexes:
                _speaker_outcome(results[index], "failed", "clustering_failed")
            continue
        for index in indexes:
            work = works[index]
            segment_turns = turns.get(index, [])
            assigned[index] = {}
            for line in work.lines:
                if work.unlabelled_from is not None and line.end_sample > work.unlabelled_from:
                    continue
                speaker = speaker_of(segment_turns, line.start_sample, line.end_sample)
                if speaker is not None:
                    assigned[index][line.order] = speaker
            if len(assigned[index]) == len(work.lines):
                _speaker_outcome(results[index], "separated", None)
            else:
                _speaker_outcome(results[index], "unassigned", work.unlabelled_reason or "no_voice_window")
    return assigned


def _speaker_outcome(result: SegmentResult, status: SpeakerStatus, code: str | None) -> None:
    result.speakers = status
    result.speakers_reason_code = code


def _segment_failure(result: SegmentResult, code: str, detail: str = "") -> None:
    result.status = "failed"
    result.reason_code = code
    result.reason_message = f"{REASONS[code]} ({detail})" if detail else REASONS[code]


def run_job(
    job: Job,
    files: AudioFiles,
    engines: Engines,
    settings: Settings,
    clock: Callable[[], float] = time.perf_counter,
) -> Outcome:
    """Processes every segment within the limits. The time limit counts from this call."""
    if job.language is not None and job.language not in engines.recognizer.languages:
        raise JobError("language_not_known_to_the_model", "/language")

    for position, term in enumerate(job.glossary):
        if engines.recognizer.control_token_in(term):
            raise JobError("glossary_control_token", f"/glossary/{position}")

    limits = settings.limits
    watch = _Stopwatch(clock)
    run = _Run(
        clock=clock,
        deadline=clock() + limits.processing_ms / 1000,
        audio_samples_left=samples_from_ms(limits.job_audio_ms),
        speaker_windows_left=limits.speaker_windows,
    )
    glossary = plan_glossary(job.glossary, engines.recognizer.count_tokens, settings.glossary_token_limit)

    results: list[SegmentResult] = []
    works: dict[int, _SegmentWork] = {}
    for index, segment in enumerate(job.segments):
        result = SegmentResult(id=segment.id)
        results.append(result)
        work = works[index] = _SegmentWork()
        try:
            if run.out_of_time():
                _segment_failure(result, "time_limit")
            elif run.audio_samples_left <= 0:
                _segment_failure(result, "job_duration_limit")
            else:
                with files.open(segment.path) as source:
                    _process_segment(index, segment, source, job, glossary.text, engines, settings, watch, run, work)
        except (EngineFatalError, MemoryError):
            raise
        except AudioMissingError:
            _segment_failure(result, "file_missing")
        except AudioRefusedError:
            _segment_failure(result, "not_a_regular_file")
        except AudioDecodeError as error:
            _segment_failure(result, error.code)
        except Exception as error:
            _segment_failure(result, "processing_failed", type(error).__name__)

        result.audio_ms = ms_from_samples(work.audio_samples)
        result.processed_ms = 0 if result.status == "failed" else ms_from_samples(work.processed_samples)
        if result.status == "failed":
            work.lines.clear()
            work.windows.clear()
            work.vectors.clear()
        else:
            result.speech_ms = ms_from_samples(work.speech_samples)
            result.clips = work.clips
            result.lines = len(work.lines)
            if work.damage is not None:
                result.status = "damaged"
                result.reason_code = work.damage
                result.reason_message = REASONS[work.damage]
            else:
                result.status = "ok" if work.lines else "empty"

        if segment.kind == "shared":
            if not job.separate_speakers:
                _speaker_outcome(result, "not_requested", None)
            elif result.status == "failed":
                _speaker_outcome(result, "failed", "segment_failed")

    assigned = _separate_speakers(job, works, results, settings, watch, run)

    lines = [line for work in works.values() for line in work.lines]
    lines.sort(key=lambda line: (line.start_ms, line.end_ms, line.segment_index, line.order))

    numbers: dict[tuple[str, int], int] = {}
    for line in lines:
        speaker = assigned.get(line.segment_index, {}).get(line.order)
        if speaker is None:
            continue
        participant = job.segments[line.segment_index].participant.id
        number = numbers.setdefault((participant, speaker), len(numbers) + 1)
        line.speaker = f"Speaker {number}"

    return Outcome(
        lines=tuple(lines),
        segments=tuple(results),
        glossary=glossary,
        speakers=tuple(NamedSpeaker(f"Speaker {number}", participant) for (participant, _), number in numbers.items()),
        stages_ms={stage: round(seconds * 1000) for stage, seconds in watch.seconds.items()},
    )


def audio_ms_total(outcome: Outcome) -> int:
    return sum(result.audio_ms for result in outcome.segments)


def speech_ms_total(outcome: Outcome) -> int:
    return sum(result.speech_ms for result in outcome.segments)
