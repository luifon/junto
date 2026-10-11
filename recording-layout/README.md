# Recording layout

The browser template for the composite recording in ADR 0602. Plain HTML,
JavaScript and CSS; the official LiveKit client and the typeface are served locally.
The page shows the named screen share, cameras and other shares as tiles, and a
camera grid when there is no share. It plays every microphone and only the main
share's audio. Each video requests the smallest published layer that covers its
drawn size, or the largest available layer. Requests change with the layout and
viewport, so large camera-grid tiles can receive higher layers.

## Build

Node.js 22 or later:

```sh
cd recording-layout
npm ci --ignore-scripts
../tools/serial.sh npm run build
```

Serve the contents of `dist/` at one static URL, including its licence files.
Paths are relative, so the directory can be mounted below a server route. The
server route and deployment packaging are implemented separately. No CDN or
font service is used at runtime.

## Recorder interface

Set the room composite request's `custom_base_url` to the directory's URL,
reachable from Egress. Egress adds `url` (LiveKit WebSocket URL), `token` (its
room token) and `layout` as URL query parameters. The server's `layout` is
`main:<screen-share-track-id>` or the empty string for automatic selection.
Never place an API secret in this URL. Keep access logs from retaining tokens.

`UpdateLayout` reaches the page as JSON `{"layout":"main:<track-id>"}` in the
recorder participant's metadata. An empty layout clears the choice. The page
reads the initial metadata and subsequent changes, including after reconnect.
The controller must resend the current track ID after a recorder reconnect or
media-server restart, as ADR 0602 requires. Other participants' metadata, room
metadata and data messages cannot change the layout.

An unavailable named share is not replaced by another share: its sound stops
until that track returns or the controller updates the choice. With no named
share, the page uses each share's first observed server version timestamp,
falling back to local observation order. Muted video is hidden and unsubscribed.
The bottom tile area fits all tiles in rows without scrolling or cropping.
See [ADR 0606](../docs/adr/0606-recording-layout-template.md).

The page logs `START_RECORDING` once all wanted media is ready, or 10 seconds
after joining, whichever comes first. A stalled track cannot block the start.
Only an explicit room closure or participant removal logs `END_RECORDING`.
Connection loss never ends the recording: after SDK retries end, the page rejoins
with pauses growing from 1 to 10 seconds and no attempt limit. The current layout
instruction persists. The server owns stopping a recording through Egress.

## What the server must provide

Serve the built directory, supply the recorder's LiveKit address and room token,
and send the main share through the layout interface. Add the participant's
display name to its join token's `name` claim. The page shows only that name and
shows no label when it is empty; it never labels a participant with an identity.
Adding the display name to server tokens belongs to a later unit.

## Browser tests

Install the dependencies above and have Docker and the Chromium revision for
Playwright 1.58.2 available in the Playwright cache. The test command does not
download a browser. From this directory:

```sh
npm test
JUNTO_LAYOUT_CPU_LOAD=1 npm test
npm run test:mutations
```

`npm test` always runs through `tools/serial.sh`, builds the page, starts one
headless browser with several test pages, and creates a temporary LiveKit server
through `tools/with-test-livekit.sh`. The dedicated container is
`junto-dev-livekit-layout`; its signalling, TCP, UDP and webhook ports are 27880,
27881, 27882 and 27890. Caller-provided `JUNTO_TEST_LIVEKIT_*` variables and
`JUNTO_TEST_WEBHOOK_ADDRESS` override these defaults. The test restarts that server through the same wrapper and
removes it afterwards. Other test containers are not touched. Optional CPU load
starts four low-priority processes after acquiring the serial lock, prints their
PIDs, and stops those exact processes before releasing it.

Publishers generate canvas video and oscillator audio without opening real
capture devices. Browser audio output is muted. Assertions observe actual SDK
subscriptions, decoded video dimensions and requested layers, visible elements,
playing audio with nonzero decoded samples, layout updates, participant rejoin,
recorder reconnect, stalled capture and an outage that outlasts SDK retries. Tests also check that unrelated
layout messages cannot override the server's instruction. An optional
`JUNTO_LAYOUT_SCREENSHOT` path saves a frame for visual inspection.

These tests exercise the browser template and LiveKit, not an Egress process,
encoded output files, real screen capture or speech.

The mutation command temporarily disables readiness checks, preserved layout state,
quality updates and cached share order one at a time. It requires each targeted
browser assertion to fail, then restores the source and rebuilds the page. It also
runs under the serial lock and never runs two browsers at once.
The cached-order test replays a newer track-info version on a real unmute event,
because the tested media server does not advance that version on mute.
