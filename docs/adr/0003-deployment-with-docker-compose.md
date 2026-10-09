# 0003. Deployment with Docker Compose in two forms

## Status

Accepted, pending step 0

## Context

The components of ADR 0001 run on hardware that the operator controls. PostgreSQL and Redis must not be reachable from the public network. The media server needs a public address. Some operators have one host with a public address; other operators keep the data on a host that cannot receive inbound connections and add a second host that has a public address. LiveKit Egress needs about 4 CPU cores while it records, which a small public host may not have.

## Decision

- The system is deployed with Docker Compose, with a separate container per component.
- PostgreSQL runs on an internal Docker network with no published port. Only the server container is on that network.
- Redis accepts connections only from the private network and requires a password.
- Two forms are documented, each with its own Compose file:
  1. **One host with a public address.** This is the default form. All components run on that host.
  2. **Two hosts.** The data host runs the server, PostgreSQL, the file storage, the recorder and the transcription worker. The public host runs the LiveKit server, Redis and the public entry on port 443. A private network link joins the two hosts.
- In the two-host form, the data host opens the private link and keeps it alive, because only the public host can receive connections.
- In the two-host form, the public host accepts HTTPS for the server and forwards it over the private link. The server listens on a local port.
- In the two-host form, LiveKit is configured with its public address.
- The recorder runs on the data host. It connects out to the LiveKit server and to Redis over the private link. The recorder can therefore run on a host that cannot receive inbound connections.

## Consequences

- In the two-host form the private link is required from the first version, because the recorder uses it.
- In the two-host form the server is unavailable while the network connection of the data host is down.
- In the two-host form no call admission check runs while the private link is down (ADR 0506).
- The host that runs the recorder needs about 4 free CPU cores while a recording runs.
- The capacity of a media host is not known in advance. Published LiveKit benchmarks contain no measurements for screen sharing and do not describe a host that also carries HTTPS, TURN and the private link. Step 0 measures the capacity of the media host with test participants while a recording runs, and tests the recorder on the data host connected to the media server on the public host (`docs/step-0/README.md`, tests 3 and 6).
