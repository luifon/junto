#!/usr/bin/env python3
"""Reduce a pw-dump JSON file to a sorted list of stable lines for comparison."""
import json
import sys

TRANSIENT_CLIENTS = {"pw-dump", "pw-cli", "wpctl", "pw-metadata", "pw-link", "pactl"}


def main(path):
    objects = json.load(open(path))
    by_id = {o["id"]: o for o in objects}

    def props(o):
        return (o.get("info") or {}).get("props") or {}

    def node_name(node_id):
        o = by_id.get(node_id)
        return props(o).get("node.name", f"?{node_id}") if o else f"?{node_id}"

    def port_name(port_id):
        o = by_id.get(port_id)
        return props(o).get("port.name", f"?{port_id}") if o else f"?{port_id}"

    lines = []
    for o in objects:
        kind = o["type"].split(":")[-1]
        p = props(o)
        info = o.get("info") or {}
        if kind == "Client":
            if p.get("application.name") in TRANSIENT_CLIENTS:
                continue
            lines.append(f"Client id={o['id']} name={p.get('application.name')} pid={p.get('pipewire.sec.pid')}")
        elif kind == "Node":
            lines.append(
                f"Node id={o['id']} name={p.get('node.name')} class={p.get('media.class')} state={info.get('state')}"
            )
        elif kind == "Port":
            lines.append(
                f"Port id={o['id']} node={node_name(p.get('node.id'))} name={p.get('port.name')} dir={info.get('direction')}"
            )
        elif kind == "Link":
            lines.append(
                "Link id={} {}:{} -> {}:{} state={}".format(
                    o["id"],
                    node_name(info.get("output-node-id")),
                    port_name(info.get("output-port-id")),
                    node_name(info.get("input-node-id")),
                    port_name(info.get("input-port-id")),
                    info.get("state"),
                )
            )
        elif kind == "Module":
            lines.append(f"Module id={o['id']} name={info.get('name')} args={info.get('args')}")
        elif kind == "Device":
            lines.append(f"Device id={o['id']} name={p.get('device.name')}")
        elif kind == "Metadata":
            for entry in o.get("metadata") or []:
                lines.append(
                    f"Metadata {p.get('metadata.name')} subject={entry.get('subject')} "
                    f"key={entry.get('key')} value={json.dumps(entry.get('value'), sort_keys=True)}"
                )
        elif kind == "Factory":
            lines.append(f"Factory id={o['id']} name={info.get('name')}")
        else:
            lines.append(f"{kind} id={o['id']}")
    for line in sorted(lines):
        print(line)


if __name__ == "__main__":
    main(sys.argv[1])
