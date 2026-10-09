"""Compares packets the server sent with packets the Mac's interface received during one run."""
import re
import sys

out = sys.argv[1]
for label in sys.argv[2:]:
    def metric(when, name, **want):
        total = 0.0
        for line in open(f"{out}/{label}.metrics-{when}.txt"):
            m = re.match(r"(\w+)\{(.*)\} ([\d.e+]+)$", line.strip())
            if m and m.group(1) == name:
                labels = dict(re.findall(r'(\w+)="([^"]*)"', m.group(2)))
                if all(labels.get(k) == v for k, v in want.items()):
                    total += float(m.group(3))
        return total

    def counter(when, pattern):
        return int(re.search(pattern, open(f"{out}/{label}.drops-{when}.txt").read()).group(1))

    def delta(fn, *a, **k):
        return fn("after", *a, **k) - fn("before", *a, **k)

    sent_initial = delta(metric, "livekit_packet_total", direction="outgoing", transmission="initial")
    sent_rtx = delta(metric, "livekit_packet_total", direction="outgoing", transmission="retransmit")
    sent_pad = delta(metric, "livekit_packet_total", direction="outgoing", transmission="padding")
    recv = delta(metric, "livekit_packet_total", direction="incoming", transmission="initial")
    nacks = delta(metric, "livekit_nack_total", direction="outgoing")
    host_tx = delta(counter, r"tx_packets=(\d+)")
    host_rx = delta(counter, r"rx_packets=(\d+)")
    mac_in = delta(counter, r"mac iface packets in/out: (\d+)")
    mac_out = delta(counter, r"mac iface packets in/out: \d+ (\d+)")
    mac_sockdrop = delta(counter, r"(\d+) dropped due to full socket buffers")
    qdrop = delta(counter, r"\(dropped (\d+)")
    print(
        f"{label}: server RTP out initial={sent_initial:.0f} retransmit={sent_rtx:.0f} padding={sent_pad:.0f}, RTP in={recv:.0f}, "
        f"NACKs received from subscribers={nacks:.0f} | host interface tx={host_tx} rx={host_rx}, qdisc dropped={qdrop} | "
        f"Mac interface in={mac_in} out={mac_out}, Mac UDP socket-buffer drops={mac_sockdrop}"
    )
