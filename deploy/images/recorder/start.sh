#!/usr/bin/env bash
# The start script of the recorder container (ADR 0602). The start script of the image
# deletes the temporary directory of the recorder at every start. This script runs first and
# copies the partial audio files that an ended recorder left there to
# <recordings>/rescued/<job>/<file>, where the server looks for them (ADR 0606).
#
# When a copy fails, the script ends without starting the recorder, so that the start script
# of the image does not delete the only copy. The container then starts again until the
# cause is removed, for example a full disk.
set -u
work="${JUNTO_RECORDER_WORK:-/home/egress/tmp}"
recordings="${JUNTO_RECORDER_DIR:-/var/lib/junto/recordings}"
image_start="${JUNTO_RECORDER_IMAGE_START:-/entrypoint.sh}"

if [ -d "$work" ]; then
    for file in "$work"/EG_*/*.ogg; do
        [ -f "$file" ] && [ ! -L "$file" ] || continue
        job=$(basename -- "$(dirname -- "$file")")
        target="$recordings/rescued/$job"
        if ! { mkdir -p -- "$target" && cp -- "$file" "$target/" && sync -- "$target/$(basename -- "$file")"; }; then
            echo "recorder start: a partial audio file of the job $job could not be copied to $recordings/rescued. The recorder does not start, and the file stays in $work." >&2
            exit 1
        fi
    done
fi
exec "$image_start"
