import hashlib
import json
import subprocess
import sys
import wave
from pathlib import Path

import numpy as np
import soundfile as sf
from piper import PiperVoice

RATE = 16000
TEXTS = Path(sys.argv[1])
VOICES = Path(sys.argv[2])
OUT = Path(sys.argv[3])
UTT = OUT / "utt"
UTT.mkdir(parents=True, exist_ok=True)

_voices = {}


def run(cmd):
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def voice(name):
    if name not in _voices:
        _voices[name] = PiperVoice.load(
            str(VOICES / f"{name}.onnx"), config_path=str(VOICES / f"{name}.onnx.json")
        )
    return _voices[name]


def voiced_spans(samples):
    frame = RATE // 50
    count = len(samples) // frame
    rms = np.sqrt(np.mean(samples[: count * frame].reshape(count, frame) ** 2, axis=1) + 1e-12)
    active = 20 * np.log10(rms) > -45.0
    spans = []
    start = None
    last = None
    for index, flag in enumerate(active):
        if not flag:
            continue
        if start is None:
            start = index
        elif index - last > 25:
            spans.append((start * frame / RATE, (last + 1) * frame / RATE))
            start = index
        last = index
    if start is not None:
        spans.append((start * frame / RATE, (last + 1) * frame / RATE))
    return spans


def utterance(voice_name, text):
    key = hashlib.sha1(f"{voice_name}|{text}".encode()).hexdigest()[:16]
    path = UTT / f"{key}.wav"
    if not path.exists():
        raw = UTT / f"{key}.raw.wav"
        with wave.open(str(raw), "wb") as handle:
            voice(voice_name).synthesize(text, handle)
        resampled = UTT / f"{key}.rs.wav"
        run(["ffmpeg", "-y", "-i", str(raw), "-ar", str(RATE), "-ac", "1", str(resampled)])
        samples, _ = sf.read(str(resampled), dtype="float32")
        level = np.sqrt(np.mean(samples**2) + 1e-12)
        samples = np.clip(samples * (10 ** (-24 / 20) / level), -0.99, 0.99)
        sf.write(str(path), samples, RATE, subtype="PCM_16")
        raw.unlink()
        resampled.unlink()
    samples, _ = sf.read(str(path), dtype="float32")
    return path, len(samples) / RATE, voiced_spans(samples)


def mix(name, placed, total=None, bed=None):
    inputs = []
    filters = []
    labels = []
    for index, (start, path) in enumerate(placed):
        inputs += ["-i", str(path)]
        delay = int(round(start * 1000))
        filters.append(f"[{index}:a]adelay={delay}:all=1[a{index}]")
        labels.append(f"[a{index}]")
    if bed:
        index = len(placed)
        inputs += ["-f", "lavfi", "-t", str(total), "-i", bed]
        filters.append(f"[{index}:a]anull[a{index}]")
        labels.append(f"[a{index}]")
    filters.append(
        "".join(labels) + f"amix=inputs={len(labels)}:normalize=0:duration=longest[m]"
    )
    script = OUT / f"{name}.filter"
    script.write_text(";\n".join(filters))
    out = OUT / f"{name}.wav"
    cmd = ["ffmpeg", "-y", *inputs, "-filter_complex_script", str(script), "-map", "[m]"]
    if total:
        cmd += ["-t", str(total)]
    cmd += ["-ar", str(RATE), "-ac", "1", "-c:a", "pcm_s16le", str(out)]
    run(cmd)
    script.unlink()
    return out


def write_ref(name, segments, duration):
    ref = {
        "duration": duration,
        "text": " ".join(segment["text"] for segment in segments),
        "segments": segments,
    }
    (OUT / f"{name}.ref.json").write_text(json.dumps(ref, ensure_ascii=False, indent=1))


def sequence(name, turns, gap, lead=0.5):
    cursor = lead
    placed = []
    segments = []
    for speaker, voice_name, text, overlap in turns:
        path, length, spans = utterance(voice_name, text)
        start = cursor - (1.5 + gap if overlap else 0.0)
        placed.append((start, path))
        segments.append(
            {
                "speaker": speaker,
                "voice": voice_name,
                "text": text,
                "overlap_with_previous": bool(overlap),
                "spans": [[round(start + a, 3), round(start + b, 3)] for a, b in spans],
            }
        )
        cursor = start + length + gap
    duration = cursor + lead
    mix(name, placed, total=round(duration, 3))
    write_ref(name, segments, duration)
    print(name, f"{duration:.1f}s", len(turns), "utterances")


def lines(file):
    return [line.strip() for line in (TEXTS / file).read_text().splitlines() if line.strip()]


def monologue(name, file, voice_name):
    sequence(name, [("A", voice_name, text, False) for text in lines(file)], gap=0.5)


def mixed(name, voices):
    turns = []
    for line in lines("mix.txt"):
        language, text = line.split("|", 1)
        turns.append((language, voices[language], text, False))
    sequence(name, turns, gap=0.4)


def dialogue(name, file, voices):
    turns = []
    for line in lines(file):
        parts = line.split("|")
        turns.append((parts[0], voices[parts[0]], parts[1], len(parts) > 2))
    sequence(name, turns, gap=0.3)


def near_silence(name, bed, sentences="silence_sentences.txt"):
    total = 600.0
    placed = []
    segments = []
    for line in lines(sentences):
        at, voice_name, text = line.split("|", 2)
        path, _, spans = utterance(voice_name, text)
        placed.append((float(at), path))
        segments.append(
            {
                "speaker": "A",
                "voice": voice_name,
                "text": text,
                "spans": [[round(float(at) + a, 3), round(float(at) + b, 3)] for a, b in spans],
            }
        )
    mix(name, placed, total=total, bed=bed)
    write_ref(name, segments, total)
    print(name, f"{total:.1f}s")


def concat(name, parts):
    inputs = []
    segments = []
    offset = 0.0
    for part in parts:
        inputs += ["-i", str(OUT / f"{part}.wav")]
        ref = json.loads((OUT / f"{part}.ref.json").read_text())
        for segment in ref["segments"]:
            moved = dict(segment)
            moved["spans"] = [[round(a + offset, 3), round(b + offset, 3)] for a, b in segment["spans"]]
            segments.append(moved)
        offset += sf.info(str(OUT / f"{part}.wav")).duration
    graph = "".join(f"[{i}:a]" for i in range(len(parts))) + f"concat=n={len(parts)}:v=0:a=1[m]"
    run(["ffmpeg", "-y", *inputs, "-filter_complex", graph, "-map", "[m]", "-ar", str(RATE), "-ac", "1", "-c:a", "pcm_s16le", str(OUT / f"{name}.wav")])
    write_ref(name, segments, offset)
    print(name, f"{offset:.1f}s")


def opus_chain(name):
    source = OUT / f"{name}.wav"
    first = OUT / f"{name}.opus1.ogg"
    second = OUT / f"{name}.opus2.ogg"
    run(["ffmpeg", "-y", "-i", str(source), "-ar", "48000", "-c:a", "libopus", "-b:a", "32k", str(first)])
    run(["ffmpeg", "-y", "-i", str(first), "-ar", "48000", "-c:a", "libopus", "-b:a", "32k", str(second)])
    for stage, encoded in (("opus1", first), ("opus2", second)):
        run(["ffmpeg", "-y", "-i", str(encoded), "-ar", str(RATE), "-ac", "1", "-c:a", "pcm_s16le", str(OUT / f"{name}_{stage}.wav")])
        (OUT / f"{name}_{stage}.ref.json").write_text((OUT / f"{name}.ref.json").read_text())
    print(name, "opus chain", first.stat().st_size, second.stat().st_size)


monologue("en_kristin", "en.txt", "en_US-kristin-medium")
monologue("en_norman", "en.txt", "en_US-norman-medium")
monologue("pt_faber", "pt.txt", "pt_BR-faber-medium")
monologue("pt_cadu", "pt.txt", "pt_BR-cadu-medium")
mixed("mix_a", {"en": "en_US-kristin-medium", "pt": "pt_BR-faber-medium"})
mixed("mix_b", {"en": "en_US-norman-medium", "pt": "pt_BR-cadu-medium"})
monologue("gloss_en", "glossary_en.txt", "en_US-norman-medium")
monologue("gloss_pt", "glossary_pt.txt", "pt_BR-faber-medium")
near_silence("silence_quiet", "anoisesrc=color=brown:amplitude=0.004:sample_rate=16000:seed=7")
near_silence(
    "silence_room",
    "anoisesrc=color=pink:amplitude=0.03:sample_rate=16000:seed=11",
)
dialogue(
    "diar_en",
    "diar_en.txt",
    {
        "A": "en_US-kristin-medium",
        "B": "en_US-norman-medium",
        "C": "en_GB-cori-medium",
        "D": "en_US-joe-medium",
    },
)
dialogue(
    "diar_pt",
    "diar_pt.txt",
    {"A": "pt_BR-faber-medium", "B": "pt_BR-cadu-medium", "C": "pt_BR-jeff-medium"},
)
for item in ("diar_en", "diar_pt", "en_kristin", "pt_faber"):
    opus_chain(item)
near_silence(
    "silence_en",
    "anoisesrc=color=brown:amplitude=0.004:sample_rate=16000:seed=7",
    "silence_sentences_en.txt",
)
concat("long_en", ["en_kristin", "en_norman", "en_kristin_opus2", "en_norman", "en_kristin"])
concat("long_pt", ["pt_faber", "pt_cadu", "pt_faber_opus2", "pt_cadu", "pt_faber"])
concat("long_diar_en", ["diar_en"] * 6)
concat("long_diar_en_opus2", ["diar_en_opus2"] * 6)
