# TLS entry on port 443 of the media host — design, not applied

Nothing in this directory was run. No DNS record was created and no certificate
was requested. The files use placeholders.

## What the entry does

One process (Caddy with the layer4 module) listens on TCP 443, reads the server
name of each TLS connection, and selects a route:

| Server name | TLS ends at | Forwarded to |
|---|---|---|
| `<TURN_HOSTNAME>` | Caddy | LiveKit TURN, plain TCP, `127.0.0.1:5349` |
| `<LIVEKIT_HOSTNAME>` | Caddy | LiveKit signalling, `127.0.0.1:7880` |
| `<CHAT_HOSTNAME>` | Caddy | chat server on the data host, over the private link |

Files: `caddy.yaml`, `livekit.additions.yaml`, `compose.addition.yaml`.

## Does TURN need TLS passthrough?

No. LiveKit has `turn.external_tls: true` for this case: the entry ends TLS and
LiveKit accepts unencrypted TURN on `tls_port`, and still advertises that port
to clients as TURN over TLS. This is what LiveKit's own generated deployment
does. Passthrough is the other valid form: the route for the TURN name has no
`tls` handler, and LiveKit ends TLS itself with `turn.cert_file` and
`turn.key_file` (or `LIVEKIT_TURN_CERT` / `LIVEKIT_TURN_KEY`). Passthrough needs
a certificate file that LiveKit can read and a restart of LiveKit after each
renewal, so the plan uses `external_tls`.

LiveKit v1.13.9 also has `turn.proxy_protocol`. Its sample configuration says
that when the proxy ends TLS, TURN reports the proxy's address to the client
unless the proxy sends a PROXY protocol header, and that Firefox rejects a
loopback address there. The two commented lines (`proxy_protocol: v2` on the
Caddy route, `proxy_protocol: true` in LiveKit) are the fix. Not verified: that
the caddy-l4 proxy handler option has exactly this name in the build used.

## Certificates

- Three names, three certificates, all obtained and renewed by Caddy
  (`apps.tls.certificates.automate`). Caddy uses ACME with Let's Encrypt, and
  ZeroSSL as the second issuer. Renewal runs inside the Caddy process with no
  timer or script; certificates and the ACME account key are in `./caddy_data`.
- Requirements at issue time and at every renewal: each name has an A record
  (and AAAA, if IPv6 is used) for the media host's public address, and TCP 443
  and TCP 80 are reachable from the internet. LiveKit's VM guide lists port 80
  for "TLS issuance".
- Alternative without port 80: the ACME DNS challenge. It needs a Caddy build
  with the module for the DNS provider and an API token limited to the zone,
  stored in the host's env file. It also works before the A records exist.
- The chat name's certificate is on the media host, not on the data host. The
  path between the two hosts is the private link; the chat server receives
  plain HTTP/1.1 from the entry.

## Firewall rules this needs (not added)

`443/tcp` (entry), `80/tcp` (certificate issue and renewal, unless the DNS
challenge is used), `3478/udp` (TURN over UDP), and the media ports already
open for the spike: `7881/tcp`, `7882/udp`. `7880/tcp` is then closed to the
internet, because signalling goes through 443.

## Open points

- The chat server sees the entry's address as the client address. If it needs
  the real one (rate limits, audit log), the route sends a PROXY header and the
  chat server reads it.
- The spike measured that `lk` (LiveKit Go SDK) does not use the TCP media
  fallback on 7881, and the libwebrtc-based client does. TURN over TLS on 443 is
  the path for networks that allow only 443.
- TURN traffic is relayed through the same single core. Not measured.

## Sources

- LiveKit, "Deploy to a VM": https://docs.livekit.io/home/self-hosting/vm/
  (generated `caddy.yaml`, ports 443 and 80, TURN domain).
- LiveKit, "Deployment": https://docs.livekit.io/home/self-hosting/deployment/
  (separate TURN domain and certificate, TURN/TLS, `tls_port`).
- `livekit/deploy`, `generate/templates/caddy.go` and `docker.go`, commit
  `1a7b369f94` (Apache-2.0): https://github.com/livekit/deploy — `caddy.yaml`
  here is that template with a third route added.
- `livekit/livekit` v1.13.9, `config-sample.yaml`, `turn:` section
  (`external_tls`, `proxy_protocol`):
  https://github.com/livekit/livekit/blob/v1.13.9/config-sample.yaml
