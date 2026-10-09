#!/bin/bash
# Usage: snapshot.sh <label>   Saves the PipeWire graph and related state under $SPIKE_ROOT/state/<label>/
set -u
. "$(dirname "$0")/env.sh"
label=$1
out=$SPIKE_ROOT/state/$label
mkdir -p "$out"
as_user pw-dump > "$out/pw-dump.json" 2> "$out/pw-dump.err"
python3 "$(dirname "$0")/graph_summary.py" "$out/pw-dump.json" > "$out/graph-summary.txt"
as_user pw-cli ls Node > "$out/pw-cli-nodes.txt" 2>/dev/null
as_user pw-cli ls Link > "$out/pw-cli-links.txt" 2>/dev/null
as_user pw-cli ls Module > "$out/pw-cli-modules.txt" 2>/dev/null
as_user pw-link -l > "$out/pw-link.txt" 2>/dev/null
as_user pactl list modules short > "$out/pactl-modules.txt" 2>/dev/null
as_user pactl list sinks short > "$out/pactl-sinks.txt" 2>/dev/null
as_user pactl list sink-inputs short > "$out/pactl-sink-inputs.txt" 2>/dev/null
as_user pactl get-default-sink > "$out/default-sink.txt" 2>/dev/null
as_user pactl get-sink-volume @DEFAULT_SINK@ > "$out/default-sink-volume.txt" 2>/dev/null
as_user pactl get-sink-mute @DEFAULT_SINK@ >> "$out/default-sink-volume.txt" 2>/dev/null
as_user pw-metadata > "$out/metadata-default.txt" 2>/dev/null
as_user pw-metadata -n settings > "$out/metadata-settings.txt" 2>/dev/null
state_dir=$(getent passwd "$SPIKE_USER" | cut -d: -f6)/.local/state/wireplumber
( cd "$state_dir" && sha256sum * ) > "$out/wireplumber-state-sha256.txt" 2>/dev/null
( cd "$state_dir" && stat -c '%n %Y %s' * ) > "$out/wireplumber-state-stat.txt" 2>/dev/null
docker ps -a --format '{{.Names}}' | sort > "$out/docker-containers.txt"
docker images --format '{{.Repository}}:{{.Tag}} {{.ID}}' | sort > "$out/docker-images.txt"
systemctl --user -M "$SPIKE_USER"@ is-active pipewire pipewire-pulse wireplumber > "$out/user-units.txt" 2>&1
ps -o pid=,lstart=,args= -C pipewire,pipewire-pulse,wireplumber > "$out/daemons.txt" 2>&1
wc -l "$out/graph-summary.txt"
