import argparse
import json
import time
from pathlib import Path

import numpy as np
import soundfile as sf

from common import load_ref
from diar import (
    cluster_windows,
    embed_windows,
    error_rate,
    load_extractor,
    reference_spans,
    speaker_windows,
    turns_from_windows,
)

parser = argparse.ArgumentParser()
parser.add_argument("--models", default="/models")
parser.add_argument("--audio", default="/audio")
parser.add_argument("--out", default="/out")
parser.add_argument("--file", action="append")
parser.add_argument("--embedding", action="append")
parser.add_argument("--threshold", action="append", type=float)
parser.add_argument("--window", type=float, default=2.5)
parser.add_argument("--hop", type=float, default=1.25)
parser.add_argument("--threads", type=int, default=8)
parser.add_argument("--tag", default="")
args = parser.parse_args()

RATE = 16000
EMBEDDINGS = args.embedding or [
    "3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k",
    "3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common",
    "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced",
    "3dspeaker_speech_eres2net_sv_en_voxceleb_16k",
    "wespeaker_en_voxceleb_resnet34_LM",
    "nemo_en_titanet_large",
]
FILES = args.file or ["diar_en", "diar_en_opus1", "diar_en_opus2", "diar_pt", "diar_pt_opus1", "diar_pt_opus2"]
THRESHOLDS = args.threshold or [0.5, 0.6, 0.7, 0.8]


results = []
for embedding in EMBEDDINGS:
    extractor = load_extractor(f"{args.models}/diar/{embedding}.onnx", args.threads)
    for name in FILES:
        ref = load_ref(args.audio, name)
        spans = reference_spans(ref)
        speakers = len({label for _, _, label in spans})
        audio, _ = sf.read(f"{args.audio}/{name}.wav", dtype="float32")
        duration = len(audio) / RATE
        started = time.time()
        pieces = speaker_windows(audio, args.window, args.hop)
        vectors = embed_windows(extractor, audio, pieces)
        seconds = time.time() - started
        for threshold in THRESHOLDS:
            turns = turns_from_windows(pieces, cluster_windows(vectors, threshold))
            strict = error_rate(spans, turns, duration)
            lenient = error_rate(spans, turns, duration, collar=0.25)
            print(
                f"{embedding[:46]:46} {name:18} threshold {threshold:<4} speakers {strict['speakers_found']}/{speakers}  "
                f"DER {strict['der'] * 100:5.1f}% (miss {strict['missed'] * 100:4.1f} fa {strict['false_alarm'] * 100:4.1f} conf {strict['confusion'] * 100:4.1f})  "
                f"DER collar0.25 {lenient['der'] * 100:5.1f}%  right speaker {strict['correct_share'] * 100:5.1f}%  "
                f"{duration:.0f}s in {seconds:.1f}s = {duration / seconds:.1f}x ({len(pieces)} windows)"
            )
            results.append(
                {
                    "embedding": embedding,
                    "file": name,
                    "threshold": threshold,
                    "window": args.window,
                    "hop": args.hop,
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
                }
            )

Path(args.out).mkdir(parents=True, exist_ok=True)
(Path(args.out) / f"t5_simple{args.tag}.json").write_text(json.dumps(results, indent=1))
