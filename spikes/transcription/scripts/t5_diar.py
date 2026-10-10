import argparse
import json
from pathlib import Path

import soundfile as sf

from common import load_ref
from diar import diarize, error_rate, reference_spans

parser = argparse.ArgumentParser()
parser.add_argument("--models", default="/models")
parser.add_argument("--audio", default="/audio")
parser.add_argument("--out", default="/out")
parser.add_argument("--file", action="append")
parser.add_argument("--embedding", action="append")
parser.add_argument("--threshold", action="append", type=float)
parser.add_argument("--no-oracle", action="store_true")
parser.add_argument("--threads", type=int, default=8)
parser.add_argument("--provider", default="cpu")
parser.add_argument("--tag", default="")
args = parser.parse_args()

SEGMENTATION = f"{args.models}/diar/sherpa-onnx-pyannote-segmentation-3-0/model.onnx"
EMBEDDINGS = args.embedding or [
    "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k",
    "3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common",
    "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced",
    "3dspeaker_speech_eres2net_sv_en_voxceleb_16k",
    "wespeaker_en_voxceleb_resnet34_LM",
    "nemo_en_titanet_large",
]
FILES = args.file or ["diar_en", "diar_en_opus1", "diar_en_opus2", "diar_pt", "diar_pt_opus1", "diar_pt_opus2"]
THRESHOLDS = args.threshold or [0.5, 0.6, 0.7]

results = []
for embedding in EMBEDDINGS:
    for name in FILES:
        ref = load_ref(args.audio, name)
        spans = reference_spans(ref)
        speakers = len({label for _, _, label in spans})
        audio, rate = sf.read(f"{args.audio}/{name}.wav", dtype="float32")
        duration = len(audio) / rate
        settings = [("threshold", value) for value in THRESHOLDS]
        if not args.no_oracle:
            settings.append(("known count", speakers))
        for kind, value in settings:
            turns, seconds, _ = diarize(
                audio,
                SEGMENTATION,
                f"{args.models}/diar/{embedding}.onnx",
                clusters=value if kind == "known count" else -1,
                threshold=value if kind == "threshold" else 0.5,
                threads=args.threads,
                provider=args.provider,
            )
            strict = error_rate(spans, turns, duration)
            lenient = error_rate(spans, turns, duration, collar=0.25)
            print(
                f"{embedding[:46]:46} {name:18} {kind:11} {value:<4} speakers {strict['speakers_found']}/{speakers}  "
                f"DER {strict['der'] * 100:5.1f}% (miss {strict['missed'] * 100:4.1f} fa {strict['false_alarm'] * 100:4.1f} conf {strict['confusion'] * 100:4.1f})  "
                f"DER collar0.25 {lenient['der'] * 100:5.1f}%  right speaker {strict['correct_share'] * 100:5.1f}%  "
                f"{duration:.0f}s in {seconds:.1f}s = {duration / seconds:.1f}x"
            )
            results.append(
                {
                    "embedding": embedding,
                    "file": name,
                    "setting": kind,
                    "value": value,
                    "speakers_found": strict["speakers_found"],
                    "speakers_reference": speakers,
                    "der": round(strict["der"], 4),
                    "missed": round(strict["missed"], 4),
                    "false_alarm": round(strict["false_alarm"], 4),
                    "confusion": round(strict["confusion"], 4),
                    "der_collar": round(lenient["der"], 4),
                    "correct_share": round(strict["correct_share"], 4),
                    "audio_seconds": round(duration, 1),
                    "seconds": round(seconds, 2),
                    "speed": round(duration / seconds, 1),
                    "turns": [[round(a, 2), round(b, 2), s] for a, b, s in turns],
                }
            )

Path(args.out).mkdir(parents=True, exist_ok=True)
(Path(args.out) / f"t5{args.tag}.json").write_text(json.dumps(results, indent=1))
