#!/bin/bash
# Copies the built binary into the run directory and generates the test tones.
. "$(dirname "$0")/env.sh"
RUN=$SPIKE_ROOT/run
mkdir -p "$RUN"
cp "$SPIKE_ROOT/src/target/release/junto-audio-spike" "$RUN/junto-audio-spike"
chown -R "$SPIKE_USER" "$RUN"
ldd "$RUN/junto-audio-spike"
for spec in "440 a" "1000 b" "1700 d" "3300 e"; do
  set -- $spec
  as_user "$RUN/junto-audio-spike" gen-wav --freq "$1" --seconds 40 --amp 0.2 --out "$RUN/tone-$2.wav"
done
