# 0793. Received pictures are drawn once per frame of the display

## Status

Accepted

## Context

A window of the application draws the pictures of a call: the watched share, a small picture of each share, and the cameras (ADR 0738, ADR 0770). The media engine gives the newest picture of each source to the calls of the client, and the window followed each source with one task: every picture that arrived changed what the window holds and asked for a drawing.

A share and a camera give 30 pictures in a second each, and a call has several sources. The window then drew once per picture, which is more often than the display shows a frame. On a machine where one drawing takes longer than the time between two pictures, the thread of the interface always had a picture to draw: the window answered late, and a test on the test platform, which draws at once after each change, did not end.

## Decision

A window has one follower for the pictures of its call. It works in two steps.

- A picture that arrives at a source marks the source. The follower takes the newest picture of each marked source and converts the ones that the window does not hold, on a blocking thread of the runtime of the client (ADR 0730): a small picture of a share is copied into an image of the window, and the picture of the watched share and a camera picture are made into a pixel buffer of Core Video on macOS, or into an image (ADR 0774). Then it asks the window for its next frame (`Window::on_next_frame`). Until that frame has run, further pictures of any source ask for nothing and cause no work on the thread of the interface; one conversion runs at a time.
- The frame puts the converted pictures in place of the ones that the window holds and asks for one drawing. A picture that a newer one replaced before its conversion started is never converted and never drawn. A frame that finds the pictures that the window already holds draws nothing.
- A window that starts to draw the call (the call moves to or from the separate call window) holds no picture of it. It asks the follower for all pictures, which converts them in the same way; the drawing does not convert them.

The pictures of all sources together cause at most one drawing per frame of the display, whatever the number of pictures and of sources. A window that the system does not draw (hidden, or minimized) gets no frame, so it draws no picture and does no work for the pictures on the thread of the interface. Its follower converts at most one set of pictures on the runtime of the client and holds it until the next frame of the window; that frame draws the held set, and the follower then converts the newest pictures for the following frame.

Without a graphics processor the cameras are drawn at a lower rate, as before: after the follower has taken the cameras, it takes them again 200 ms later at the earliest. The pictures of the shares are taken as they arrive in that time, without the cameras.

The test harness of the window:

- The test platform has no display and delivers no frame. The harness delivers one frame to every window in each of its steps (`Window::frame`).
- A wait of the harness has the limit of one step (60 seconds), also the wait for the application to have no work: the harness runs the work of the application piece by piece and fails the test when the application still has work after the limit (`Window::park`). Before, that wait had no limit, and an application that always had work held the test for ever. The functions of gpui that send a click or a key run the work of the application without a limit; the harness calls its own wait before them.

## Consequences

- The work of the interface for pictures has the rate of the display as its bound, on every machine.
- A picture is shown at the next frame after its arrival: up to one frame later than before.
- The image of a picture that a newer one replaced is released from the GPU of every window (ADR 0774).
- A test that needs a picture of a call to be drawn must deliver a frame; the steps of the harness do.
- Test: `calls::received_pictures_cause_one_drawing_per_frame_of_the_display` in the window tests, for the watched share, the small pictures of the shares, the cameras, and the three together: fifty pictures arrive with the work of the application run after each one and nothing is drawn; the frames that follow draw once or twice, with the newest pictures; a frame without a newer picture draws nothing; five pictures between two waits for frames draw at most twice. Every drawing of the window in these steps must be one that pictures asked for. On the test platform of gpui a window is drawn whenever one of its views has changed, without a frame, so a change of the state of the application (an event of the server) also draws it; the test counts these changes and names their number when a drawing that no picture asked for fails it. Before the measurement the window is given frames until it has no work for one: work that the earlier steps left for a frame draws the window in the next frame.
- An earlier form of this test counted all drawings and failed twice in full runs of the window tests on macOS, with two drawings in the first frame; it passed alone. The cause was not reproduced on Linux: 40 bursts of fifty pictures, 24 single pictures after a pause of 0 or 2.6 seconds, and 8 more after a scroll of the wheel, with and without reduced motion, gave no drawing that pictures did not ask for. From the code, the sources of such a drawing in this window are a change of the state of the application, work that a step before the measurement left for the next frame (the earlier form gave no frames before it measured), and the scrollbars and transitions of gpui-base, which ask for a frame while they move: the scrollbars by the clock of the system for 2 seconds after a scroll and a fade of 500 ms, the transitions by the clock of the executor, which does not move in a test. The present form gives the frames first and fails with the count of state changes, so a drawing of another cause is named when it happens.
- Test: `calls::without_a_graphics_processor_the_cameras_are_taken_at_most_once_in_200_ms` in the window tests: with the software video of the test hook, a picture of a camera that arrives within 200 ms of the last taken one waits, a picture of the watched share is taken meanwhile, and the waiting picture is taken at the end of the 200 ms of the clock of the executor; with a graphics processor each newer picture of a camera is taken.
