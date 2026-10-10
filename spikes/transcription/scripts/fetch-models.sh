#!/bin/sh
set -eu
dest="${1:-/models}"
hf="https://huggingface.co"
gh="https://github.com/k2-fsa/sherpa-onnx/releases/download"

get() {
  url="$1"; out="$2"
  mkdir -p "$(dirname "$out")"
  if [ ! -s "$out" ]; then
    curl -fsSL --retry 5 --retry-delay 3 -o "$out.part" "$url"
    mv "$out.part" "$out"
  fi
}

ct2_model() {
  repo="$1"; rev="$2"; dir="$3"
  for f in config.json model.bin preprocessor_config.json tokenizer.json vocabulary.json; do
    get "$hf/$repo/resolve/$rev/$f" "$dest/$dir/$f"
  done
}

ct2_model dropbox-dash/faster-whisper-large-v3-turbo 0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf faster-whisper-large-v3-turbo
ct2_model Systran/faster-whisper-large-v3 edaa852ec7e145841d8ffdb056a99866b5f0a478 faster-whisper-large-v3

get "$gh/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2" "$dest/diar/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2"
if [ ! -d "$dest/diar/sherpa-onnx-pyannote-segmentation-3-0" ]; then
  python3 -c 'import sys, tarfile; tarfile.open(sys.argv[1]).extractall(sys.argv[2], filter="data")' \
    "$dest/diar/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2" "$dest/diar"
fi
for m in \
  3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx \
  3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common.onnx \
  3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx \
  3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx \
  wespeaker_en_voxceleb_resnet34_LM.onnx \
  nemo_en_titanet_large.onnx
do
  get "$gh/speaker-recongition-models/$m" "$dest/diar/$m"
done

whisper_rev=5359861c739e955e79d9a303bcbc70fb988958b1
get "$hf/ggerganov/whisper.cpp/resolve/$whisper_rev/ggml-large-v3-turbo.bin" "$dest/whisper.cpp/ggml-large-v3-turbo.bin"
get "$hf/ggml-org/whisper-vad/resolve/9ffd54a1e1ee413ddf265af9913beaf518d1639b/ggml-silero-v6.2.0.bin" "$dest/whisper.cpp/ggml-silero-v6.2.0.bin"

voice() {
  path="$1"; name="$2"
  get "$hf/rhasspy/piper-voices/resolve/main/$path/$name.onnx" "$dest/piper/$name.onnx"
  get "$hf/rhasspy/piper-voices/resolve/main/$path/$name.onnx.json" "$dest/piper/$name.onnx.json"
  get "$hf/rhasspy/piper-voices/resolve/main/$path/MODEL_CARD" "$dest/piper/$name.MODEL_CARD"
}
voice en/en_US/kristin/medium en_US-kristin-medium
voice en/en_US/norman/medium en_US-norman-medium
voice en/en_US/joe/medium en_US-joe-medium
voice en/en_GB/cori/medium en_GB-cori-medium
voice pt/pt_BR/faber/medium pt_BR-faber-medium
voice pt/pt_BR/cadu/medium pt_BR-cadu-medium
voice pt/pt_BR/jeff/medium pt_BR-jeff-medium

cd "$dest"
find . -type f ! -name '*.tar.bz2' ! -name SHA256SUMS | sort | xargs sha256sum > SHA256SUMS
du -sh "$dest"/*
