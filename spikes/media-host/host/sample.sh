#!/bin/sh
# usage: sample.sh <seconds> <interval>
total=${1:-60}; step=${2:-5}
cid=$(docker inspect -f '{{.Id}}' junto-spike-livekit)
cg=/sys/fs/cgroup/system.slice/docker-$cid.scope
iface=$(ip -o -4 route get 1.1.1.1 | sed -n 's/.* dev \([^ ]*\).*/\1/p')
read_all() {
  lk_usec=$(awk '/^usage_usec/{print $2}' $cg/cpu.stat)
  set -- $(awk '/^cpu /{print $2+$3, $4, $5+$6, $7+$8, $9, $2+$3+$4+$5+$6+$7+$8+$9}' /proc/stat)
  c_user=$1; c_sys=$2; c_idle=$3; c_irq=$4; c_steal=$5; c_total=$6
  rx=$(cat /sys/class/net/$iface/statistics/rx_bytes); tx=$(cat /sys/class/net/$iface/statistics/tx_bytes)
  udp_err=$(awk '/^Udp:/{n++; if(n==2) print $6+$7}' /proc/net/snmp)
  udp6_err=$(awk '/Udp6RcvbufErrors|Udp6SndbufErrors/{s+=$2} END{print s+0}' /proc/net/snmp6)
  now=$(date +%s.%N)
}
echo "time lk_cpu% host_user% host_sys% host_softirq% host_steal% host_idle% lk_mem_MiB host_avail_MiB rx_Mbit tx_Mbit udp_buf_errors"
read_all
n=0
while [ $n -lt $total ]; do
  p_lk=$lk_usec; p_user=$c_user; p_sys=$c_sys; p_idle=$c_idle; p_irq=$c_irq; p_steal=$c_steal; p_total=$c_total; p_rx=$rx; p_tx=$tx; p_err=$((udp_err+udp6_err)); p_now=$now
  sleep $step; n=$((n+step))
  read_all
  awk -v lk=$((lk_usec-p_lk)) -v dt=$(echo "$now $p_now" | awk '{print $1-$2}') \
      -v u=$((c_user-p_user)) -v s=$((c_sys-p_sys)) -v i=$((c_idle-p_idle)) -v q=$((c_irq-p_irq)) -v st=$((c_steal-p_steal)) -v t=$((c_total-p_total)) \
      -v rx=$((rx-p_rx)) -v tx=$((tx-p_tx)) -v err=$((udp_err+udp6_err-p_err)) \
      -v mem=$(cat $cg/memory.current) -v avail=$(awk '/MemAvailable/{print $2}' /proc/meminfo) -v ts=$(date +%H:%M:%S) \
      'BEGIN{printf "%s %.0f %.0f %.0f %.0f %.0f %.0f %.0f %.0f %.1f %.1f %d\n", ts, lk/dt/10000, 100*u/t, 100*s/t, 100*q/t, 100*st/t, 100*i/t, mem/1048576, avail/1024, rx*8/dt/1e6, tx*8/dt/1e6, err}'
done
