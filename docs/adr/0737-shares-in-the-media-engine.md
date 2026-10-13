# 0737. Shares in the media engine: the tracks of a share, the watched share and its sound

## Status

Accepted

Open: the call library (`livekit` 0.9.4) sends a sound track with one channel. It has no setting for stereo when it uses two peer connections, and its mode with one peer connection did not give two channels either: the request that adds a track does not state stereo. The sound of a share therefore arrives as the mean of its two channels. The pipeline of the client is stereo up to the call library.

Open: when the estimate of the upload rate of a sharer is below what both layers need, the call library sends only the layer of half the size, and a viewer who watches the share gets that layer: small text is not readable. The degradation preference does not decide this. It was measured on a machine with a busy processor and no limit of the network: the estimate fell from 1.4 Mbit/s to 35 kbit/s within one second, and the library stated the reason "bandwidth". A sharer with a muted microphone got the full layer back after 8 seconds. A sharer who also sent a voice did not get it back in 60 seconds: the estimate rose to 158 kbit/s and stayed there. The full layer of a screen of 1280 by 720 was not sent at an estimate of 272 kbit/s and was sent again at 682 kbit/s. The call library gives the client no access to the layers of a track that is published. Two other sets of layers were sent correctly by the call library in a test and are not decided: one layer of the full size, and two layers of the full size at 5 pictures per second and at the chosen frame rate.

Open: after a viewer chooses another share, the full size can arrive late. In seven runs of the live test in the Linux container, with the two layers that a share had then, the full size at the full frame rate began in the first second six times. Once the viewer got 16 pictures of the full size in the first second, none for six seconds, and the full rate from the eighth second on. The cause is not established. The media server decided which layer it sent to a viewer; since ADR 0773 a share has one encoding of the full size, and the measurement was not repeated on Linux with it before the two changes were joined.

## Context

ADR 0502 decides that several people can share at once, that each viewer picks the share to watch and hears the sound of that share, that a viewer can turn the sound of a share off or change its volume and keep the picture, and that shared sound is its own track, at a higher bitrate and without noise suppression and echo cancellation. ADR 0510 decides that the server shows a share from the report of the sharing device while the media server lists its picture track, and that a microphone block closes the sound of a share. ADR 0602 lets the layout page of the recorder ask for the quality layer of the size at which a picture is drawn. The engine carries one media connection (ADR 0733), and the calls of a client join it with the rules of a server (ADR 0734).

## Decision

The two-layer publication policy below is superseded by ADR 0773. Capture ownership, audio and ordered viewer requests remain as described here.

The share of this device:

- The picture is one video track with the source "screen share", sent with VP8 in two layers: the full size at the frame rate that the sharer chose, and half the size at no more than 15 pictures per second. The bitrate is 3 Mbit/s for 1920 by 1080 at 30 pictures per second, in proportion to the pixels and, for half of it, to the frame rate, between 150 kbit/s and 8 Mbit/s. The defaults of the call library are not used: they send a share of less than 1920 pixels at 15 or at 5 pictures per second. When the processor of the sharer is busy, the call library lowers the frame rate and keeps the size of the picture: the client states the degradation preference "maintain resolution" itself and does not rely on the default of the call library for a screen share. The sound is one audio track with the source "screen share audio": 48 kHz, two channels, 128 kbit/s, no processing, no silence suppression and no redundant copy.
- The name of the picture track states what is shared and at which frame rate: `screen/30` or `window/30`. The record of a share on the server holds only its sound, and a viewer shows both. A track with another name states nothing. The title of a shared window is sent to nobody.
- The share runs when the capture has started and the picture track is published. The calls then report it to the server with its sound choice, and report its end when it stops (ADR 0510). Two reports of the share are at least 1 second apart, so that they and the reports of the own state (ADR 0734) stay within the 30 reports per 10 seconds of the server. A report that failed is sent again after 3 seconds.
- The start of the capture belongs to the share. A share that the person stops while the system still prepares its capture ends that work, and what the system had started is stopped.
- When the call library connects anew, it withdraws each track of the device and publishes it again under another identifier. A track of the share that was withdrawn without a request of the share is waited for, for 10 seconds; the capture goes on. Only the track with the identifier that the share sent returns to it. A picture track that does not return ends the share. A sound track that does not return is published anew when the person may send it. A track that returns for a share that was stopped or replaced is withdrawn. The microphone track is handled in the same way (ADR 0733).
- While the capture gives no picture, the last picture is sent again once a second. A screen that does not change sends nothing otherwise, and a person who starts to watch would get no picture.
- The sound track is published while the share runs with a sound and the permission of the participant allows the source. A microphone block of the server withdraws it; the picture stays, and the sharer is told that the sound is closed. Without the block it is published again.
- A share ends when the person stops it, when the capture ends by itself, when the person leaves the call, and when the media connection is lost for good. A new share replaces one that runs. A share does not start again by itself.
- A capture that fails to start, and a share that ended by itself, are told with their reason until the person starts a share or puts the statement away.

The shares of the other people:

- A share exists for a viewer when the picture track of a person with an account is received. The shares are kept in the order in which they were received.
- The viewer watches one share: the one that the viewer chose while it runs; otherwise the share of another person that was received first; otherwise the own share. A share that starts does not replace the share of another person that the viewer watches. It replaces the preview of the own share, unless the viewer chose the preview.
- The picture of the watched share is converted to BGRA at the size at which the interface draws it, at most 30 times a second; a picture that arrives earlier waits, so the last picture of a share is always drawn. A small picture of every share is made once a second. The interface tells the engine the size of its drawing in pixels.
- The engine asks the media server for each picture in the layer of its drawn size: the watched share at the size of the interface, every other share at the size of its small picture. It asks when the size for a share changes and not otherwise; a change of the share volume asks for nothing. The requests reach the media server in the order in which the engine made them. The call library sends each request from a task that it starts for that request, and it gives no handle to wait for: on a runtime with several threads two requests that are made at the same moment arrive in any order, and the media server keeps the size that arrived last. The engine therefore makes these requests on one thread of the process, whose runtime runs its tasks in the order in which they were started. A request that panics does not end that thread. Without the thread, which the system can refuse, a request is made from the caller and the order is not kept; the engine logs that.
- After a viewer chooses another share, the live test counts the pictures of the full size in each second from the change, prints the counts in every run, and fails when three seconds in a row with at least 66 pictures do not begin within 20 seconds. A layer of half the size, which a peer with the two-layer policy publishes, gives at most 45 pictures in three seconds.
- The sound of a share is played only while its share is watched, with the share volume of the viewer (0 to 200 %, where 0 keeps the picture without the sound). The sound track of every other share is received and has no volume in the mix. The engine does not end and start subscriptions with the choice of the viewer: the call library sends each such change by itself, in no fixed order, and a sound track is a small part of what a share costs. The volume of a sound is set in the mix at once, also for what the mix already holds.
- The sharer gets the pictures of the own share as a preview, at most 15 times a second, and never its sound.
- The level of a person and the speaking mark are measured from the microphone track only.

Sound of a share before the call library:

- The capture gives blocks of any size at its own rate. They become frames of 10 ms at 48 kHz with two channels: one channel is sent on both, and of more than two the first two. Up to half a second waits when the media connection takes the frames late; a further frame is dropped.

## Consequences

- A viewer of two shares receives two picture tracks, one of them in its small layer, and both sound tracks, of which one is played.
- A change of the watched share draws the last received picture of the new share at once.
- A person whose microphone is blocked shares pictures without sound, whatever client is used (ADR 0510).
- The sound of a share is sent while the person has muted the own microphone: the two are separate tracks.
- A share that ended because the media connection was lost is started again by the person after the call has returned.
- The recorder receives the share tracks as any participant does (ADR 0602).
- The preview of a shared screen that shows the client draws the client inside itself.
- The order of the requests for a size depends on how the call library sends a request (`livekit` 0.9.4: one task for each request, and every wait on the way to the connection serves the tasks in the order of their arrival). It is checked again with each new version of the call library; the test of the last size of a drawing fails when the order is lost.
