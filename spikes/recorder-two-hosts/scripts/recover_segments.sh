#!/bin/sh
# Turns the segments of a recording into one MP4 without re-encoding.
#   recover_segments.sh PLAYLIST.m3u8 OUTPUT.mp4
# A playlist that a killed Egress left behind has no "#EXT-X-ENDLIST" line, and
# ffmpeg then waits for more segments without end. This script adds the line
# to a copy of the playlist first. When the playlist itself is missing or
# damaged, the second form joins the segment files by name:
#   recover_segments.sh --files 'DIR/PREFIX_*.ts' OUTPUT.mp4
set -eu
if [ "$1" = "--files" ]; then
  list=$(mktemp)
  for part in $2; do printf "file '%s'\n" "$(cd "$(dirname "$part")" && pwd)/$(basename "$part")"; done | sort > "$list"
  ffmpeg -v error -nostdin -y -f concat -safe 0 -i "$list" -c copy -movflags +faststart "$3"
  rm -f "$list"
  exit 0
fi
playlist=$1
if ! grep -q '^#EXT-X-ENDLIST' "$playlist"; then
  closed="${playlist%.m3u8}-closed.m3u8"
  { cat "$playlist"; echo '#EXT-X-ENDLIST'; } > "$closed"
  playlist=$closed
fi
ffmpeg -v error -nostdin -y -i "$playlist" -c copy -movflags +faststart "$2"
