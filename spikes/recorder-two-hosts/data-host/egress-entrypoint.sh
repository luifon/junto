#!/usr/bin/env bash
# Runs before the image's own entrypoint, which deletes Egress's working
# directory. Copies the media files that a killed Egress left there to
# /out/rescued/<egress id>/, then starts Egress in the normal way.
set -u
cd /home/egress/tmp 2>/dev/null && {
  mkdir -p /out/rescued
  find . -path './EG_*' -type f \( -name '*.ogg' -o -name '*.ts' -o -name '*.m3u8' -o -name '*.mp4' -o -name '*.json' \) \
    -not -path '*chromedp*' -not -path '*Chrome*' -exec cp --parents -t /out/rescued {} + 2>/dev/null
}
exec /entrypoint.sh
