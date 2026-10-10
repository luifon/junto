import argparse
import json
import os
import time
from pathlib import Path

from common import GpuSampler, cgroup_peak_mb, load_model, peak_rss_mb, speech_clips, word_error_rate

parser = argparse.ArgumentParser()
parser.add_argument("--kind", choices=("mic", "shared"), required=True)
parser.add_argument("--audio", required=True)
parser.add_argument("--models", default="/models")
parser.add_argument("--model", default="large-v3-turbo")
parser.add_argument("--device", default="cuda")
parser.add_argument("--compute-type")
parser.add_argument("--threads", type=int, default=8)
parser.add_argument("--batch", type=int, default=8)
parser.add_argument("--glossary")
parser.add_argument("--member", default="Member")
parser.add_argument("--embedding", default="nemo_en_titanet_large")
parser.add_argument("--threshold", type=float, default=0.6)
parser.add_argument("--reference")
parser.add_argument("--reference-language", default="en")
parser.add_argument("--stats")
parser.add_argument("--quiet", action="store_true")
args = parser.parse_args()

from faster_whisper import BatchedInferencePipeline
from faster_whisper.audio import decode_audio

stages = {}
with GpuSampler() as gpu:
    started = time.time()
    samples = decode_audio(args.audio, sampling_rate=16000)
    duration = len(samples) / 16000
    stages["decode"] = time.time() - started

    started = time.time()
    clips = speech_clips(samples)
    stages["voice_detection"] = time.time() - started
    voiced = sum(clip["end"] - clip["start"] for clip in clips)

    model, stages["model_load"] = load_model(args.models, args.model, args.device, args.compute_type, args.threads)
    hotwords = Path(args.glossary).read_text().strip() if args.glossary else None

    started = time.time()
    lines = []
    if clips:
        segments, _ = BatchedInferencePipeline(model).transcribe(
            samples,
            clip_timestamps=clips,
            vad_filter=False,
            multilingual=True,
            batch_size=args.batch,
            hotwords=hotwords,
        )
        lines = [{"start": s.start, "end": s.end, "text": s.text.strip()} for s in segments if s.text.strip()]
    stages["whisper"] = time.time() - started

    mapping = {}
    turns = []
    if args.kind == "shared":
        from diar import diarize_simple, error_rate, reference_spans

        turns, stages["diarization"], stages["diarization_load"] = diarize_simple(
            samples,
            f"{args.models}/diar/{args.embedding}.onnx",
            threshold=args.threshold,
            threads=args.threads,
        )
        for line in lines:
            overlap = {}
            for start, end, speaker in turns:
                shared = min(end, line["end"]) - max(start, line["start"])
                if shared > 0:
                    overlap[speaker] = overlap.get(speaker, 0.0) + shared
            line["speaker"] = max(overlap, key=overlap.get) if overlap else None

names = {}
for line in lines:
    if args.kind == "mic":
        label = args.member
    elif line["speaker"] is None:
        label = "Shared audio"
    else:
        label = f"Speaker {names.setdefault(line['speaker'], len(names) + 1)}"
    line["label"] = label
    if not args.quiet:
        print(f"[{line['start']:7.2f}-{line['end']:7.2f}] {label}: {line['text']}")

stats = {
    "kind": args.kind,
    "audio": os.path.basename(args.audio),
    "model": args.model,
    "device": args.device,
    "audio_seconds": round(duration, 1),
    "voiced_seconds": round(voiced, 1),
    "clips": len(clips),
    "lines": len(lines),
    "stage_seconds": {key: round(value, 2) for key, value in stages.items()},
    "peak_rss_mb": peak_rss_mb(),
    "cgroup_peak_mb": cgroup_peak_mb(),
    "gpu_baseline_mb": gpu.baseline,
    "gpu_peak_mb": gpu.peak,
}
processing = sum(value for key, value in stages.items() if key not in ("model_load", "diarization_load"))
stats["processing_seconds"] = round(processing, 2)
stats["speed"] = round(duration / processing, 1)

if args.reference:
    ref = json.loads(Path(args.reference).read_text())
    rate, counts = word_error_rate(ref["text"], " ".join(line["text"] for line in lines), args.reference_language)
    stats["wer"] = round(rate, 4)
    if args.kind == "shared":
        spans = reference_spans(ref)
        score = error_rate(spans, turns, duration)
        stats["der"] = round(score["der"], 4)
        stats["speakers_found"] = score["speakers_found"]
        stats["speakers_reference"] = score["speakers_reference"]
        right = total = 0
        for line in lines:
            overlap = {}
            for start, end, speaker in spans:
                shared = min(end, line["end"]) - max(start, line["start"])
                if shared > 0:
                    overlap[speaker] = overlap.get(speaker, 0.0) + shared
            if not overlap:
                continue
            words = len(line["text"].split())
            total += words
            if score["mapping"].get(line["speaker"]) == max(overlap, key=overlap.get):
                right += words
        stats["words_with_right_speaker"] = round(right / total, 4) if total else None

print(json.dumps(stats))
if args.stats:
    Path(args.stats).write_text(json.dumps(stats, indent=1))
