# 0007. Monitoring and no telemetry

## Status

Accepted

## Context

An administrator needs to know whether a server and the components it depends on are working. The system is self-hosted and stores private conversations, recordings and transcripts. Components and libraries from third parties, model libraries in particular, can contain their own reporting to an external service.

## Decision

Monitoring:

- A health endpoint reports whether the database and LiveKit are reachable.
- The server can export metrics, traces and logs in OpenTelemetry format to an address that the administrator sets.
- The export is off until the administrator configures it.

No telemetry:

- The application sends nothing to its authors or to a third party.
- This rule includes the components of the system. The transcription worker runs without outbound network access. Any reporting that is built into a model library is turned off.
- The client connects only to the servers in its server book and to the project's release page for the update check (ADR 0704).

## Consequences

- The project receives no usage data and no crash reports. Problems reach the project only when a person reports them.
- An administrator who wants metrics, traces or logs runs a collector and sets its address.
- The transcription worker cannot download a model at run time. The models are present before the worker starts.
- The diarization component must run without network access (ADR 0603).
