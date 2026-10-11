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
- ONNX Runtime, which the transcription worker uses for voice detection, contains a telemetry client that is on by default from version 1.29. The worker pins version 1.28.0, which does not contain it, and sets the variable that turns the client off (ADR 0610). With both in place, and with no network in the container, the transcription chain made no socket call for an Internet address.
- The recorder sends requests to public STUN servers of third parties when it connects to a room, and this conflicts with the rule above. The servers are a default of the media server, which gives them to every client when its configuration names no STUN server. The deployment names a server of its own in `rtc.stun_servers` in every form (ADR 0020). In the local form the log of the recorder named two public servers without the setting, and only the named server with it. The browser of the recorder and the desktop client were not observed.
