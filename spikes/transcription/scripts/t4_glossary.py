import argparse
import json
from pathlib import Path

from common import load_model, load_ref, normalize, transcribe, transcribe_clips, word_error_rate

parser = argparse.ArgumentParser()
parser.add_argument("--models", default="/models")
parser.add_argument("--audio", default="/audio")
parser.add_argument("--texts", default="/texts")
parser.add_argument("--out", default="/out")
parser.add_argument("--device", default="cuda")
parser.add_argument("--model", action="append")
args = parser.parse_args()

TERMS = [line.strip() for line in (Path(args.texts) / "glossary_terms.txt").read_text().splitlines() if line.strip()]
GLOSSARY = ", ".join(TERMS) + "."
FILLER_WORDS = (
    "alder birch cedar dogwood elm fir ginkgo hazel juniper larch maple oak pine rowan spruce "
    "willow yew amber bronze copper crimson indigo ivory jade khaki lilac magenta ochre olive "
    "russet saffron scarlet teal umber violet anchor barrel candle drawer engine feather garden "
    "hammer island jacket kettle ladder mirror needle orchard pencil quarry ribbon saddle tunnel"
).split()
FILLER = ", ".join(word.capitalize() for word in FILLER_WORDS * 3) + "."

CONDITIONS = {
    "none": {},
    "initial_prompt": {"initial_prompt": GLOSSARY},
    "hotwords": {"hotwords": GLOSSARY},
    "long initial_prompt, terms first": {"initial_prompt": GLOSSARY + " " + FILLER},
    "long initial_prompt, terms last": {"initial_prompt": FILLER + " " + GLOSSARY},
    "long hotwords, terms first": {"hotwords": GLOSSARY + " " + FILLER},
    "long hotwords, terms last": {"hotwords": FILLER + " " + GLOSSARY},
}
MODES = ("sequential", "batched8", "clips8")


def spelling(reference, hypothesis):
    ref = " " + normalize(reference, "pt") + " "
    hyp = " " + normalize(hypothesis, "pt") + " "
    wanted = found = 0
    missed = []
    for term in TERMS:
        key = " " + normalize(term, "pt") + " "
        count = ref.count(key)
        if not count:
            continue
        got = min(count, hyp.count(key))
        wanted += count
        found += got
        if got < count:
            missed.append(f"{term} {got}/{count}")
    return found, wanted, missed


def run(model, name, mode, options):
    path = Path(args.audio) / f"{name}.wav"
    if mode == "clips8":
        return transcribe_clips(model, path, batch=8, **options)
    return transcribe(model, path, batched=8 if mode == "batched8" else 0, vad_filter=True, **options)


results = []
for model_name in args.model or ["large-v3-turbo", "large-v3"]:
    model, _ = load_model(args.models, model_name, args.device)
    encode = lambda text: model.hf_tokenizer.encode(" " + text, add_special_tokens=False).ids
    limit = model.max_length // 2 - 1
    print(
        f"\n# {model_name}: max_length={model.max_length}, prompt limit={limit} tokens; "
        f"glossary={len(encode(GLOSSARY))} tokens ({len(TERMS)} entries), "
        f"filler={len(encode(FILLER))} tokens, long={len(encode(GLOSSARY + ' ' + FILLER))} tokens"
    )
    for mode in MODES:
        for label, options in CONDITIONS.items():
            row = {"model": model_name, "mode": mode, "condition": label}
            cells = []
            for name in ("gloss_en", "gloss_pt"):
                ref = load_ref(args.audio, name)
                try:
                    out = run(model, name, mode, dict(options))
                except Exception as error:
                    row[name] = {"error": f"{type(error).__name__}: {error}"[:300]}
                    cells.append(f"{name}: ERROR {type(error).__name__}: {str(error)[:120]}")
                    continue
                found, wanted, missed = spelling(ref["text"], out["text"])
                rate, _ = word_error_rate(ref["text"], out["text"], name[-2:])
                row[name] = {"found": found, "wanted": wanted, "missed": missed, "wer": round(rate, 4), "text": out["text"]}
                cells.append(f"{name}: {found}/{wanted} terms, WER {rate * 100:.1f}%")
            print(f"{model_name:15} {mode:10} {label:34} " + " | ".join(cells))
            results.append(row)

    print(f"\n## {model_name}: side effects of a glossary on files without glossary terms (clips8)")
    for name, language in (("en_kristin", "en"), ("pt_faber", "pt"), ("silence_quiet", None)):
        ref = load_ref(args.audio, name)
        for label in ("none", "initial_prompt", "hotwords"):
            out = transcribe_clips(model, Path(args.audio) / f"{name}.wav", batch=8, **dict(CONDITIONS[label]))
            if language:
                rate, _ = word_error_rate(ref["text"], out["text"], language)
                print(f"{name:14} {label:15} WER {rate * 100:.1f}%")
                results.append({"model": model_name, "side_effect": name, "condition": label, "wer": round(rate, 4)})
            else:
                print(f"{name:14} {label:15} lines: {[row['text'] for row in out['segments']]}")
                results.append({"model": model_name, "side_effect": name, "condition": label, "lines": [row["text"] for row in out["segments"]]})
    del model

Path(args.out).mkdir(parents=True, exist_ok=True)
(Path(args.out) / f"t4_{args.device}.json").write_text(json.dumps(results, ensure_ascii=False, indent=1))
