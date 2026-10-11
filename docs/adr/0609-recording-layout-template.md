# 0609. Recording layout template details

## Status

Accepted

## Context

ADR 0602 requires the server to name the main screen share and the page to request low quality for small tiles. The recorder spike used a fixed tile column that could cut off later tiles. A layout instruction can arrive before its named track, and a media-server restart replaces track identifiers.

## Decision

- The page is built from `recording-layout/` with plain web technology and the official LiveKit client. Its script and font are served with the page, without requests to a third-party asset service. The built script retains the licence notices of its bundled code.
- The page reads `main:<track-id>` from the start URL and reads layout updates from the recorder's own participant metadata. Initial metadata takes precedence over the URL. Room metadata, other participants' metadata and data messages do not control this page, because the server uses the recorder's layout interface.
- An empty layout enables automatic selection. When a named share is unavailable, the page waits for it or another server instruction and plays no share audio. It does not substitute another person's share sound.
- Automatic selection caches the first server version timestamp seen for each share, or the observation time if no timestamp exists. Later changes of that track's timestamp do not reorder it. The pinned library exposes no track start timestamp, so a join or full reconnect after an older share was updated can order it incorrectly until the server names the main share.
- A share occupies the main area. Other video occupies a bottom area of 24 percent of the output height, with enough rows to fit every tile. Without a share, cameras fill a grid. Tiles retain a 16:9 box and contain the full video image.
- Each visible video requests the smallest published layer that covers its drawn image size, or the largest available layer when none is large enough. The page recalculates the request after layout and size changes, including when the main share changes or participants join or leave the grid. Muted video is hidden and unsubscribed until it is unmuted.
- The page uses the call design's dark video surfaces, borders and Atkinson Hyperlegible Next typeface. Labels show the participant's display name only; an empty name has no label. It has no call controls or chat panels.
- The page signals recording start once the wanted tracks are subscribed and initial media is ready, or 10 seconds after joining the room, whichever happens first. A stalled track cannot prevent the start.
- A lost connection never signals recording end. After the library exhausts its retries, the page rejoins with exponential pauses of 1 to 10 seconds and no attempt limit, preserving the current layout instruction. Only a media-server report that the room was closed or this participant was removed signals the end. The application server decides when to stop the recorder.
- The recorder passes the media-server address and the page trusts it.

## Consequences

- Every subscribed video has space in the frame, with smaller tiles as the count grows. Large camera-grid tiles request higher resolution than small camera tiles.
- While an instructed share is missing, microphones continue and other video remains visible, but no other share sound enters the composite.
- The server must serve the built directory, put display names in participant join tokens and keep the recorder URL token out of logs.
- Browser tests can prove template behaviour without an Egress process; encoded composite correctness still requires tests with the recorder.
