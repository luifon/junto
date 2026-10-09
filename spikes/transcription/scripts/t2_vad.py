import argparse
import json
import time
from pathlib import Path

from common import load_model, load_ref, normalize, transcribe, transcribe_clips

parser = argparse.ArgumentParser()
parser.add_argument("--models", default="/models")
parser.add_argument("--audio", default="/audio")
parser.add_argument("--out", default="/out")
parser.add_argument("--device", default="cuda")
parser.add_argument("--model", action="append")
args = parser.parse_args()

from faster_whisper.audio import decode_audio
from faster_whisper.vad import VadOptions, get_speech_timestamps

results = []
for name in ("silence_quiet", "silence_room", "silence_en"):
    path = Path(args.audio) / f"{name}.wav"
    ref = load_ref(args.audio, name)
    samples = decode_audio(str(path), sampling_rate=16000)
    started = time.time()
    stamps = get_speech_timestamps(samples, VadOptions())
    vad_seconds = time.time() - started
    spans = [[round(s["start"] / 16000, 2), round(s["end"] / 16000, 2)] for s in stamps]
    print(f"\n## {name}: reference spans {[seg['spans'] for seg in ref['segments']]}")
    print(f"VAD alone: {spans} in {vad_seconds:.2f}s ({600 / vad_seconds:.0f}x real time)")
    results.append({"file": name, "vad_spans": spans, "vad_seconds": round(vad_seconds, 2)})

for model_name in args.model or ["large-v3-turbo", "large-v3"]:
    model, _ = load_model(args.models, model_name, args.device)
    for name in ("silence_quiet", "silence_room", "silence_en"):
        path = Path(args.audio) / f"{name}.wav"
        ref = load_ref(args.audio, name)
        wanted = [normalize(seg["text"], "pt") for seg in ref["segments"]]
        for label, options in (
            ("no VAD", {"vad_filter": False}),
            ("no VAD, multilingual", {"vad_filter": False, "multilingual": True}),
            ("VAD", {"vad_filter": True}),
            ("VAD, multilingual", {"vad_filter": True, "multilingual": True}),
            ("VAD, batched 8, multilingual", {"vad_filter": True, "multilingual": True, "batched": 8}),
            ("own VAD clips, language per clip", {"clips": True}),
        ):
            options = dict(options)
            batched = options.pop("batched", 0)
            if options.pop("clips", False):
                out = transcribe_clips(model, path)
            else:
                out = transcribe(model, path, batched=batched, **options)
            lines = [normalize(row["text"], "pt") for row in out["segments"]]
            exact = [line for line in lines if line in wanted]
            invented = [row for row in out["segments"] if normalize(row["text"], "pt") not in wanted]
            print(
                f"\n[{model_name}] {name} / {label}: language={out['language']} "
                f"lines={len(lines)} exact={len(exact)}/2 other={len(invented)} "
                f"time={out['seconds']}s"
            )
            for row in out["segments"][:40]:
                print(f"   {row['start']:7.2f}-{row['end']:7.2f} {row['text']}")
            if len(out["segments"]) > 40:
                print(f"   ... {len(out['segments']) - 40} more lines")
            results.append(
                {
                    "model": model_name,
                    "file": name,
                    "condition": label,
                    "language": out["language"],
                    "lines": len(lines),
                    "exact": len(exact),
                    "other": len(invented),
                    "seconds": out["seconds"],
                    "segments": out["segments"],
                }
            )
    del model

Path(args.out).mkdir(parents=True, exist_ok=True)
(Path(args.out) / f"t2_{args.device}.json").write_text(json.dumps(results, ensure_ascii=False, indent=1))
