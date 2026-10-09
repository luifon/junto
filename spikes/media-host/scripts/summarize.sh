#!/bin/sh
# usage: summarize.sh <label>...  — steady-state averages (drops the first 2 and last 3 samples)
here=$(cd "$(dirname "$0")" && pwd); out="$here/../out/m5"
for label in "$@"; do
  host=$(awk 'NR>1{a[NR]=$0} END{n=0; for(i=4;i<=NR-3;i++){split(a[i],f," "); lk+=f[2]; busy+=100-f[7]; st+=f[6]; if(f[2]>mx)mx=f[2]; if(100-f[7]>bmx)bmx=100-f[7]; mem=f[8]; rx+=f[10]; tx+=f[11]; n++} for(i=2;i<=NR;i++){split(a[i],f," "); err+=f[12]; if(f[8]>mm)mm=f[8]; if(av==""||f[9]<av)av=f[9]} printf "lk_cpu avg %.0f%% max %.0f%% | host busy avg %.0f%% max %.0f%% (steal avg %.0f%%) | lk mem max %d MiB, host avail min %d MiB | host rx %.1f tx %.1f Mbit/s | udp buf errors %d", lk/n, mx, busy/n, bmx, st/n, mm, av, rx/n, tx/n, err}' "$out/$label.host.txt")
  mac=$(awk 'NR>1{a[NR]=$0} END{n=0; for(i=4;i<=NR-3;i++){split(a[i],f," "); c+=f[2]; if(f[2]>mx)mx=f[2]; rx+=f[4]; tx+=f[5]; n++} printf "generator cpu avg %.0f%% max %.0f%% of one core | mac rx %.1f tx %.1f Mbit/s", c/n, mx, rx/n, tx/n}' "$out/$label.mac.txt")
  total=$(cat "$out/$label".lk*.txt | grep -E "│ Total  " | sed 's/│/|/g' | awk -F'|' '{split($3,t,"/"); tracks+=t[1]; r=$4; gsub(/ \(.*/,"",r); gsub(/ /,"",r); v=r+0; if(r ~ /kbps/) v=v/1000; rate+=v; split($5,l," "); lost+=l[1]; n++; pct=$5; gsub(/.*\(|%.*/,"",pct); if(pct+0>mx) mx=pct+0; sumpct+=pct} END{if(n==0){print "no summary"; exit} printf "%d generator process(es), tracks %d, bitrate sum %.1f Mbit/s, packets lost %d, loss%% mean %.3f max %.3f", n, tracks, rate, lost, sumpct/n, mx}')
  errors=$(cat "$out/$label".lk*.txt | grep -v "Error │" | grep -ciE "error|failed|timeout")
  echo "== $label"; echo "   $host"; echo "   $mac"; echo "   lk total: $total  (log lines with error/failed/timeout: $errors)"
done
