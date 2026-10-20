# 0007. Monitoring and no telemetry

## Status

Accepted

## Context

An administrator needs to know whether a server and the components it depends on are working. The system is self-hosted and stores private conversations, recordings and transcripts. Components and libraries from third parties, model libraries in particular, can contain their own reporting to an external service.

## Decision

Monitoring:

- A health endpoint reports whether the database and LiveKit are reachable.
- The server can export metrics and traces in OpenTelemetry format to an address that the administrator sets. Logs go to standard error (ADR 0008).
- The export is off until the administrator configures it.

Server export:

- `JUNTO_OTEL_ENDPOINT` is an optional HTTP or HTTPS base URL. Unset or empty means
  no exporter, background export thread or monitoring connection. A URL with
  credentials, a query or a fragment is refused without printing its value.
- Traces and metrics use OTLP/HTTP JSON at `<base>/v1/traces` and
  `<base>/v1/metrics`. HTTP reuses the application's transport dependencies, and
  JSON permits inspection of the exported data without a protocol decoder.
  HTTPS checks the system trust store. The exporter follows no redirects and uses
  no proxy, so data goes only to the configured destination.
- The server image carries the CA bundle and its copyright notice from its
  existing Debian build stage, pinned by the Debian snapshot, for HTTPS collectors.
- OpenTelemetry API and SDK 0.31.0, OTLP 0.31.1 and its HTTP client reqwest 0.12.28
  are pinned. This set shares the TLS dependencies of the client and needs no
  additional system TLS library in the server image.
- The lock uses rustls-webpki 0.103.16 to reject distrusted intermediate public
  keys (RUSTSEC-2026-0382).
- Every completed HTTP response produces one server span named `HTTP request`,
  an increment of the cumulative counter `http.server.request.count` (unit
  `{request}`), and an observation of `http.server.request.duration` (unit `s`).
  Duration ends at response headers, before a streamed body or an upgraded
  connection ends. Status codes 500 to 599 mark the span as an error.
- The duration histogram has boundaries at 0.005, 0.01, 0.025, 0.05, 0.075, 0.1,
  0.25, 0.5, 0.75, 1, 2.5, 5 and 10 seconds.
- Span and metric attributes are limited to `http.request.method`,
  `http.route` and `http.response.status_code`. The method is a standard HTTP
  method or `_OTHER`; the route is the router's template or `unmatched`. The only
  resource attribute is the constant `service.name=junto-server`. No environment
  resource detector runs.
- There is no exported content, name, account or message identifier, raw path,
  query, header, key, network address, log event or incoming trace context.
  Request spans are created directly, independently of the logging subscriber,
  so other libraries and application log fields cannot add data to them.
  Logs continue to use standard error and owner codes use standard output
  (ADR 0008); forwarding logs is outside this exporter.
- Export runs on SDK background threads every five seconds, or when a span batch
  reaches 256. The queue holds at most 1024 finished spans and drops further spans
  without waiting. Each metric has a cardinality limit of 4096, with an overflow
  series for additional combinations; that series has only the fixed attribute
  `otel.metric.overflow=true`. Requests update aggregates without waiting for
  the network.
- Each export attempt has a two-second timeout and no retry. Failed span batches
  are lost; metrics are cumulative and are collected again at the next interval.
  Graceful shutdown attempts to flush both providers off the asynchronous runtime
  threads. An unavailable collector does not prevent serving requests.
- Compose passes the optional endpoint to the server. It installs no collector:
  the administrator runs one separately and makes it reachable from the server.
- Unit tests use an in-process HTTP collector to inspect both signals, aggregation,
  privacy and behavior with a blocked collector and full queue. An API test checks
  the running server and its final export on shutdown.

No telemetry:

- The application sends nothing to its authors or to a third party.
- This rule includes the components of the system. The transcription worker runs without outbound network access. Any reporting that is built into a model library is turned off.
- The client connects only to the servers in its server book and to the project's release page for the update check (ADR 0704).

## Consequences

- The project receives no usage data and no crash reports. Problems reach the project only when a person reports them.
- An administrator who wants metrics or traces runs a collector and sets its address. A collector can collect standard-error logs separately.
- The transcription worker cannot download a model at run time. The models are present before the worker starts.
- The diarization component must run without network access (ADR 0603).
- ONNX Runtime, which the transcription worker uses for voice detection, contains a telemetry client that is on by default from version 1.29. The worker pins version 1.28.0, which does not contain it, and sets the variable that turns the client off (ADR 0610). With both in place, and with no network in the container, the transcription chain made no socket call for an Internet address.
- The recorder sends requests to public STUN servers of third parties when it connects to a room, and this conflicts with the rule above. The servers are a default of the media server, which gives them to every client when its configuration names no STUN server. The deployment names a server of its own in `rtc.stun_servers` in every form (ADR 0020). In the local form the log of the recorder named two public servers without the setting, and only the named server with it. The browser of the recorder and the desktop client were not observed.
