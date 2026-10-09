#!/bin/sh
# usage: run.sh <name> <gpu|cpu> <net|nonet> <cmd...>
name="$1"; dev="$2"; net="$3"; shift 3
gpu=""; [ "$dev" = gpu ] && gpu="--gpus all"
network=""; [ "$net" = nonet ] && network="--network none"
exec docker run --rm --name "junto-spike-stt-$name" $gpu $network --cpus 8 --memory 12g \
  -v /srv/junto-spike-stt/models:/models:ro \
  -v /srv/junto-spike-stt/audio:/audio:ro \
  -v /srv/junto-spike-stt/out:/out \
  -v /srv/junto-spike-stt/src/scripts:/src:ro \
  -v /srv/junto-spike-stt/src/texts:/texts:ro \
  -w /src junto-spike-stt-worker:2 "$@"
