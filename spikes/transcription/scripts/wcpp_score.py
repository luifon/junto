import json
import sys
from pathlib import Path

from common import load_ref, normalize, word_error_rate

out = Path("/out/wcpp")
terms = [line.strip() for line in Path("/texts/glossary_terms.txt").read_text().splitlines() if line.strip()]
languages = {"en_kristin": "en", "pt_faber": "pt", "mix_a": "pt", "mix_b": "pt", "long_en": "en", "long_pt": "pt", "gloss_en": "en", "gloss_pt": "pt"}

for line in sys.stdin:
    label, name, load, total = line.strip().split("\t")
    data = json.loads((out / f"{label}.json").read_text())
    rows = [row["text"].strip() for row in data["transcription"] if row["text"].strip()]
    text = " ".join(rows)
    ref = load_ref("/audio", name)
    seconds = (float(total.split("=")[1]) - float(load.split("=")[1])) / 1000
    speed = ref["duration"] / seconds
    detected = data.get("result", {}).get("language")
    if name.startswith("silence"):
        wanted = [normalize(seg["text"], "pt") for seg in ref["segments"]]
        exact = sum(1 for row in rows if normalize(row, "pt") in wanted)
        print(f"{label:28} lang={detected} lines={len(rows)} exact={exact}/2 other={len(rows) - exact}  {seconds:.1f}s  first lines: {rows[:4]}")
        continue
    rate, counts = word_error_rate(ref["text"], text, languages[name])
    extra = ""
    if name.startswith("gloss"):
        ref_text = " " + normalize(ref["text"], "pt") + " "
        hyp_text = " " + normalize(text, "pt") + " "
        wanted = found = 0
        for term in terms:
            key = " " + normalize(term, "pt") + " "
            wanted += ref_text.count(key)
            found += min(ref_text.count(key), hyp_text.count(key))
        extra = f"  terms {found}/{wanted}"
    print(
        f"{label:28} lang={detected} WER {rate * 100:5.1f}% (S{counts['sub']} D{counts['del']} I{counts['ins']} / {counts['words']})"
        f"{extra}  {ref['duration']:.0f}s audio in {seconds:.1f}s = {speed:.1f}x"
    )
