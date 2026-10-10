import json
import re
import resource
import subprocess
import threading
import time
import unicodedata
from pathlib import Path

NUMBER_WORDS = {
    "en": "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty".split(),
    "pt": "zero um dois três quatro cinco seis sete oito nove dez onze doze treze catorze quinze dezesseis dezessete dezoito dezenove vinte".split(),
}


def normalize(text, language="en"):
    text = unicodedata.normalize("NFC", text).lower()
    text = re.sub(r"[-–—/]", " ", text)
    text = "".join(ch for ch in text if not unicodedata.category(ch).startswith(("P", "S")))
    words = []
    for word in text.split():
        if word.isdigit() and int(word) < len(NUMBER_WORDS[language]):
            word = NUMBER_WORDS[language][int(word)]
        words.append(word)
    return " ".join(words)


def word_error_rate(reference, hypothesis, language="en"):
    import jiwer

    ref = normalize(reference, language)
    hyp = normalize(hypothesis, language)
    if not hyp:
        return 1.0, {"sub": 0, "del": len(ref.split()), "ins": 0, "words": len(ref.split())}
    out = jiwer.process_words(ref, hyp)
    return out.wer, {
        "sub": out.substitutions,
        "del": out.deletions,
        "ins": out.insertions,
        "words": len(ref.split()),
    }


def load_ref(audio_dir, name):
    return json.loads((Path(audio_dir) / f"{name}.ref.json").read_text())


def load_model(models_dir, name, device, compute_type=None, threads=8):
    from faster_whisper import WhisperModel

    if compute_type is None:
        compute_type = "float16" if device == "cuda" else "int8"
    started = time.time()
    model = WhisperModel(
        str(Path(models_dir) / f"faster-whisper-{name}"),
        device=device,
        compute_type=compute_type,
        cpu_threads=threads,
        local_files_only=True,
    )
    return model, time.time() - started


def transcribe(model, path, batched=0, **options):
    from faster_whisper import BatchedInferencePipeline

    started = time.time()
    if batched:
        segments, info = BatchedInferencePipeline(model).transcribe(
            str(path), batch_size=batched, **options
        )
    else:
        segments, info = model.transcribe(str(path), **options)
    rows = [
        {"start": round(s.start, 2), "end": round(s.end, 2), "text": s.text.strip()}
        for s in segments
    ]
    elapsed = time.time() - started
    return {
        "segments": rows,
        "text": " ".join(row["text"] for row in rows),
        "language": info.language,
        "language_probability": round(info.language_probability, 3),
        "duration": info.duration,
        "duration_after_vad": info.duration_after_vad,
        "seconds": round(elapsed, 2),
        "speed": round(info.duration / elapsed, 1),
    }


def speech_clips(samples, min_silence_ms=300, pad_ms=400, max_seconds=29.0):
    from faster_whisper.vad import VadOptions, get_speech_timestamps

    stamps = get_speech_timestamps(
        samples,
        VadOptions(
            min_silence_duration_ms=min_silence_ms,
            speech_pad_ms=pad_ms,
            max_speech_duration_s=max_seconds,
        ),
    )
    return [{"start": s["start"] / 16000, "end": s["end"] / 16000} for s in stamps]


def transcribe_clips(model, path, batch=8, min_silence_ms=300, **options):
    from faster_whisper import BatchedInferencePipeline
    from faster_whisper.audio import decode_audio

    started = time.time()
    samples = decode_audio(str(path), sampling_rate=16000)
    duration = len(samples) / 16000
    clips = speech_clips(samples, min_silence_ms=min_silence_ms)
    vad_seconds = time.time() - started
    rows = []
    languages = {}
    if clips:
        segments, _ = BatchedInferencePipeline(model).transcribe(
            samples,
            clip_timestamps=clips,
            vad_filter=False,
            multilingual=options.pop("multilingual", True),
            batch_size=batch,
            **options,
        )
        for s in segments:
            rows.append({"start": round(s.start, 2), "end": round(s.end, 2), "text": s.text.strip()})
    elapsed = time.time() - started
    return {
        "segments": rows,
        "text": " ".join(row["text"] for row in rows),
        "language": "per-clip",
        "language_probability": 0.0,
        "clips": len(clips),
        "duration": duration,
        "duration_after_vad": sum(c["end"] - c["start"] for c in clips),
        "vad_seconds": round(vad_seconds, 2),
        "seconds": round(elapsed, 2),
        "speed": round(duration / elapsed, 1),
    }


def peak_rss_mb():
    return round(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024)


def cgroup_peak_mb():
    try:
        return round(int(Path("/sys/fs/cgroup/memory.peak").read_text()) / 2**20)
    except OSError:
        return None


def gpu_used_mb():
    try:
        out = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=5,
        ).stdout
        return int(out.strip().splitlines()[0])
    except Exception:
        return None


class GpuSampler:
    def __init__(self, interval=0.2):
        self.interval = interval
        self.baseline = gpu_used_mb()
        self.peak = self.baseline
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._loop, daemon=True)

    def _loop(self):
        while not self._stop.is_set():
            used = gpu_used_mb()
            if used is not None and (self.peak is None or used > self.peak):
                self.peak = used
            self._stop.wait(self.interval)

    def __enter__(self):
        if self.baseline is not None:
            self._thread.start()
        return self

    def __exit__(self, *_):
        self._stop.set()
        if self._thread.is_alive():
            self._thread.join()
