# 0773. Full-resolution screen shares

## Status

Accepted. Supersedes the two-layer publication policy in ADR 0737.

## Context

Text in a shared screen must remain readable when the bandwidth estimate falls. A degradation preference that maintains resolution does not prevent the media server from selecting a smaller simulcast layer. The full layer may be suspended while the smaller layer remains available.

## Decision

- Publish a screen or window as one full-resolution stream, in the codec of the system (ADR 0737: H.264 of the hardware encoder on macOS within the limits of level 5.2, VP8 elsewhere), with simulcast disabled and the explicit `MaintainResolution` degradation preference. Preserve the chosen capture frame rate and the bitrate cap calculated in ADR 0737. There is no lower-resolution encoding to select when the bandwidth estimate falls.
- A share larger than the limits of level 5.2 of H.264 (36,864 blocks of 16 × 16 pixels, such as 4096 × 2304; 2,073,600 blocks per second; 543 blocks on a side) is sent in VP8 by the software encoder at its full size, also on macOS. It is not made smaller to stay within H.264: a share of a 5K display (5120 × 2880) scaled into level 5.2 would be 4096 × 2304, 80 % of its size in each direction, and its text would become less readable, which is what this ADR prevents. The cost above the limit is the cost of every share before the hardware encoder: the software encoder at the full size. Test: `engine::sent::tests::a_picture_beyond_the_levels_of_h264_is_encoded_in_vp8_at_its_size` (every system); the media live test `share::a_share_larger_than_the_levels_of_h264_arrives_at_its_size_in_vp8` sends a share of 5120 × 2880 and checks that it arrives at that size in VP8.
- The viewer still reports drawing bounds, and conversion uses those bounds. Thumbnail and main-picture conversions remain separate. Requests stay ordered for interoperability with peers that publish multiple layers.
- The pinned Rust SDK exposes a maximum bitrate, frame rate, degradation preference and simulcast choices. Its publication options expose no minimum bitrate, bandwidth-estimate override or supported operation to force an inactive full layer to resume. The client does not patch private transport internals.
- A constrained connection can reduce frame cadence or suspend delivery. This policy preserves the dimensions of available frames; it does not promise uninterrupted motion or legible compression at arbitrarily low bandwidth.
- A deterministic test inspects the encodings calculated by the SDK and requires exactly one encoding at scale one. Live tests retain voice while alternating drawing sizes, check source dimensions and final conversion bounds, and exercise the same code under CPU load. CPU load is not a deterministic bandwidth-collapse simulator.

## Consequences

Every viewer receives the full stream even for a thumbnail. This increases bandwidth and decoding cost compared with the smaller simulcast layer; bounded conversion still limits drawing cost. The former live half-layer expectation is replaced by full source dimensions plus final drawing bounds because the publication contract deliberately changed. The ordered-job tests continue to verify request execution order independently of bandwidth or encoder adaptation.
