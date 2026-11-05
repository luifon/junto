# 0773. Full-resolution screen shares

## Status

Accepted. Supersedes the two-layer publication policy in ADR 0737.

## Context

Text in a shared screen must remain readable when the bandwidth estimate falls. A degradation preference that maintains resolution does not prevent the media server from selecting a smaller simulcast layer. The full layer may be suspended while the smaller layer remains available.

## Decision

- Publish a screen or window as one full-resolution stream, in the codec of the system (ADR 0737: H.264 of the hardware encoder on macOS within level 3.1, the level that the media server answers; VP8 elsewhere), with simulcast disabled and the explicit `MaintainResolution` degradation preference. Preserve the chosen capture frame rate and the bitrate cap calculated in ADR 0737. There is no lower-resolution encoding to select when the bandwidth estimate falls.
- A share larger than level 3.1 of H.264 (3,600 blocks of 16 × 16 pixels, such as 1280 × 720; 108,000 blocks per second; 169 blocks on a side) is sent in VP8 by the software encoder at its full size, also on macOS. It is not made smaller to stay within H.264: a share of 1920 × 1080 scaled into level 3.1 would be 1280 × 720, two thirds of its size in each direction, and its text would become less readable, which is what this ADR prevents. The cost above the limit is the cost of every share before the hardware encoder: the software encoder at the full size. Test: `engine::sent::tests::a_picture_beyond_the_negotiated_level_of_h264_is_encoded_in_vp8_at_its_size` (every system); the media live test `share::a_share_larger_than_the_levels_of_h264_arrives_at_its_size_in_vp8` sends a share of 5120 × 2880 and checks that it arrives at that size in VP8.
- A share keeps the codec that the size of its pictures allows. When the pictures change size, such as a shared window that grows beyond level 3.1 on macOS, and the size changes the encoder that the rule above chooses, the engine publishes the picture track again from the same source, under the same name and slot, in the codec of the new size. The new track is placed in the track permissions with the share before the earlier track is withdrawn, so a viewer always holds a track of the share: it keeps watching the share and keeps its place among the shares, lists it once while both tracks exist, and keeps hearing its sound, whose track is not changed. The recording follows: the server names the current picture track of the slot as the main share at its next pass (ADR 0602). Test: the media live test `share::a_window_that_grows_beyond_the_level_of_h264_is_published_again_in_vp8` (macOS) and the server test `recordings_control::a_share_whose_picture_track_is_published_again_stays_the_main_share`.
- The viewer still reports drawing bounds, and conversion uses those bounds. Thumbnail and main-picture conversions remain separate. Requests stay ordered for interoperability with peers that publish multiple layers.
- The pinned Rust SDK exposes a maximum bitrate, frame rate, degradation preference and simulcast choices. Its publication options expose no minimum bitrate, bandwidth-estimate override or supported operation to force an inactive full layer to resume. The client does not patch private transport internals.
- A constrained connection can reduce frame cadence or suspend delivery. This policy preserves the dimensions of available frames; it does not promise uninterrupted motion or legible compression at arbitrarily low bandwidth.
- A deterministic test inspects the encodings calculated by the SDK and requires exactly one encoding at scale one. Live tests retain voice while alternating drawing sizes, check source dimensions and final conversion bounds, and exercise the same code under CPU load. CPU load is not a deterministic bandwidth-collapse simulator.

## Consequences

Every viewer receives the full stream even for a thumbnail. This increases bandwidth and decoding cost compared with the smaller simulcast layer; bounded conversion still limits drawing cost. The former live half-layer expectation is replaced by full source dimensions plus final drawing bounds because the publication contract deliberately changed. The ordered-job tests continue to verify request execution order independently of bandwidth or encoder adaptation.
