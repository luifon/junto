#!/bin/sh
# Runs one X7 measurement: the test script in the tools container on the data
# host, and at the same time the sampler on the public host. Run on the Mac.
#   x7-run.sh ROOM SAMPLE_SECONDS x7_cost.py ROOM
#   x7-run.sh rth-load-LABEL SAMPLE_SECONDS x7_load.py LABEL a40|b16 rec|norec [LAYOUT]
# JUNTO_TEST_MEDIA_HOST is the SSH name of the public host, JUNTO_TEST_DATA_HOST of the data host.
set -u
public_ssh="${JUNTO_TEST_MEDIA_HOST:?set the SSH name of the public host}"
data_ssh="${JUNTO_TEST_DATA_HOST:?set the SSH name of the data host}"
here=$(cd "$(dirname "$0")/.." && pwd)
room=$1; seconds=$2; shift 2
mkdir -p "$here/out/$room"
ssh "$public_ssh" "sh /opt/junto-spike-link/sample.sh $seconds 5" > "$here/out/$room/public-host.txt" 2>&1 &
ssh "$data_ssh" "docker exec junto-spike-rec-ctl python3 /work/scripts/rooms.py" > "$here/out/$room/rooms-before.txt" 2>&1
ssh "$data_ssh" "docker exec junto-spike-rec-ctl sh -c 'cd /work/scripts && python3 $*'" > "$here/out/$room/run.txt" 2>&1
wait
rsync -a --exclude '*.mp4' --exclude '*.ts' --exclude '*.ogg' --exclude 'egress.log' "$data_ssh:/srv/junto-spike-rec/out/$room/" "$here/out/$room/"
python3 "$here/scripts/x7_summary.py" "$room"
