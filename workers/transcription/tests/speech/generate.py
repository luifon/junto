"""Generates the speech files of the model tests with an offline text-to-speech engine.

Usage: generate.py <texts directory> <voices directory> <output directory>
                   [--seed N] [--only name,name] [--check SHA256SUMS]

Runs inside the image of `tests/speech/Dockerfile`, without network access. Every sentence is
synthesised alone, so the reference text and the reference times are exact. Each test file is
written as 16 kHz mono WAV and as 48 kHz stereo Ogg/Opus, the format the recorder writes.

The speech is reproducible. Piper adds random variation to every sentence; the generator sets
the random seed of ONNX Runtime from `--seed` (default 1), the voice and the text before it
loads the voice for that sentence, and ffmpeg writes without run-specific values. The same
seed gives the same files, and `SHA256SUMS` in the output directory lists their checksums.
`--check` compares them with a recorded list. Another seed gives another generation of the
same sentences, for a measurement of how much a result changes with the speech.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import wave
from pathlib import Path
from typing import Any

import numpy as np
import onnxruntime
import soundfile
from piper import PiperVoice

RATE = 16000
LEVEL_DBFS = -24.0
SPEECH_FLOOR_DBFS = -45.0

VOICES = {
    "en": "en_US-kristin-medium",
    "pt": "pt_BR-faber-medium",
    "A": "en_US-kristin-medium",
    "B": "en_US-norman-medium",
    "C": "en_GB-cori-medium",
}
QUICK_SENTENCES = {"en": 6, "pt": 6, "mix": 6, "gloss_en": 5, "gloss_pt": 5}
NOISE = {
    "brown": "anoisesrc=color=brown:amplitude=0.004:sample_rate=16000:seed=7",
    "pink": "anoisesrc=color=pink:amplitude=0.03:sample_rate=16000:seed=11",
}

Turn = tuple[str, str, str]

parser = argparse.ArgumentParser()
parser.add_argument("texts", type=Path)
parser.add_argument("voices", type=Path)
parser.add_argument("out", type=Path)
parser.add_argument("--seed", type=int, default=1, help="0 sets no seed: every run then gives other speech")
parser.add_argument("--only", default="", help="names of the files to generate, separated by commas (default: all)")
parser.add_argument("--check", type=Path, help="a recorded SHA256SUMS file to compare with")
arguments = parser.parse_args()

texts: Path = arguments.texts
voices: Path = arguments.voices
out: Path = arguments.out
cache = out / "sentences"
only = {name for name in arguments.only.split(",") if name}
BITEXACT = ["-fflags", "+bitexact", "-flags:a", "+bitexact"]


def run(command: list[str]) -> None:
    subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def lines(name: str) -> list[str]:
    return [line.strip() for line in (texts / name).read_text("utf-8").splitlines() if line.strip()]


def speech_range(samples: np.ndarray) -> tuple[float, float]:
    frame = RATE // 50
    count = len(samples) // frame
    level = np.sqrt(np.mean(samples[: count * frame].reshape(count, frame) ** 2, axis=1) + 1e-12)
    active = np.flatnonzero(20 * np.log10(level) > SPEECH_FLOOR_DBFS)
    return float(active[0] * frame / RATE), float((active[-1] + 1) * frame / RATE)


def sentence(voice: str, text: str) -> tuple[Path, float, tuple[float, float]]:
    """One sentence at a fixed level: its file, its length and the range that contains speech."""
    key = hashlib.sha256(f"{voice}|{text}".encode()).hexdigest()[:16]
    path = cache / f"{key}.wav"
    if not path.exists():
        # The random nodes of the voice take their seed when the voice is loaded.
        if arguments.seed:
            digest = hashlib.sha256(f"{arguments.seed}|{voice}|{text}".encode()).digest()
            onnxruntime.set_seed(int.from_bytes(digest[:4], "big") % 2**31)
        speaker: Any = PiperVoice.load(str(voices / f"{voice}.onnx"), config_path=str(voices / f"{voice}.onnx.json"))
        raw = cache / f"{key}.raw.wav"
        with wave.open(str(raw), "wb") as handle:
            speaker.synthesize(text, handle)
        resampled = cache / f"{key}.16k.wav"
        run(["ffmpeg", "-y", "-i", str(raw), "-ar", str(RATE), "-ac", "1", *BITEXACT, str(resampled)])
        samples, _ = soundfile.read(str(resampled), dtype="float32")
        gain = 10 ** (LEVEL_DBFS / 20) / np.sqrt(np.mean(samples**2) + 1e-12)
        soundfile.write(str(path), np.clip(samples * gain, -0.99, 0.99), RATE, subtype="PCM_16")
        raw.unlink()
        resampled.unlink()
    samples, _ = soundfile.read(str(path), dtype="float32")
    return path, len(samples) / RATE, speech_range(samples)


def mix(name: str, placed: list[tuple[float, Path]], seconds: float, noise: str | None = None) -> None:
    inputs: list[str] = []
    filters: list[str] = []
    labels: list[str] = []
    for index, (start, path) in enumerate(placed):
        inputs += ["-i", str(path)]
        filters.append(f"[{index}:a]adelay={round(start * 1000)}:all=1[a{index}]")
        labels.append(f"[a{index}]")
    if noise is not None or not placed:
        index = len(placed)
        source = noise or "anullsrc=sample_rate=16000:channel_layout=mono"
        inputs += ["-f", "lavfi", "-t", str(seconds), "-i", source]
        filters.append(f"[{index}:a]anull[a{index}]")
        labels.append(f"[a{index}]")
    filters.append("".join(labels) + f"amix=inputs={len(labels)}:normalize=0:duration=longest[m]")
    script = out / f"{name}.filter"
    script.write_text(";\n".join(filters))
    wav = out / f"{name}.wav"
    graph = ["-filter_complex_script", str(script), "-map", "[m]", "-t", str(seconds)]
    run(["ffmpeg", "-y", *inputs, *graph, "-ar", str(RATE), "-ac", "1", "-c:a", "pcm_s16le", *BITEXACT, str(wav)])
    script.unlink()
    opus = ["-ar", "48000", "-ac", "2", "-c:a", "libopus", "-b:a", "128k"]
    run(["ffmpeg", "-y", "-i", str(wav), *opus, *BITEXACT, str(out / f"{name}.ogg")])


def write(name: str, turns: list[Turn], gap: float = 0.5, lead: float = 0.5) -> None:
    """Sentences one after the other. A turn is (speaker, language, text)."""
    if only and name not in only:
        return
    cursor = lead
    placed: list[tuple[float, Path]] = []
    sentences: list[dict[str, Any]] = []
    for speaker, language, text in turns:
        path, length, (first, last) = sentence(VOICES[speaker], text)
        placed.append((cursor, path))
        sentences.append(
            {
                "speaker": speaker,
                "language": language,
                "text": text,
                "start": round(cursor + first, 3),
                "end": round(cursor + last, 3),
            }
        )
        cursor += length + gap
    seconds = round(cursor - gap + lead, 3)
    mix(name, placed, seconds)
    reference = {"seconds": seconds, "text": " ".join(item["text"] for item in sentences), "sentences": sentences}
    (out / f"{name}.json").write_text(json.dumps(reference, ensure_ascii=False, indent=1))
    print(f"{name}: {seconds:.1f} s, {len(turns)} sentences")


def write_with_quick(name: str, turns: list[Turn], gap: float = 0.5) -> None:
    write(name, turns, gap)
    write(f"{name}_quick", turns[: QUICK_SENTENCES[name]], gap)


def write_noise(name: str, colour: str, seconds: float, speech: list[tuple[float, str, str]]) -> None:
    if only and name not in only:
        return
    placed: list[tuple[float, Path]] = []
    sentences: list[dict[str, Any]] = []
    for at, language, text in speech:
        path, _, (first, last) = sentence(VOICES[language], text)
        placed.append((at, path))
        sentences.append(
            {
                "speaker": language,
                "language": language,
                "text": text,
                "start": round(at + first, 3),
                "end": round(at + last, 3),
            }
        )
    mix(name, placed, seconds, NOISE[colour])
    reference = {"seconds": seconds, "text": " ".join(item["text"] for item in sentences), "sentences": sentences}
    (out / f"{name}.json").write_text(json.dumps(reference, ensure_ascii=False, indent=1))
    print(f"{name}: {seconds:.1f} s of {colour} noise, {len(speech)} sentences")


cache.mkdir(parents=True, exist_ok=True)

write_with_quick("en", [("en", "en", text) for text in lines("en.txt")])
write_with_quick("pt", [("pt", "pt", text) for text in lines("pt.txt")])
write_with_quick("gloss_en", [("en", "en", text) for text in lines("glossary_en.txt")])
write_with_quick("gloss_pt", [("pt", "pt", text) for text in lines("glossary_pt.txt")])
write_with_quick(
    "mix", [(line.split("|")[0], line.split("|")[0], line.split("|", 1)[1]) for line in lines("mix.txt")], gap=0.4
)

trio = [(line.split("|")[0], "en", line.split("|", 1)[1]) for line in lines("trio.txt")]
write("trio", trio, gap=0.3)
write("trio_first", trio[:9], gap=0.3)
write("trio_second", trio[9:], gap=0.3)

sparse = [(float(line.split("|")[0]), line.split("|")[1], line.split("|", 2)[2]) for line in lines("sparse.txt")]
write_noise("noise_brown", "brown", 600.0, [])
write_noise("noise_pink", "pink", 600.0, [])
write_noise("noise_brown_quick", "brown", 120.0, [])
write_noise("sparse", "brown", 600.0, sparse)
write_noise("sparse_room", "pink", 600.0, sparse)

for path in sorted(cache.iterdir()):
    path.unlink()
cache.rmdir()

produced = sorted(path for path in out.iterdir() if path.suffix in {".wav", ".ogg", ".json"})
sums = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in produced}
(out / "SHA256SUMS").write_text("".join(f"{digest}  {name}\n" for name, digest in sums.items()))
print(f"seed {arguments.seed}: {len(sums)} files, checksums in SHA256SUMS")
if arguments.check is not None:
    pairs = (line.split("  ", 1) for line in arguments.check.read_text().splitlines() if line)
    recorded = {name: digest for digest, name in pairs}
    different = sorted(name for name, digest in sums.items() if recorded.get(name) != digest)
    if different:
        print(f"{len(different)} of {len(sums)} files differ from the recorded checksums: {', '.join(different[:8])}")
        raise SystemExit(3)
    print("every file has its recorded checksum")
