#!/bin/sh
set -u
bin=/app/build/bin/whisper-cli
model=/models/whisper.cpp/ggml-large-v3-turbo.bin
vad=/models/whisper.cpp/ggml-silero-v6.2.0.bin
out=/out/wcpp
mkdir -p "$out"
glossary="Oswin Thackerby, Ysolde Varkonen, Brannoch Illesku, Jurandira Pindobaçu, Evaristo Castelhão, Quorvex, Lumitrax, Tarnbook, Velkira, Zyphra, Nexavio, Junto."

run() {
  label="$1"; file="$2"; shift 2
  "$bin" -m "$model" -f "/audio/$file.wav" -l auto -t 8 -oj -of "$out/$label" "$@" > "$out/$label.stdout" 2> "$out/$label.stderr"
  load=$(grep -E "load time" "$out/$label.stderr" | head -1 | sed -E 's/.*= *([0-9.]+) ms.*/\1/')
  total=$(grep -E "total time" "$out/$label.stderr" | head -1 | sed -E 's/.*= *([0-9.]+) ms.*/\1/')
  echo "$label	$file	load_ms=$load	total_ms=$total"
}

case "${1:-gpu}" in
gpu)
  run silence_quiet_novad silence_quiet
  run silence_quiet_vad silence_quiet --vad -vm "$vad"
  run silence_quiet_vad_pad400 silence_quiet --vad -vm "$vad" -vp 400
  run silence_room_novad silence_room
  run silence_room_vad_pad400 silence_room --vad -vm "$vad" -vp 400
  for f in en_kristin pt_faber mix_a mix_b long_en long_pt; do
    run "${f}_vad" "$f" --vad -vm "$vad" -vp 400
  done
  run gloss_en_none gloss_en --vad -vm "$vad" -vp 400
  run gloss_en_prompt gloss_en --vad -vm "$vad" -vp 400 --prompt "$glossary"
  run gloss_en_prompt_carry gloss_en --vad -vm "$vad" -vp 400 --prompt "$glossary" --carry-initial-prompt
  run gloss_pt_none gloss_pt --vad -vm "$vad" -vp 400
  run gloss_pt_prompt gloss_pt --vad -vm "$vad" -vp 400 --prompt "$glossary"
  run gloss_pt_prompt_carry gloss_pt --vad -vm "$vad" -vp 400 --prompt "$glossary" --carry-initial-prompt
  ;;
cpu)
  run cpu_en_kristin_vad en_kristin --vad -vm "$vad" -vp 400 -ng
  ;;
esac
