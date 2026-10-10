#!/bin/sh
# Downloads the Piper voices of the model tests. Test data only: the voices and the generated
# audio are not distributed with Junto. Usage: fetch-voices.sh <directory>
set -eu
dest="${1:?usage: fetch-voices.sh <directory>}"
base="https://huggingface.co/rhasspy/piper-voices/resolve/c10ece1aade47bb51c153c893d14e5bf8e5b7117"
mkdir -p "$dest"
voice() {
  for suffix in onnx onnx.json; do
    [ -s "$dest/$2.$suffix" ] || curl -fsSL --retry 5 --retry-delay 3 -o "$dest/$2.$suffix" "$base/$1/$2.$suffix"
  done
  [ -s "$dest/$2.MODEL_CARD" ] || curl -fsSL --retry 5 -o "$dest/$2.MODEL_CARD" "$base/$1/MODEL_CARD"
}
voice en/en_US/kristin/medium en_US-kristin-medium
voice en/en_US/norman/medium en_US-norman-medium
voice en/en_GB/cori/medium en_GB-cori-medium
voice pt/pt_BR/faber/medium pt_BR-faber-medium
