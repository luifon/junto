import argparse
import json
from pathlib import Path

from common import load_model, load_ref, transcribe, transcribe_clips, word_error_rate

parser = argparse.ArgumentParser()
parser.add_argument("--models", default="/models")
parser.add_argument("--audio", default="/audio")
parser.add_argument("--out", default="/out")
parser.add_argument("--device", default="cuda")
parser.add_argument("--compute-type")
parser.add_argument("--threads", type=int, default=8)
parser.add_argument("--model", action="append")
parser.add_argument("--file", action="append")
parser.add_argument("--mode", action="append")
parser.add_argument("--tag", default="")
args = parser.parse_args()

FILES = {
    "en_kristin": "en",
    "en_norman": "en",
    "pt_faber": "pt",
    "pt_cadu": "pt",
    "en_kristin_opus2": "en",
    "pt_faber_opus2": "pt",
    "mix_a": "mix",
    "mix_b": "mix",
    "long_en": "en",
    "long_pt": "pt",
}
MODES = {
    "sequential": {"batched": 0},
    "batched8": {"batched": 8},
    "sequential-multilingual": {"batched": 0, "multilingual": True},
    "batched8-multilingual": {"batched": 8, "multilingual": True},
    "clips8": {"clips": 8},
}


def mixed_reference(ref):
    return [(seg["speaker"], seg["text"]) for seg in ref["segments"]]


results = []
for model_name in args.model or ["large-v3-turbo", "large-v3"]:
    model, load_seconds = load_model(args.models, model_name, args.device, args.compute_type, args.threads)
    print(f"\n# {model_name} on {args.device}: load {load_seconds:.1f}s")
    transcribe(model, Path(args.audio) / "gloss_en.wav", vad_filter=True)
    for name in args.file or list(FILES):
        language = FILES[name]
        ref = load_ref(args.audio, name)
        for mode in args.mode or list(MODES):
            if language != "mix" and "multilingual" in mode:
                continue
            options = dict(MODES[mode])
            if "clips" in options:
                out = transcribe_clips(model, Path(args.audio) / f"{name}.wav", batch=options["clips"])
            else:
                batched = options.pop("batched")
                out = transcribe(model, Path(args.audio) / f"{name}.wav", batched=batched, vad_filter=True, **options)
            if language == "mix":
                reference = " ".join(text for _, text in mixed_reference(ref))
                rate, counts = word_error_rate(reference, out["text"], "pt")
            else:
                rate, counts = word_error_rate(ref["text"], out["text"], language)
            print(
                f"{model_name:15} {name:17} {mode:24} WER {rate * 100:5.1f}% "
                f"(S{counts['sub']} D{counts['del']} I{counts['ins']} / {counts['words']})  "
                f"lang={out['language']}  {out['duration']:.0f}s audio in {out['seconds']}s = {out['speed']}x"
            )
            results.append(
                {
                    "model": model_name,
                    "device": args.device,
                    "file": name,
                    "mode": mode,
                    "wer": round(rate, 4),
                    **counts,
                    "language": out["language"],
                    "audio_seconds": round(out["duration"], 1),
                    "seconds": out["seconds"],
                    "speed": out["speed"],
                    "text": out["text"],
                }
            )
    del model

Path(args.out).mkdir(parents=True, exist_ok=True)
(Path(args.out) / f"t3_{args.device}{args.tag}.json").write_text(json.dumps(results, ensure_ascii=False, indent=1))
