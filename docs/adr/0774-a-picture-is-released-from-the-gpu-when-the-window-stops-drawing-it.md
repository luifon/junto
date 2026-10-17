# 0774. A picture is released from the GPU when the window stops drawing it

## Status

Accepted

## Context

A window of the application draws pictures as images of gpui (`RenderImage`): the pictures of a call, the pictures of messages (ADR 0740), the pictures of people (ADR 0783) and the logo of a server (ADR 0799). The first time that a window draws an image, gpui copies its pixels into a texture of the sprite atlas of that window, under the identifier of the image. The copy stays until the image is released with `drop_image`, or until the window closes.

Each picture of a call is a new image with a new identifier: the frame of the display converts the newest picture of each source into an image (ADR 0793). A share of 30 pictures per second gives the window a new image at almost every frame.

`App::drop_image(image, None)` removes the copies from the windows that the application holds at the moment of the call. During the work of a window (a frame of the display, an event, a drawing), gpui takes that window out of the application, so the call skips it. The window replaced the picture of a source in the frame of the display and released the image that it replaced in the same work. The release therefore never reached the window that drew the image: the atlas kept a texture for every picture that the window drew. During a share the GPU memory of the process grew by about 20 MB per second, and it stayed after the share had ended.

## Decision

How a picture of a call reaches the GPU:

- The media engine gives the newest picture of each source (ADR 0793). The follower of the pictures of the window converts the newest picture of each source that has a new one into one image, on a blocking thread of the runtime of the client, and the next frame of the display takes it. A window holds one image per source that it draws: one for the watched share, one small picture for each share, one for each camera.
- The window draws the image with `img`. gpui copies the image into the atlas of the window at the first drawing, and draws the copy until the image is released.
- On macOS a camera picture is a pixel buffer of Core Video, drawn as a surface of gpui (ADR 0701). It does not enter the atlas. At each drawing the renderer makes a Metal texture over the memory of the buffer. The buffer is freed when no picture holds it, so the window keeps no more buffers than it holds pictures.

When the GPU copy is released:

- When the window replaces the image of a source with the image of a newer picture.
- When a source ends: a share stops, a camera is turned off, a person with a share leaves the call.
- When the window stops drawing the call: the call moves to the separate call window (ADR 0772) or back, or the call ends.

One function of the application releases an image: `images::release`. It does not call `drop_image` at once: it asks gpui to call it after the current work of the application (`App::defer`). At that time every window is back in the application, and `drop_image` removes the copies from every window. The crate refuses a direct call of `drop_image` with the `disallowed-methods` rule of clippy, so every release goes through this function.

The same function releases the pictures of messages, of people and the logo of a server, which had the same error when a window released them in its own work.

## Consequences

- The GPU memory of the pictures of a call in a window is bounded by the number of sources that the window draws, whatever the number of pictures and the duration of the call. The memory of a source is released when the source ends.
- A replaced image keeps its GPU copy until the current work of the application ends. The window does not draw it in that time: it holds the newer image.
- Tests:
  - `calls::a_window_holds_gpu_images_only_for_the_pictures_that_it_draws_now` in the window tests, on the test platform of gpui: forty pictures of each source (the watched share, two small pictures, two cameras) are drawn in the main window and then in the separate call window. Each window then has atlas entries only for the images that it holds now, and none after the shares and the cameras end. It runs in the ordinary suite, without a device.
  - The test target `frames` (macOS, with `JUNTO_TEST_FRAMES=1`): two native windows drawn by Metal, in a call with the media server of the tests, both sharing the synthetic screen and showing the synthetic camera of both people. The windows get the frames of a display for 20 seconds with the call in the main window and then in the separate call window. The program reads the graphics memory of the process with `footprint` and fails when it grows by more than 48 MiB in that time, or stays more than 48 MiB above its value from before the shares after they have ended.
