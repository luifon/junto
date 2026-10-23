# 0773. Full-resolution screen shares

## Status

Accepted. Supersedes the two-layer publication policy in ADR 0737.

## Context

Text in a shared screen must remain readable when the bandwidth estimate falls. A degradation preference that maintains resolution does not prevent the media server from selecting a smaller simulcast layer. The full layer may be suspended while the smaller layer remains available.

## Decision

- Publish a screen or window as one full-resolution stream, in the codec of the system (ADR 0737: H.264 of the hardware encoder on macOS, VP8 elsewhere), with simulcast disabled and the explicit `MaintainResolution` degradation preference. Preserve the chosen capture frame rate and the bitrate cap calculated in ADR 0737. There is no lower-resolution encoding to select when the bandwidth estimate falls.
- The viewer still reports drawing bounds, and conversion uses those bounds. Thumbnail and main-picture conversions remain separate. Requests stay ordered for interoperability with peers that publish multiple layers.
- The pinned Rust SDK exposes a maximum bitrate, frame rate, degradation preference and simulcast choices. Its publication options expose no minimum bitrate, bandwidth-estimate override or supported operation to force an inactive full layer to resume. The client does not patch private transport internals.
- A constrained connection can reduce frame cadence or suspend delivery. This policy preserves the dimensions of available frames; it does not promise uninterrupted motion or legible compression at arbitrarily low bandwidth.
- A deterministic test inspects the encodings calculated by the SDK and requires exactly one encoding at scale one. Live tests retain voice while alternating drawing sizes, check source dimensions and final conversion bounds, and exercise the same code under CPU load. CPU load is not a deterministic bandwidth-collapse simulator.

## Consequences

Every viewer receives the full stream even for a thumbnail. This increases bandwidth and decoding cost compared with the smaller simulcast layer; bounded conversion still limits drawing cost. The former live half-layer expectation is replaced by full source dimensions plus final drawing bounds because the publication contract deliberately changed. The ordered-job tests continue to verify request execution order independently of bandwidth or encoder adaptation.
