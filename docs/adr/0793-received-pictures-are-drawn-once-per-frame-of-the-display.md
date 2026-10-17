# 0793. Received pictures are drawn once per frame of the display

## Status

Accepted

## Context

A window of the application draws the pictures of a call: the watched share, a small picture of each share, and the cameras (ADR 0738, ADR 0770). The media engine gives the newest picture of each source to the calls of the client, and the window followed each source with one task: every picture that arrived changed what the window holds and asked for a drawing.

A share and a camera give 30 pictures in a second each, and a call has several sources. The window then drew once per picture, which is more often than the display shows a frame. On a machine where one drawing takes longer than the time between two pictures, the thread of the interface always had a picture to draw: the window answered late, and a test on the test platform, which draws at once after each change, did not end.

## Decision

A window has one follower for the pictures of its call. It works in two steps.

- A picture that arrives at a source asks the window for its next frame (`Window::on_next_frame`) and marks the source. Until that frame has run, further pictures of any source ask for nothing and cause no work on the thread of the interface.
- The frame takes the newest picture of each marked source and asks for one drawing. A picture that a newer one replaced before the frame is never converted and never drawn. A frame that finds the pictures that the window already holds draws nothing.

The pictures of all sources together cause at most one drawing per frame of the display, whatever the number of pictures and of sources. A window that the system does not draw (hidden, or minimized) gets no frame, takes no picture and does no work for the pictures of the call.

Without a graphics processor the cameras are drawn at a lower rate, as before: after the follower has taken the cameras, it takes them again 200 ms later at the earliest. The pictures of the shares are taken as they arrive in that time, without the cameras.

The test harness of the window:

- The test platform has no display and delivers no frame. The harness delivers one frame to every window in each of its steps (`Window::frame`).
- A wait of the harness has the limit of one step (60 seconds), also the wait for the application to have no work: the harness runs the work of the application piece by piece and fails the test when the application still has work after the limit (`Window::park`). Before, that wait had no limit, and an application that always had work held the test for ever. The functions of gpui that send a click or a key run the work of the application without a limit; the harness calls its own wait before them.

## Consequences

- The work of the interface for pictures has the rate of the display as its bound, on every machine.
- A picture is shown at the next frame after its arrival: up to one frame later than before.
- The image of a picture that a newer one replaced is released from the GPU of every window (ADR 0774).
- A test that needs a picture of a call to be drawn must deliver a frame; the steps of the harness do.
- Test: `calls::received_pictures_cause_one_drawing_per_frame_of_the_display` in the window tests: fifty pictures arrive with the work of the application run after each one, no picture asks for a drawing, the application comes to rest each time, one frame draws once with the last picture, and a frame without a newer picture draws nothing. The window counts the drawings that pictures ask for apart from its other drawings, because an event of the server can draw the window at any time; the test also bounds all drawings of the window in that time.
