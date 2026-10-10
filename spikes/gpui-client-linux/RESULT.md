# Spike: gpui client on Linux (step 0, test 2, Linux)

Date: 2026-10-09. Test machine: a Linux desktop, reached over SSH. openSUSE Tumbleweed 20260826, kernel 7.2.0, glibc 2.43, an x86-64 CPU with 8 cores and 16 threads, 32 GB, an NVIDIA GPU with 8 GB and driver 580.178.04. Desktop session: KDE Plasma 6.7.5 on X11, KWin with compositing, display `:0` at 1920×1080, 96 dpi.

Two facts about this machine decide how to read the numbers:

- The X server of the session uses the `dummy` video driver (the machine has no monitor). It has no DRI3 and no NVIDIA GLX extension. KWin composites with llvmpipe (software). The NVIDIA GPU does not drive the display.
- Other tests used the machine at the same time. The 1-minute load average was between 2.5 and 25 during this spike. Each measurement below states the load.

## Question

Does the application of `spikes/gpui-client/` (gates C1 to C12, proved on macOS) build and work on a Linux desktop with the same dependency set, and what differs?

## Result

| Gate | Result | Against macOS |
|---|---|---|
| C1 Dependency set and build | PASS | Same crates. The lock file needs 29 more packages. Build needs 5 Debian packages. Binary 66.5 MB (macOS 32.9). |
| C2 Composer | PARTIAL | Same as macOS for typing, paste, undo, selection, dead keys. Differences: no system emoji picker, no primary selection, no input method in the session to test. |
| C3 Rich text | PASS | Same as macOS. Difference: a selection is not written to the X11 primary selection. |
| C4 Long list | PARTIAL | Function same as macOS. Frame interval 40 to 152 ms in the three configurations this machine allows (macOS 8.3 ms). None of them is a display driven by the GPU. |
| C5 Layout | PASS | Same as macOS. |
| C6 Interface scale | PASS | Same as macOS. The desktop scale is taken from `Xft.dpi` on X11 and from the output scale on Wayland; 150 % observed on both. |
| C7 Drag and drop | PARTIAL | (b) in-application drag: same as macOS. (a) file drop from another application on X11: FAIL as observed. |
| C8 Second window | PASS | Same as macOS, except "always on top": `WindowKind::Floating` is not above other applications on X11 and is not kept above on Wayland. |
| C9 Video tiles | PARTIAL | No surface path. Image path at 1280×720: 1 tile 30 fps, 4 tiles 22 to 24 fps, 9 tiles 11 fps on the GPU path. At 640×360: 9 tiles at 30 fps. |
| C10 Notifications | PASS | Banner, action click and body click observed. No bundle and no permission prompt. The same tag does not replace; dismiss does nothing. |
| C11 Accessibility | recorded | Same tree content as macOS through AT-SPI. The tree is published only when the session's accessibility status is enabled. |
| C12 Keyboard | PASS | Same as macOS with Ctrl in place of Cmd. |
| L-A Wayland | PASS for C1, C2, C5, C8 | Ran under a headless labwc compositor in a container, with the GPU and with software rendering. |
| L-B Secret storage | PASS | `ksecretd` provides the Secret Service. Store, read and delete in 0.115 s, no prompt. |
| L-C Packaging | recorded | Three small libraries can be bundled; the C library, graphics drivers and desktop services come from the host. |

Every statement below is marked **observed** (run and seen on the test machine), **source** (read in an Apache-2.0 or MIT crate, not run) or **not verified**.

## Build recipe

`Containerfile.build` (Debian 12, glibc 2.36, older than the host's 2.43):

```dockerfile
FROM docker.io/library/rust:1.98.1-slim-bookworm
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      libfontconfig1-dev \
      libxcb1-dev \
      libxkbcommon-dev \
      libxkbcommon-x11-dev \
 && rm -rf /var/lib/apt/lists/*
ENV CARGO_HOME=/work/cargo CARGO_TARGET_DIR=/work/target
WORKDIR /work/src
```

Commands, on the test machine, with the source copied to `/srv/junto-spike-gpui/src`:

```sh
docker build -t junto-spike-gpui-build:1 -f Containerfile.build .
docker run -d --name junto-spike-gpui-build --cpus 4 --memory 8g \
  -v /srv/junto-spike-gpui:/work junto-spike-gpui-build:1 sleep infinity
docker exec junto-spike-gpui-build bash -c 'cd /work/src && cargo build --release --locked'
cp /srv/junto-spike-gpui/target/release/spike /srv/junto-spike-gpui/run/junto-gpui-spike
```

How the package list was found — **observed**: the build started with only `pkg-config`. It failed first in the build script of `yeslogic-fontconfig-sys` (no `fontconfig.pc`), then at the link step (`unable to find library -lxcb`, `-lxkbcommon`, `-lxkbcommon-x11`). With the five packages a clean build from a new image passed. Not needed: cmake, clang, Wayland development files, Vulkan headers, OpenSSL.

`Containerfile.tools` (openSUSE Tumbleweed with labwc, grim, wtype, wlrctl, Xvfb, xdotool, AT-SPI and the Python binding) is the image for the Wayland and AT-SPI tests. Two containers ran from it:

```sh
docker run -d --name junto-spike-gpui-tools --cpus 4 --memory 8g --shm-size 1g \
  -v /srv/junto-spike-gpui:/work -v /usr/share/fonts:/usr/share/fonts:ro junto-spike-gpui-tools:1 sleep infinity
docker run -d --name junto-spike-gpui-gpu --cpus 4 --memory 8g --shm-size 1g \
  --runtime nvidia -e NVIDIA_VISIBLE_DEVICES=all -e NVIDIA_DRIVER_CAPABILITIES=all --device /dev/dri \
  -v /srv/junto-spike-gpui:/work -v /usr/share/fonts:/usr/share/fonts:ro junto-spike-gpui-tools:1 sleep infinity
```

Neither container received the X11 socket or the session bus of the user. The `nvidia` container runtime was already configured on the host.

## Method

The same application, one screen per gate. Three places to run it:

1. **The user's X11 session** (as the desktop user, found with `loginctl`): the binary runs directly on the host. `scripts/desk.sh` starts it, takes screenshots with `import`, and sends input with `xdotool` (XTEST). Every key or click command first checks that the active window belongs to the spike process, and every click checks the window under the pointer. `scripts/remote.sh <command>` runs it from the Mac; `scripts/fetch.sh` copies screenshots to `out/`. Both read the SSH name of the test machine from the environment variable `JUNTO_TEST_LINUX_HOST`.
2. **A headless Wayland compositor in a container** (`scripts/container/wl-up.sh`, `wl-run.sh`, `wl.sh`): labwc 0.20.2 (wlroots 0.20.2) with the headless backend. In the GPU container its renderer is GLES2 on the NVIDIA device; in the other container it is pixman (software). Screenshots with `grim`, keys with `wtype`, pointer with `wlrctl`.
3. **A private X server in a container** (`scripts/container/x-up.sh`, `x-run.sh`): Xvfb with a private D-Bus session and an AT-SPI bus started with accessibility enabled. Used for C11 and for the `Xft.dpi` test.

The host tools used: `xdotool`, `import` and `magick` (ImageMagick), `xprop`, `wmctrl`, `xclip`, `busctl`, `gdbus`, `eu-stack`, and `python3` with the GTK 3 introspection binding. Nothing was installed on the host.

Screenshots, benchmark files, logs and the AT-SPI dumps are in `out/` (not tracked). Files from the X11 session are in `out/` and `out/x11/`; files from the Wayland GPU container are in `out/wayland-gpu/` and have the prefix `wl-` or `wlgpu-`.

## Linux-specific changes to the source

`spikes/gpui-client/` was not modified. The copy here differs in these points:

| File | Change | Reason |
|---|---|---|
| `Cargo.toml` | `gpui-pre-platform` gets the features `x11` and `wayland` | The platform crate depends on `gpui-pre-linux` with `default-features = false`; without these features no window backend is compiled. |
| `Cargo.toml` | `core-video` and `core-foundation` moved to `[target.'cfg(target_os = "macos")'.dependencies]` | They are Apple-only. |
| `Cargo.toml` | `oo7` 0.6 as a Linux dependency and a second binary `secret_test` | Gate L-B. `oo7` (MIT) is already in the tree: `gpui-pre-linux` uses it for `cx.write_credentials`. |
| `Cargo.lock` | 841 → 870 packages | The two features enable optional dependencies that the macOS lock file did not contain. See C1. |
| `src/main.rs` | Key bindings `cmd-…` → `secondary-…` | `cmd` is the Super key on Linux. `secondary` is Cmd on macOS and Ctrl on Linux. |
| `src/video.rs` | The surface path (types, buffers, benchmark rows) is under `#[cfg(target_os = "macos")]`; on other systems the benchmark has the three image rows | `gpui::surface` accepts only `CVPixelBuffer` and exists only on macOS and iOS (**source**, `gpui-pre/src/elements/surface.rs`). |
| `src/video.rs` | Frame size from the environment variable `SPIKE_VIDEO_SIZE` (default `1280x720`); the mark sizes follow the width | To measure smaller frames in C9. At 1280 wide the counter cells are 49 px instead of 48. |
| `src/shell.rs` | The status line also shows `window.scale_factor()` and the viewport size | Gate C6, desktop scale. |
| `src/bin/secret_test.rs` | New | Gate L-B. |
| `scripts/` | New scripts for Linux. The macOS Swift tools and the scripts for the macOS desktop automation tool were not copied. | Different test tools. |

Not changed, and visible on Linux: two elements name the font family `Menlo`, which Linux does not have; the text is drawn with a fallback proportional font (**observed**, the `value=…` line in `c2-01-two-lines.png`). Labels still say "Cmd+K" and "Finder".

**observed** The changed source still compiles for macOS: `../../tools/serial.sh cargo check --locked` on the Mac, 1 min 19 s, no error. **not verified** The changed source was not run on macOS.

## C1 Dependency set and build — PASS

Dependency set:

- **observed** The same published crates: `gpui-pre` 0.3.8, `gpui-pre-platform` 0.3.8, `gpui-pre-reqwest-client` 0.3.8, `gpui-component` 0.7.1, `gpui-base` 0.7.1, `gpui-kit-assets` 0.7.1. On Linux the platform code is `gpui-pre-linux` 0.3.8 and the renderer is `gpui-pre-wgpu` 0.3.8 with `wgpu` 29.0.4. Both are Apache-2.0.
- **observed** `cargo fetch --locked` with the macOS lock file fails on Linux: "cannot update the lock file". Without `--locked`, cargo added 29 packages at the newest compatible versions: `ahash`, `as-raw-xcb-connection`, `calloop-wayland-source`, `const-random`, `const-random-macro`, `downcast-rs`, `filedescriptor`, `gethostname`, `hashbrown` 0.14.5, `is-docker`, `is-wsl`, `open`, `scoped-tls`, `tiny-keccak`, `wayland-backend`, `wayland-client`, `wayland-cursor`, `wayland-protocols`, `wayland-protocols-plasma`, `wayland-protocols-wlr`, `wayland-scanner`, `x11rb`, `x11rb-protocol`, `xcursor`, `xim-ctext`, `xim-parser`, `xkbcommon`, `xkeysym`, `zed-xim`. The lock file in this directory has all 870 and works with `--locked` on both systems.
- **observed** Licence field of all 643 packages downloaded for the Linux target (`scripts/container/licences.sh`, `out/licences-linux.txt`): no GPL-only package. One is MPL-2.0 (`option-ext`). One offers GPL-2.0 only as an alternative to Apache-2.0 (`self_cell`).
- **observed** `gpui-pre-platform/Cargo.toml` has `[package.metadata.gpui-pre] zed-rev = "279fe070bb389b79652e52065b2f001edcc0b11b"`, and its description names the same revision. The macOS result says the crate carries no commit hash; that statement is wrong for this crate.
- Other Linux crates pulled in by gpui: `cosmic-text` 0.19 and `fontdb` (text), `ash` (Vulkan), `x11rb`, `xkbcommon`, `zed-xim` (X input method client), `wayland-client`, `ashpd` (portals), `notify-rust` (notifications), `oo7` (secrets), `accesskit_unix` 0.22 with `atspi` (accessibility).

System libraries and tools:

| When | Needed | Host had it |
|---|---|---|
| Build | Rust 1.98.1 with its linker driver `cc`; `pkg-config`; development files of fontconfig, libxcb, libxkbcommon, libxkbcommon-x11 | In the container only. Nothing built on the host. |
| Run, linked (`readelf -d`) | `libxcb.so.1`, `libxkbcommon.so.0`, `libxkbcommon-x11.so.0`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` (highest symbol version GLIBC_2.35) | Yes, all. `ldd` on the host resolved every entry. |
| Run, loaded by name | `libvulkan.so.1` and through it the driver files (`libGLX_nvidia`, `libnvidia-glcore`, `libvulkan_lvp` with `libLLVM`); `libEGL.so.1` for the OpenGL backend; `libwayland-client.so.0` under Wayland | Yes. |
| Run, data | fontconfig configuration and font directories, xkb data, compose tables, cursor themes, the CA certificate bundle | Yes. |

**observed** The binary ran on the host without installing a package. The complete list of the 65 shared objects the process mapped in the X11 session, each with its host package, is in `out/c1-runtime-libraries.txt`; the 63 mapped under Wayland are in `out/c1-runtime-libraries-wayland.txt`. `libfontconfig` and `libfreetype` are not in either list: the development package is needed by a build script, and the binary does not load the library.

Graphics backend:

- **source** The renderer creates a `wgpu` instance with the Vulkan and OpenGL backends, sorts the adapters (discrete GPU, integrated GPU, other, virtual, CPU; Vulkan before OpenGL) and takes the first that can configure the window surface (`gpui-pre-wgpu/src/wgpu_context.rs`).
- **observed** In the X11 session it listed three adapters and selected the Vulkan adapter of the NVIDIA GPU. It runs on this driver: the GPU memory in use rose from 1 MiB to 86 MiB while the spike ran, and in the Wayland run `nvidia-smi` listed the spike process.
- **observed** In this session each frame reaches the X server through `xcb_put_image`: six stack samples of the main thread during scrolling (`eu-stack`) showed `xcb_put_image → xcb_send_request → writev` four times and the event loop twice. The X server has no DRI3, so the driver copies each finished frame through the X socket. This is a property of the dummy X server, not of gpui.
- **observed** With `VK_DRIVER_FILES` set to the lavapipe file only, gpui selected `llvmpipe (Vulkan)` and ran. Software rendering works without a change.
- **observed** Under the Wayland compositor with the GPU, gpui selected the NVIDIA Vulkan adapter and presented without copying through a socket.

Measured:

| Item | Linux | macOS | How |
|---|---|---|---|
| Clean release build | 9 min 33 s wall, 34 min 56 s user CPU, 591 crates, 4 CPUs of a busy machine | 199 s wall, 951 s user CPU, 8 cores | new image from `Containerfile.build`, empty target directory, `time cargo build --release --locked`; sources already downloaded |
| Rebuild after a change in the spike crate | 45 s | 10–12 s | cargo output |
| Binary | 66.5 MB (48.5 MB after `strip`) | 32.9 MB (25.2 MB) | `ls -l` |
| Target directory after one release build | 2.1 GB | 1.8 GB | `du -sh` |
| Idle window, composer screen, X11 session, NVIDIA | 237 MB RSS: 64 MB anonymous, 173 MB file-backed (mapped driver libraries); Pss 225 MB | 88 MB RSS | `/proc/<pid>/status`, `smaps_rollup`, about 1 minute after start |
| Same, Wayland with GPU | 195 MB RSS: 55 MB anonymous | | same |
| Idle CPU, composer screen (cursor blink), Wayland with GPU | 1.2 % of one core | 1 % | `/proc/<pid>/stat` over 10 s |
| Idle CPU, composer screen, X11 session, NVIDIA | 13 % of one core | | same; each blink frame is copied with `xcb_put_image` |
| Start until the window's refresh timer runs | 0.5 to 1.2 s, of which the adapter test takes 0.25 s | not measured | time stamps in the run logs |

**observed** The binary from the clean build started in the X11 session (`c1-clean-build-binary.png`). The gates were run with a binary from an earlier incremental build of the same source.

## C2 Composer — PARTIAL

| Item | X11 session | Wayland | Evidence |
|---|---|---|---|
| Typing, Enter sends, Shift+Enter adds a line | observed, same as macOS | observed | `c2-sheet-1.png`, events `PressEnter shift=true`, `sent "First line\nsecond line after Shift+Enter"`; `wl-c2-sheet.png` |
| Paste of 7 lines from the clipboard (Ctrl+V), word wrap, growth to 6 rows, scrollbar | observed, same as macOS, including the view that stays at the top after a long paste | observed, clipboard set with `wl-copy` | same sheets |
| Undo (Ctrl+Z) | observed | observed | `c2-sheet-1.png` |
| Selection by keyboard (Shift+Left, Ctrl+Shift+Left) | observed | observed | `c2-sheet-1.png` |
| Selection by mouse drag across lines, and click then Shift+click | observed | not verified (the pointer tool cannot hold a button) | `c2-sheet-2.png` |
| Dead keys of the layout in use (`us`, variant `intl`): ´e ~n ~a ^o ´c `a ¨u | observed: `éñãôçàü`. `´` then `c` gives `ç`, which is the compose table of the session's `LC_CTYPE=pt_BR.UTF-8` | observed with a virtual keyboard: `éñãôç` | `c2-sheet-2.png`, `wl-c2-sheet.png` |
| Pending dead key drawn | observed: `´` is shown and is part of `value()` until the next key | observed | `c2-09-dead-key-pending.png` |
| Compose key | not verified: the layout has no Compose key, and the test did not change the keyboard configuration | observed: Compose ' a gives `á` | `wl-c2-sheet.png` |
| Input method | none is running: `XMODIFIERS=@im=local`, `GTK_IM_MODULE=cedilla`, `QT_IM_MODULE=xim`, no IBus or Fcitx process. gpui logged `zed_xim: Try connect local`. Dead keys are composed by gpui with libxkbcommon. | none in the container | session environment, run log |
| System emoji picker | not available: the button calls `Window::show_character_palette()`, nothing opens | nothing opens | `c2-sheet-3.png`; **source**: the Linux platform code has no implementation |
| Primary selection | A selection is not written to PRIMARY, and a middle click does not paste | `wl-paste -p`: "Nothing is copied" | `xclip -o -selection primary` kept its earlier content |

Differences from macOS:

- **observed** No system emoji picker on Linux.
- **observed** No primary selection. **source**: gpui has `write_to_primary` and `read_from_primary` for X11 and Wayland; `gpui-base` and `gpui-component` 0.7.1 do not call them.
- **observed** On this headless X server gpui logs `Found no xinput mouse pointers` at start. Pointer input through XTEST worked.

Not verified: a full input method (same as macOS), the Compose key on X11.

## C3 Rich text — PASS

| Item | Result | Evidence |
|---|---|---|
| Bold, italic, strikethrough, inline code, code blocks with syntax colours, quote, lists, task list, mention chip | observed, same as macOS | `c3-01-rich.png` |
| Emoji | observed: 🎉 👍🏽 🇧🇷 in colour (Noto Color Emoji from the host); ❤️ is drawn as a black text glyph | `c3-01-rich.png` |
| Link click | observed | event `link clicked https://example.org/docs` |
| Link hover | not observed, same as macOS (a capture does not show the cursor) | |
| Image from a file, from HTTP, from HTTPS | observed. The HTTPS URL answers with a redirect, and the image was shown. **source**: the binary reads the host's CA bundle. | `c3-07-block-images.png`, log lines `http server: GET /sample.png -> 200` |
| Animated GIF, file and HTTP | observed: three captures show three frames | `c3-gif-sheet.png` |
| Mouse selection across lines, Ctrl+C | observed: text on the X11 CLIPBOARD | output of `xclip -o -selection clipboard` |
| Selection across messages, Ctrl+C | observed, same shape as macOS: author names are not copied | `c3-05-selection-across-messages.png`, `c3-copied-across-messages.txt` |
| Primary selection | not written | `xclip -o -selection primary` unchanged |
| Copied text after the application exits | observed: still on the clipboard. Klipper, the session's clipboard manager, was running. | `xclip -o` after `stop` |

## C4 Long list — PARTIAL

Function — **observed**, same as macOS: 20,000 rows, opens at the bottom (`#19999`), the wheel scrolls (`c4-00-start.png`). Generating the rows took 29.1 ms (macOS 12.7 ms).

Frame times of the same benchmark (600 frames per phase, interval between `on_next_frame` callbacks, ms). Files in `out/x11/` and `out/wayland-gpu/`.

| Configuration | Load | up 30 px: mean / p95 / max | up 400 px | down 400 px | `Window::draw` p50 / p95 | Process CPU |
|---|---|---|---|---|---|---|
| macOS (Apple Silicon, 120 Hz) | | 8.33 / 9.03 / 11.59 | 8.33 / 9.22 / 9.36 | 8.51 / 9.32 / 18.48 | 3.35 / 4.30 | 50 % |
| X11 session, NVIDIA Vulkan, frames copied with `xcb_put_image` | 13–25 | 145.1 / 200.1 / 300.1 | 125.2 / 200.0 / 416.8 | 152.6 / 349.9 / 466.8 | 11.19 / 28.79 | 94 % |
| X11 session, llvmpipe (software) | 22 | 59.0 / 100.0 / 166.6 | 56.3 / 83.6 / 166.7 | 47.7 / 66.8 / 99.9 | 14.00 / 35.42 | 311 % |
| Wayland, headless labwc on the GPU, NVIDIA Vulkan, container with 4 CPUs | 3 | 44.9 / 49.3 / 50.0 | 40.0 / 49.4 / 50.0 | 40.1 / 49.6 / 50.0 | 10.97 / 15.55 | 32 % |
| Same, output set to 120 Hz | 4 | 45.4 / 49.0 / 50.0 | 38.7 / 49.2 / 50.0 | 41.5 / 49.2 / 50.0 | 10.55 / 17.55 | 33 % |

Memory (RSS): 224 MB with the list at the bottom and 265 MB after the benchmark in the X11 session with NVIDIA; 228 MB and 277 MB with llvmpipe; 248 MB after the benchmark under Wayland. macOS: 116 MB and 141 MB physical footprint.

What the numbers say:

- **observed** No configuration on this machine reached 60 frames per second.
- **observed** In the X11 session the limit is the copy of each frame to the X server (stack samples, C1).
- **observed** Under the headless Wayland compositor the process used 32 % of one core, so it waited most of the time. The interval did not change when the output went from 60 Hz to 120 Hz, and its maximum is 50.0 ms in every phase. The cause of this pacing was not found. A headless compositor has no display; this interval is not a measurement of a desktop.
- **observed** The part that transfers to other machines is the CPU time: `Window::draw` p50 is about 11 ms on this CPU at low load, against 3.35 ms on the Mac. The same CPU generated the rows 2.3 times slower than the Mac, so about 1.4 times of the difference is not explained by the CPU (an estimate from that one workload).
- **not verified** The frame interval on a Linux session whose display is driven by the GPU. This machine cannot provide one without a change to its X server.

## C5 Layout — PASS

All items **observed** in the X11 session, same as macOS; the Wayland column was observed in the GPU container.

| Item | X11 | Wayland | Evidence |
|---|---|---|---|
| Three columns, right panel, font from files (slashed zero), tabular numerals | observed | observed | `c5-01-layout.png`, `wl-c5-01-layout.png` |
| Rail collapses to icons | observed | not run | `c5-08-rail-collapsed.png` |
| Tooltip on a library button and on a plain element | observed | observed | `c5-sheet-1.png`, `c5-sheet-2.png`, `wl-c5-sheet.png` |
| Hover menu, item chosen | observed, by keyboard: `action menu: Studio` | observed, by pointer | events |
| Context menu (right click), item chosen | observed, by keyboard: `action context: Reply in thread` | observed, by pointer | `c5-05-context-menu.png`, events |
| Modal dialog with an input, Enter confirms, Escape cancels | observed: `join dialog confirmed with "harbor.example:7443"`, `join dialog cancelled` | observed: confirmed with `arbor.example:7443`; the first key came before the click had given the input focus | `c5-06-dialog.png`, events |

- The window has the title bar of the window manager (KWin on X11, labwc on Wayland). On macOS gpui draws into the title bar area.
- **observed** The dialog's input does not have focus when the dialog opens; a click on it was needed before typing. This is in the spike code and is not specific to Linux.
- **not verified** Menu items chosen by keyboard under Wayland: each call of the virtual keyboard tool makes the window lose keyboard focus, and the menu closes.
- **not verified** A compositor that draws no title bar for the client (GNOME). The client would have to draw its own.

## C6 Interface scale — PASS

| Item | Result | Evidence |
|---|---|---|
| 150 % with the button | observed: rem 24 px, same as macOS | `c6-03-scale-150.png` |
| 90 % with Ctrl+- four times | observed: rem 14.4 px | `c6-01-scale-90.png`, events `interface scale set to 125% … 90%` |
| Text and controls grow together, layout holds | observed, same as macOS | same captures |

Desktop scale:

- **observed** The session has `Xft.dpi: 96`; gpui logged `Using scale factor from Xft.dpi: 1`. The desktop scale is 100 %.
- **source** Order on X11: the environment variable `GPUI_X11_SCALE_FACTOR` (a number or `randr`), then the X resource `Xft.dpi` divided by 96, then RandR. XSETTINGS is not read (`gpui-pre-linux/src/linux/x11/client.rs`, a `TODO` says so).
- **observed** `GPUI_X11_SCALE_FACTOR=1.5` in the X11 session: window scale factor 1.5, rem 16 px, text sharp. The window asked for 1320×860 logical pixels, which is 1980×1290 device pixels on a 1920×1080 screen; KWin gave it 1920×1006 (`c6-10-desktop-scale-150.png`).
- **observed** `Xft.dpi: 144` on the private X server: `Using scale factor from Xft.dpi: 1.5`.
- **observed** Wayland output scale 1.5: window scale factor 1.5, text sharp (`wl-c6-10-output-scale-150.png`).
- **not verified** A change of the desktop scale while the application runs.

## C7 Drag and drop — PARTIAL

| Item | Result | Evidence |
|---|---|---|
| (b) Drag a person to another voice channel | observed, same as macOS | `c7-sheet-1.png`, event `Bruno moved to Focus room by drag` |
| (b) Same move through a menu | observed | event `Ana moved to HR by menu` |
| (a) Files dropped from another application, X11 | FAIL as observed | below |
| (a) Under Wayland | not verified: the pointer tool cannot hold a button | |

(a) No file manager was running in the session. The source of the drag was a GTK 3 window (`scripts/drag_source.py`, run with the host's Python) that offers two files as `text/uri-list`. `xdotool` pressed the button on it, moved the pointer in 40 steps onto the drop area and released. Three runs, same result:

- **observed** The GTK side reported `drag began`, then one data request for the target `text/uri-list`, which it answered with both `file://` URIs, then `drag ended`.
- **observed** gpui logged `activate is not implemented on Linux` at the moment of each drop. **source**: gpui calls `cx.activate(true)` when it handles the platform's drop event, so the drop message reached gpui.
- **observed** The drop area did not show its drag-over style during the hover, the `on_drop::<ExternalPaths>` handler did not run, and the screen stayed at "0 paths received" (`c7-06-file-drag-over.png`, `c7-07-file-dropped.png`).
- **not verified** The cause. **source**: `gpui-pre-linux` handles `XdndEnter`, `XdndPosition`, `XdndDrop` and the selection reply, and the element code looks correct, so the fault was not found by reading. No second drag source was available on the host to rule out the GTK tool, and the tool did not start in the container.

The spike does not prove that a file can be dropped on a gpui window on X11.

## C8 Second window — PASS

| Item | X11 | Wayland | Evidence |
|---|---|---|---|
| Open a second window from the first | observed | observed | `c8-02-call-window.png`, `wl-c8-02-call-window.png` |
| Both update from shared state; mute toggled in the call window shows in both | observed | observed | counters 77/77 and 9/9 in the captures; event `mute toggled from the call window` |
| Return by button, from either window | observed from the main window | observed from the call window | events `call returned to the main window`, `call window released` |
| Close by the window manager's close request | observed (`wmctrl -c`) | observed (`wlrctl toplevel close`) | event `call window released` |

Differences from macOS:

- **observed** The requested position of the second window (780, 70) was not used; KWin placed it at (630, 366).
- **observed** The windows have no `WM_CLASS` and no icon (`xprop`). The task bar shows a generic icon for each. The spike does not set `WindowOptions::app_id`.

Always on top:

- **observed** X11: `WindowKind::Floating` sets `WM_TRANSIENT_FOR` to the main window and nothing else; `_NET_WM_STATE` has no `_NET_WM_STATE_ABOVE` (`xprop`). The stacking order (`_NET_CLIENT_LIST_STACKING`) keeps the call window above the main window after the main window is activated. A window of another program, once activated, is above the call window (`scripts/stacking.sh`).
- **observed** X11: after the standard request `_NET_WM_STATE` add `_NET_WM_STATE_ABOVE` (sent with `wmctrl -b add,above`), KWin set the state and kept the call window above the active window of the other program (`c8-08-above-other-app.png`). gpui has no call that sends this request (**source**). The request can be sent at any time, so a pin toggle is possible on X11 without a new window.
- **observed** Wayland (labwc): after a click on the main window the floating call window is behind it (`wl-c8-05-floating-main-clicked.png`). **not verified** other compositors. The core Wayland protocols have no request by which a normal window asks to stay on top.

## C9 Video tiles — PARTIAL

Linux has only the image path: a new BGRA `RenderImage` per frame and per tile, and `window.drop_image` for the old one. The benchmark is the same as on macOS: 8 s after 2 s of warm-up, "shown" is frames drawn per tile, CPU is the whole process as a share of one core and includes frame generation.

1280×720 at 30 fps:

| Tiles | macOS image path | X11 session, NVIDIA, `xcb_put_image` (load 2–7) | X11 session, llvmpipe (load 8–11) | Wayland with GPU, two runs (load 10–11) |
|---|---|---|---|---|
| 1 | 30.0 fps, 12 % | 10.5 fps, 97 % | 30.0 fps, 624 % | 30.0 fps, 30 % and 32 % |
| 4 | 30.0 fps, 22 % | 8.5 fps, 112 % | 20.0 fps, 505 % | 24.3 and 21.6 fps, 107 % and 108 % |
| 9 | 30.0 fps, 43 % | 6.2 fps, 137 % | 11.7 fps, 388 % | 11.4 and 11.1 fps, 166 % and 143 % |

Smaller frames, Wayland with GPU (load 5–6):

| Tiles | 640×360 | 320×180 |
|---|---|---|
| 1 | 30.0 fps, 15 % | 30.0 fps, 14 % |
| 4 | 30.0 fps, 32 % | 30.0 fps, 27 % |
| 9 | 30.0 fps, 53 % | 30.0 fps, 31 % |

- **observed** Frame generation alone (one copy of the frame and the marks, on its own thread) cost 5 %, 30–34 % and 62–79 % of one core for 1, 4 and 9 tiles at 1280×720 under Wayland, against 1 %, 3 % and 6 % on macOS. This machine moves memory several times slower than the Mac while other tests run.
- **observed** Peak RSS with 9 tiles at 1280×720: 410–427 MB under Wayland, 580 MB in the X11 session with NVIDIA, 612 MB with llvmpipe (macOS 199 MB). At 640×360: 232 MB.
- **observed** In the X11 session the rate is limited by the frame copy to the X server, as in C4, so that column says nothing about a normal desktop.
- Evidence: `c9-01-img-1.png`, `c9-03-img-9-llvmpipe.png`, `wlgpu-c9-01-img-9.png`, `out/x11/c9-bench-*.txt`, `out/wayland-gpu/c9-bench-*.txt`.

Answer for the product: on this 2018 desktop one tile of 1280×720 holds 30 fps at about a third of one core. Four and nine tiles of 1280×720 do not hold 30 fps. Nine tiles of 640×360 hold 30 fps at about half of one core. In a 3×3 grid on a 1320-pixel window each tile is about 430 pixels wide, so frames of 640×360 lose nothing visible.

**not verified** GPU load. **not verified** A machine with an integrated GPU.

## C10 Notifications — PASS

- **observed** A notification service is running: `org.freedesktop.Notifications` is Plasma 6.7.5, specification 1.2, with the capability `actions`. Notifications were not inhibited.
- **source** gpui shows notifications with `notify-rust` over D-Bus (`gpui-pre-linux/src/linux/system_notifications.rs`).

| Item | Result | Evidence |
|---|---|---|
| Banner on screen with application name, title, body, two actions | observed | `c10-02-popup.png`, `c10-03-popup-hover.png` |
| Action click comes back | observed | event `action 'reply' received for tag 'junto-spike-mention'` |
| Body click comes back | observed | event `body clicked for tag 'junto-spike-mention'` |
| Application bundle or permission prompt | not needed, none appeared | |
| Same tag replaces the earlier notification | no: two posts 1 s apart gave two popups | `c10-sheet-2.png` |
| `dismiss_system_notification` | **source**: empty on Linux. Not distinguishable by observation: the popups close by themselves after about 5 s. | |

The clicks went to the popup of the spike's own notification, which is a window of plasmashell. `scripts/notification_click.sh` clicks only when the window under the pointer has the window type notification, and the popup was checked in a capture first.

Limits: the gpui type has no sound, image, urgency or replace field. The response arrives only while the process runs; each shown notification keeps one thread waiting for its action (**source**).

## C11 Accessibility — recorded

- **observed** The user's session has `org.a11y.Status IsEnabled = false` and `ScreenReaderEnabled = false`. In that session no run of the spike logged `Accessibility activated`: gpui publishes no tree until the session enables accessibility. The test did not change the status, because that is a setting of the user.
- **observed** On the private X server with an AT-SPI bus started with `--a11y=1`, gpui logged `Accessibility activated` at 0.18 s and registered as toolkit `AccessKit`. The tree was read with the Atspi introspection binding (`scripts/atspi_tool.py`), `out/c11-atspi-tree.txt`.

Content, compared with the macOS tree — **observed**, the same nodes:

| macOS | AT-SPI on Linux |
|---|---|
| `AXButton` with title, `AXPress` | `button` with name, action `click` |
| `AXCheckBox` subrole `AXSwitch`, value | `toggle button` with the state `pressed` |
| `AXCheckBox` | `check box` |
| `AXTextField` with title | `entry` with name, states `focusable editable` |
| heading with a text child | `heading` with a `label` child |
| list with items | `list` with `list item` children |
| group with title | `panel` with name |

- **observed** The action `click` through AT-SPI runs the handler: events `mute button pressed, muted=true` and `noise filter false`.
- **observed** The long list screen exposes its buttons only; no message text (`out/c11-atspi-tree-long-list.txt`). Same gap as macOS.
- **not verified** A screen reader (Orca): it speaks.

## C12 Keyboard — PASS

All **observed** in the X11 session with `scripts/keyboard_sequence.sh`; evidence `c12-sheet-1.png` and the events.

| Item | Result |
|---|---|
| Tab order | First → Second → input → Third → plain focusable element; Shift+Tab goes back to Third. Each control was activated after the Tab that reached it: events `button activated: First`, `Second`, `Third`, `raw div activated`, `Third`. |
| Focus ring | visible on the button, the input and the plain element |
| Text typed into the input reached by Tab | `typed after tab` |
| Space and Enter activate the focused control | both used |
| Shortcut bound to an action (Ctrl+K) | events `action ran, count 1` and `count 2` |
| Keyboard path to the hover menu by shortcut (Ctrl+Shift+M) | event `server menu opened by keyboard shortcut`; arrows and Enter chose `Client Alpha` |
| Keyboard path by Tab then Space | arrows and Enter chose `Studio` |

Difference from macOS: the bindings must be written with `secondary-`. **not verified** whether a binding with the Super key reaches the application under KWin.

## L-A Wayland — PASS for C1, C2, C5, C8

No Wayland compositor was started in the user's session. labwc ran with the headless backend inside the two containers. The same binary was used; with `WAYLAND_DISPLAY` set and `DISPLAY` unset gpui chose its Wayland backend.

| Gate | Result | Evidence |
|---|---|---|
| C1 | observed with the GPU (NVIDIA Vulkan, compositor renders with GLES2 on the same GPU) and in software (llvmpipe, compositor renders with pixman). Idle 195 MB RSS, 1.2 % CPU. | `wlgpu-c4-00.png`, `wlsoft-c5-01-layout.png`, run logs |
| C2 | observed: typing, Shift+Enter, paste from the Wayland clipboard, undo, keyboard selection, copy to the Wayland clipboard, dead keys, Compose key. Not written: primary selection. Not available: emoji picker. | `wl-c2-sheet.png` |
| C5 | observed: layout, both tooltips, hover menu, context menu, dialog | `wl-c5-01-layout.png`, `wl-c5-sheet.png` |
| C8 | observed: second window, shared state, return, close. A floating window is not kept above. | `wl-c8-02-call-window.png`, `wl-c8-05-floating-main-clicked.png` |
| C6, desktop scale | observed at output scale 1.5 | `wl-c6-10-output-scale-150.png` |

Not verified under Wayland: KWin and GNOME as compositors, a compositor without server-side title bars, an input method, drag and drop, mouse-drag selection.

## L-B Secret storage — PASS

- **observed** A Secret Service provider is running in the session: the bus name `org.freedesktop.secrets` is owned by `ksecretd` (KDE). One collection, `kdewallet`, is the default alias and was unlocked.
- **observed** `src/bin/secret_test.rs` uses `oo7` 0.6 (MIT), the crate gpui uses for its own credentials call. Run as the desktop user:

```
[  0.099 s] backend: Secret Service over D-Bus
collection locked before unlock: false
[  0.100 s] unlocked
[  0.111 s] stored
items found: 1
read back: label="Junto gpui spike test secret" matches=true
[  0.112 s] read
[  0.115 s] deleted
items left after delete: 0
```

- **observed** No prompt: the list of windows on the desktop was the same before and after the run.
- **observed** The test secret is removed: the collection had 5 items before and 5 after, and a search for the test attribute returns no item.
- **not verified** A locked collection (the provider would show a prompt) and a session without a provider. The fallback with a key file was not shown, because a provider is running.

## L-C Packaging facts

From `readelf -d`, `ldd` and the list of mapped libraries.

An AppImage can bundle:

- `libxcb.so.1`, `libxkbcommon.so.0`, `libxkbcommon-x11.so.0`, and what they need: `libxcb-xkb.so.1`, `libXau.so.6`, `libXdmcp.so.6`, and from a Debian build also `libbsd.so.0` and `libmd.so.0`.
- The interface fonts (the spike loads its typeface from files).

Must come from the host:

- The C library (`libc.so.6`, `libm.so.6`, loader) and `libgcc_s.so.1`. A build on Debian 12 needs glibc 2.35 or newer on the host. An older build base is needed for older distributions.
- `libvulkan.so.1` and the graphics driver it loads (NVIDIA, Mesa, or Mesa lavapipe for software); `libEGL.so.1` and its driver for the OpenGL backend. A driver must match the kernel module of the machine, so it cannot be bundled.
- `libwayland-client.so.0` under Wayland.
- Data read at run time: fontconfig configuration (`/etc/fonts`) and system fonts, including a colour emoji font; xkb data and compose tables (`/usr/share/X11`); cursor themes; the CA certificate bundle (the binary names `/etc/ssl/…` and `/etc/pki/…` paths and the variables `SSL_CERT_FILE`, `SSL_CERT_DIR`).
- Session services over D-Bus: notifications, Secret Service, desktop portal (gpui reads settings from it), AT-SPI.

Not needed: `libfontconfig`, `libfreetype`, GTK, Qt, OpenSSL.

**not verified** An AppImage was not built.

## What the product must do differently on Linux

1. **Video.** Use the image path, and give gpui frames at the size of the tile, not at the size of the stream: 1280×720 only for one tile, at most 640×360 per tile for four or more. Detect a software adapter (`llvmpipe`) and reduce or stop video and animation there.
2. **Frame rate.** Measure scrolling on a Linux machine whose display is driven by its GPU before a frame-rate promise is made. This spike has only CPU cost: about 11 ms of `draw` per frame of the long list on a 2018 desktop CPU.
3. **File attachments.** Do not depend on a file drop. Provide a file chooser, and test the drop from Dolphin and Nautilus on X11 and Wayland on a real desktop.
4. **Always on top.** On X11, send `_NET_WM_STATE_ABOVE` through the raw window handle. On Wayland there is no general way; the feature must be optional there.
5. **Application identity.** Set `WindowOptions::app_id`, install a `.desktop` file with the same name and an icon. Without them the task bar shows a generic icon.
6. **Shortcuts.** Write bindings with `secondary-`, and show Ctrl in labels.
7. **Notifications.** Accept that a tag does not replace and that dismiss does nothing, or call the notification service directly with `replaces_id` and `CloseNotification`.
8. **Text input.** Build an emoji picker. Add the primary selection and middle-click paste with gpui's primary calls. Test with IBus and Fcitx.
9. **Fonts.** Ship a monospace font or name one that Linux has; do not name `Menlo`.
10. **Accessibility.** As on macOS: each message needs its own accessible node. On Linux the tree exists only while the session's accessibility status is enabled.
11. **Secrets.** Use the Secret Service through `oo7`. Handle a locked collection, which prompts, and a session without a provider, with a key file readable only by the user.
12. **Build and packaging.** Keep one lock file made with the Linux features. Build on a base with an old C library. Bundle the three xcb and xkbcommon libraries; take drivers, the C library and the session services from the host.
13. **Title bar.** Decide between the window manager's title bar and a client-drawn one, and test GNOME, which draws none for the client.

## Needs that would require GPL code

None was met. No GPL-licensed crate of the Zed repository was opened. Read for this spike: `gpui-pre`, `gpui-pre-platform`, `gpui-pre-linux`, `gpui-pre-wgpu`, `gpui-base`, `gpui-component` (all Apache-2.0) and `oo7` (MIT). The licence field of each was checked first.

## Not verified

- C2: a full input method; the Compose key on X11.
- C3: link hover feedback.
- C4, C9: frame rate on a display driven by the GPU; the cause of the 40 to 50 ms interval under the headless compositor; GPU load.
- C6: a change of the desktop scale at run time.
- C7 (a): the cause of the failed file drop on X11; a drop from a file manager; any drop under Wayland.
- C8: always on top on other Wayland compositors.
- C10: dismiss.
- C11: a screen reader.
- L-A: KWin and GNOME as Wayland compositors; client-side title bars; an input method; drag and drop.
- L-B: a locked collection; a session without a provider; the key file fallback.
- L-C: an AppImage.
- The changed source run on macOS (it compiles there).

## Effects on the test machine, and what is left in place

Removed — **observed**: the directory `/srv/junto-spike-gpui/`; the containers `junto-spike-gpui-build`, `junto-spike-gpui-tools`, `junto-spike-gpui-gpu`; the images `junto-spike-gpui-build:probe`, `junto-spike-gpui-build:1`, `junto-spike-gpui-tools:1` and the pulled Tumbleweed base image; the six build-cache records of the two container files, including the pulled Rust base image (other build-cache records were not touched). No package was installed, no service or container of the machine was stopped, no configuration was changed. Every window the spike opened is closed: the spike windows, the GTK test windows (drag source and stacking test), the notification popups.

Left on the test machine:

- Shader cache files written by the graphics drivers for the spike process under the desktop user's `~/.cache/mesa_shader_cache/` and `~/.cache/nvidia/GLCache/`. They are shared caches; the spike's entries cannot be told from other entries, so nothing was deleted.
- The wallet files `~/.local/share/kwalletd/kdewallet.kwl` and `kdewallet_attributes.json` were rewritten by the store and delete of the test secret. The item count is unchanged.
- Up to seven entries named "Junto gpui spike" in the notification history of Plasma, if Plasma keeps unanswered notifications.
- The test strings of C2 and C3 in the history of Klipper. The clipboard itself was saved before the tests and restored (checked); the primary selection was never changed (checked).
- The pointer was moved during the tests and put back at its first position (70, 0).
- The active window is now the terminal window that was already open. Before the spike the desktop had focus. KWin moved focus there when the last spike window closed; the spike sent no input to that window.
- No file-system snapshot was taken while the directory existed (`snapper list`).

On the Mac:

- Deleted: `target/` from the macOS compile check, and the temporary scripts under `/tmp`.
- Left in `out/` (not tracked, 9.6 MB): screenshots, benchmark files, run logs, build logs, the AT-SPI dumps, the library lists, the licence list, and `out/fonts/` (the six font files and `OFL.txt`, copied from `spikes/gpui-client/out/fonts/`), which the application loads at start.
- Left and meant to be tracked: `Cargo.toml`, `Cargo.lock`, `Containerfile.build`, `Containerfile.tools`, `src/`, `scripts/`, this file.
