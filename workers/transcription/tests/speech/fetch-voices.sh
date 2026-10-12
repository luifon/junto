#!/bin/sh
# Downloads the Piper voices of the model tests. Test data only: the voices and the generated
# audio are not distributed with Junto. Usage: fetch-voices.sh <directory>
set -eu
dest="${1:?usage: fetch-voices.sh <directory>}"
base="https://huggingface.co/rhasspy/piper-voices/resolve/c10ece1aade47bb51c153c893d14e5bf8e5b7117"
mkdir -p "$dest"
# Keeps a file only with its SHA-256 checksum. A file that is there with another content,
# for example from a download that was interrupted, is downloaded again.
fetch() {
  if [ ! -s "$dest/$1" ] || ! echo "$3  $dest/$1" | sha256sum -c - > /dev/null 2>&1; then
    curl -fsSL --retry 5 --retry-delay 3 -o "$dest/$1.part" "$2"
    echo "$3  $dest/$1.part" | sha256sum -c - > /dev/null \
      || { echo "$1 does not have its checksum" >&2; rm -f "$dest/$1.part"; exit 1; }
    mv "$dest/$1.part" "$dest/$1"
  fi
}
# voice <directory of the voice> <name> <checksum of the model> <checksum of its settings>
voice() {
  fetch "$2.onnx" "$base/$1/$2.onnx" "$3"
  fetch "$2.onnx.json" "$base/$1/$2.onnx.json" "$4"
  [ -s "$dest/$2.MODEL_CARD" ] || curl -fsSL --retry 5 -o "$dest/$2.MODEL_CARD" "$base/$1/MODEL_CARD"
}
voice en/en_US/kristin/medium en_US-kristin-medium \
  5849957f929cbf720c258f8458692d6103fff2f0e3d3b19c8259474bb06a18d4 5681426d4aead22195de70531eeeeddb46493cfaffc5764b2ea3db73428b651c
voice en/en_US/norman/medium en_US-norman-medium \
  b9739443232a80a59c7d18810dd856899bf16a7964725f5ab81ea49b1351cb71 6c2db7f558a4a8deb9fe822583c1c5105f6c4e834dd0f9de8ad17a888ee9fe1d
voice en/en_GB/cori/medium en_GB-cori-medium \
  1899f98e5fb8310154f3c2973f4b8a929ba7245e722b3d3a85680b833d95f10d e262c16d7f192f69d4edd6b4ef8a5915379e67495fcc402f1ab15eeb33da3d36
voice pt/pt_BR/faber/medium pt_BR-faber-medium \
  858555e3a064209c57088fe6bd70c4c3dc54d03eaa00c45d5ecaf43a33f95aa7 7e694de195ae3fc36dd732c445eb04fb49b649854893cb5506b978f0d50a1d6f
