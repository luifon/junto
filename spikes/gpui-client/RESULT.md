# Spike: gpui client (step 0, test 2, macOS)

Date: 2026-10-09. Machine: macOS 26.6.1, Apple Silicon (8 cores, 16 GB), display 1512×982 at 120 Hz.

## Question

Can gpui and an Apache-2.0 component library provide what the Junto desktop client needs on macOS, and which set of dependency revisions works together?

## Result

| Gate | Result | One line |
|---|---|---|
| C1 Dependency set | PASS | Published crates `gpui-pre` 0.3.8 + `gpui-component` 0.7.1 build and run with Homebrew Rust 1.98.1. |
| C2 Composer | PARTIAL | All items observed except the press-and-hold accent menu, which the test tools could not open. |
| C3 Rich text | PASS | All listed elements render; selection across lines and across messages copies. Link hover feedback was not observable. |
| C4 Long list | PASS | 20,000 messages, opens at the bottom, 8.33 ms frame interval while scrolling, position kept on new messages. |
| C5 Layout | PASS | Three columns, collapsible rail, font from file, tabular numerals, tooltips, hover menu, context menu, modal dialog. |
| C6 Interface scale | PASS | 90 % to 150 % at run time through the rem size. Widths given in pixels do not scale. |
| C7 Drag and drop | PARTIAL | (b) in-application drag observed. (a) file drop from Finder not verified: the test tools cannot drive a system drag session. |
| C8 Second window | PASS | Second window opened, both windows update from one shared entity, closed by button and by the close control. |
| C9 Video tiles | PASS | 1, 4 and 9 tiles of 1280×720 at 30 fps on both paths. CPU 7–10 % (surface) and 12–43 % (image) of one core. |
| C10 Notifications | PASS | Title, body and two actions delivered; action click and body click received. Needs an application bundle and a permission prompt. |
| C11 Accessibility | recorded | The accessibility tree contains the labelled controls. It does not contain message text. |
| C12 Keyboard | PASS | Tab order, a bound shortcut, and two keyboard paths to the hover menu. |

Every statement below is marked **observed** (run and seen here) or **not verified**.

## Dependency set (C1)

All from crates.io, all Apache-2.0. No git dependency. `Cargo.lock` in this directory records the full set (841 packages, 510 compiled for macOS).

| Crate | Version | Role |
|---|---|---|
| `gpui-pre` | 0.3.8 | gpui, feature `profiler` |
| `gpui-pre-platform` | 0.3.8 | platform selection, feature `font-kit` (needed for glyphs on macOS) |
| `gpui-pre-macos`, `gpui-pre-apple`, `gpui-pre-macros`, `gpui-pre-sum-tree`, `gpui-pre-http-client` | 0.3.8 | pulled in by the above |
| `gpui-pre-reqwest-client` | 0.3.8 | HTTP client for images loaded by URL |
| `gpui-component` | 0.7.1 | component library, features `tree-sitter-rust`, `tree-sitter-toml` |
| `gpui-base` | 0.7.1 | unstyled layer of the same library; holds the Markdown view and the input engine |
| `gpui-kit-assets` | 0.7.1 | icon assets of the library |
| `core-video` | 0.5.2 | `CVPixelBuffer` for the video surface path (the version gpui uses) |
| `accesskit` / `accesskit_macos` | 0.24.1 / 0.26.3 | accessibility, pulled in by gpui |
| `taffy` | 0.13.0 | layout, pulled in by gpui |

Facts about this set:

- **observed** `gpui-component` 0.7.1 pins `gpui-pre = "=0.3.8"`. The two must be updated together.
- **observed** `gpui-pre*` are snapshot crates of the gpui sources in the Zed repository. The crates.io owner is the maintainer of the component library, not the Zed organisation. A new snapshot was published each week (0.3.5 on 2026-09-14 to 0.3.8 on 2026-10-05). The crate carries no commit hash of the Zed repository.
- **observed** The crate named `gpui` on crates.io is 0.2.2 from 2025-10-22. It was not used.
- **not verified** A set built from git revisions of the Zed repository and of the component library. The published set worked, so this was not tried.
- **observed** Licence check of every downloaded package in `Cargo.lock` (licence field of each `Cargo.toml`): no package is GPL-only. Three are MPL-2.0 (`cbindgen`, `dwrote`, `option-ext`); two offer LGPL only as one of three choices (`r-efi`).
- **observed** The component library is Apache-2.0 (`license` field and `LICENSE-APACHE` in the crate). Its repository field is `github.com/longbridge/gpui-kit`.

Toolchain and build:

- **observed** Homebrew Rust 1.98.1 builds the set. No rustup and no pinned toolchain were needed. The crates use edition 2024.
- **observed** Xcode 26.5 is installed. gpui compiles its Metal shaders at build time and its documentation requires Xcode.
- **observed** Nothing was installed with Homebrew.

Measured:

| Item | Value | How |
|---|---|---|
| Clean release build | 199 s wall, 951 s user CPU | `cargo clean`, then `tools/serial.sh /usr/bin/time -p cargo build --release`, at `nice 10` |
| Rebuild after a change in the spike crate | 10–12 s | cargo output |
| Binary | 32.9 MB (25.2 MB after `strip`) | `ls -l target/release/spike` |
| `target/` after one release build | 1.8 GB | `du -sh` |
| Idle window, composer screen, 1320×861 points | 96 MB physical footprint, 88 MB RSS | `footprint -p`, `ps -o rss`, 20 s after start |
| Idle CPU | 1 % of one core (cursor blink) | `top -l 2 -pid` |

## Method

One Rust application, `src/`, with one screen per gate, chosen by `--gate c2` … `--gate c12` or by the buttons in the top bar. C6 is the scale buttons in the top bar and applies to every screen. The application writes each event it receives to `out/events.txt`.

The windows were operated with the command-line program of a desktop automation tool and with small Swift tools in `scripts/`. The scripts read the command of the automation tool from the environment variable `JUNTO_TEST_AUTOMATION_CLI`. Screenshots are in `out/`, resampled so that one image pixel is one window point.

What the test tools could and could not do matters for the two PARTIAL results:

- **observed** Clicks, key presses and text typed through the automation tool reached gpui.
- **observed** Scroll events, right clicks and drags posted to the process did not reach gpui. Scroll and drag worked when posted to the HID event tap, which moves the real pointer for a moment.
- **observed** A system drag session (`NSDraggingSession`) did not follow synthetic events: a control window that accepts file drops (`scripts/drag_dest.swift`) received nothing either. This is a limit of the tools, not a finding about gpui.

Commands (from this directory):

```sh
../../tools/serial.sh cargo build --release
../../tools/serial.sh scripts/build_tools.sh      # Swift test tools into out/bin
scripts/run.sh c4                                 # start one gate; pid in out/pid
scripts/shot.sh name                              # screenshot to out/name.png and the accessibility tree
scripts/press.sh "Run scroll benchmark"           # press a control by its accessibility label
scripts/act.sh hotkey --key CmdOrCtrl+Shift+M     # any subcommand of the automation tool against the spike
scripts/in.sh gscroll 600 400 40 15               # pointer events in window coordinates
scripts/bundle.sh && scripts/run_bundle.sh c10    # application bundle, needed for notifications
out/bin/ax_dump "$(cat out/pid)"                  # accessibility tree through the AX API
```

The font files (Atkinson Hyperlegible Next, OFL 1.1) were downloaded from the `googlefonts/atkinson-hyperlegible-next` repository, `fonts/ttf/`, commit `7925f50f`, into `out/fonts/`.

## Gates

### C2 Composer — PARTIAL

Component: `gpui_component::input::Textarea` with `TextareaState::new(..).auto_grow(1, 6).submit_on_enter(true)`.

| Item | Result | Evidence |
|---|---|---|
| Typing | observed | `c2-01-two-lines.png` |
| Enter sends | observed | event `PressEnter shift=false`, `c2-02-pasted.png` shows the sent message |
| Shift+Enter adds a line | observed | event `PressEnter shift=true`, value contains `\n` |
| Paste, several lines | observed | `c2-02-pasted.png` |
| Word wrap | observed | first pasted line wraps to three rows |
| Growth to 6 rows, then scrolling with a scrollbar | observed | `c2-02-pasted.png`, `c2-04-typed-after-paste-crop.png` |
| Undo | observed | Cmd+Z removed the last typed text, `c2-05-undo-crop.png` |
| Selection, keyboard | observed | Shift+Left, Shift+Option+Left, `c2-06-keyboard-selection-crop.png` |
| Selection, mouse drag across lines | observed | `c2-17-crop.png` (HID-level drag) |
| Selection, click then Shift+click | observed | `c2-08-shift-click-selection-crop.png` |
| Dead keys: Option-e e, Option-n n, Option-n a, Option-i o | observed | value `caféñãô`, `c2-11-dead-keys-more-crop.png`; a layout with dead keys |
| Marked text drawn | observed | `´` drawn underlined before the second key, `c2-09-dead-key-marked-crop.png` |
| System emoji picker | observed | opened by `Window::show_character_palette()`, 🚀 inserted, `c2-15-emoji-picker.png`, `c2-16-crop.png` |
| Press-and-hold accent menu | not verified | synthetic key repeats did not open the system menu; no repeated characters were inserted either |
| Full input method (for example Japanese or Chinese) | not verified | needs an input source added in System Settings |

Limits found:

- **observed** After a paste that exceeds the maximum height, the view stays at the top although the caret is at the end. It scrolls to the caret at the next key press.
- **observed** `cursor()` and `selected_range()` are byte offsets, not character counts.
- gpui passes held keys to the system input context by default (`apple_press_and_hold_enabled` returns true in `gpui-pre/src/platform.rs`). This is source reading, not an observation.

### C3 Rich text — PASS

Component: `gpui_base::text::TextView::markdown(id, text).selectable(true)`. The styled wrapper in `gpui-component` does not expose `image_source`, so the spike uses the base type; the theme and the syntax highlighter still apply.

| Item | Result | Evidence |
|---|---|---|
| Bold, italic, strikethrough, inline code | observed | `c3-01-rich.png` |
| Code block with syntax colours (Rust, TOML) | observed | `c3-01-rich.png`; needs the `tree-sitter-<language>` features |
| Quote, ordered list, nested list, task list | observed | `c3-01-rich.png` |
| Link click | observed | event `link clicked https://example.org/docs` through `on_link_click` |
| Link hover | not observed | no visible change in the window capture; the library sets a pointing-hand cursor (source), which a window capture does not show |
| Mention chip | observed | `MarkdownPlugin` of 20 lines in `src/rich.rs`, written for the spike |
| Emoji, including skin tone and flag sequences | observed | `c3-01-rich.png` |
| Image from a file | observed | `c3-07-block-images.png`; needs an `image_source` resolver, because `file://` is not loaded by default |
| Image from an HTTP URL | observed | request logged by the local server, `c3-07-block-images.png` |
| Image from an HTTPS URL | observed | `c3-08-crop.png` |
| Animated GIF, file and HTTP | observed | three captures show three frames, `c3-gif-*-crop.png` |
| Mouse selection across several lines | observed | `c3-04-crop.png` |
| Selection across several messages, then copy | observed | `c3-05-selection-across-messages.png`, clipboard text in `c3-copied-across-messages.txt` |

Limits found:

- **observed** An image inside a line of text is drawn at line height. An image alone in a paragraph is drawn at its own size.
- **observed** Text that is not inside a `TextView` (author name, time) is not selectable and is not copied. List numbers are not copied.

### C4 Long list — PASS

Two variants on one screen: gpui `list` with `ListState` and `FollowMode::Tail`, and `gpui_component::message_scroller::MessageScroller`, which wraps the same list and adds a scrollbar and a jump button. Each row is an avatar, a name, a time and a Markdown `TextView`; heights differ (1 to 7 sentences, a code block every 13th row, a list every 17th).

| Item | Result | Evidence |
|---|---|---|
| 20,000 rows of varying height | observed | generated in 12.7 ms, `c4-05-following-tail.png` |
| Opens at the bottom | observed | last row `#19999` visible at start |
| Position kept when messages arrive while scrolled up | observed | 19 messages added, same pixels, `c4-01-scrolled-up-crop.png` and `c4-03-after-auto-added-crop.png` |
| Jump to bottom, then follows new messages | observed | `c4-05-following-tail.png` |
| `MessageScroller` jump button | observed | `c4-07-message-scroller-scrolled-up.png` |

Frame times, measured in the application (`out/c4-bench.txt`). The benchmark scrolls the list by a fixed number of pixels in every `Window::on_next_frame` callback and records the interval between callbacks. The draw duration and the present interval come from gpui's own histograms (`Window::frame_duration_snapshot`, feature `profiler`).

| Phase (600 frames each) | mean | p95 | p99 | max | frames over 12.5 ms |
|---|---|---|---|---|---|
| Up, 30 px per frame | 8.33 ms | 9.03 | 9.26 | 11.59 | 0 |
| Up, 400 px per frame | 8.33 ms | 9.22 | 9.34 | 9.36 | 0 |
| Down, 400 px per frame | 8.51 ms | 9.32 | 14.54 | 18.48 | 14 |

- gpui `Window::draw` duration over the 1,802 frames: p50 3.35 ms, p95 4.30, p99 4.66, max 7.59.
- gpui present interval: p50 8.35 ms, p95 9.94, p99 12.26, max 18.48.
- Process CPU during the benchmark: 50 % of one core.
- Memory: 116 MB physical footprint with the list at the bottom, 141 MB after the benchmark (peak 156 MB).

### C5 Layout — PASS

| Item | Result | Evidence |
|---|---|---|
| Three columns with a right panel | observed | `c5-01-layout.png` |
| Rail collapses to icons | observed | `c5-08-rail-collapsed.png` |
| Font loaded from files (`TextSystem::add_fonts`) | observed | 6 files loaded; slashed zero of the typeface visible in every capture |
| Tabular numerals (`font_features` with `tnum`) | observed | `11:11:11` is wider with `tnum`, `c5-01-layout.png` |
| Tooltip on a library button and on a plain element | observed | `c5-02-crop.png`, `c5-10-crop.png` |
| Hover menu | observed | `c5-03-hover-menu.png`, `c5-09-rail-collapsed-hover-menu.png` |
| Context menu (right click) | observed | `c5-05-crop.png`, event `context: Reply in thread` |
| Modal dialog with an input | observed | `c5-06-dialog.png`, event `join dialog confirmed with "harbor.example:7443"` |

Limits found:

- **observed** `Button::tooltip` and `Button::on_hover` do not work together: the hover handler was not called. The spike puts the hover handler on a wrapping element.
- **observed** An action handler runs only when its element is on the focus path. Each screen needs its own focus handle, and a menu needs `action_context` set to it.
- **observed** `Dialog` has no buttons unless a footer is given. Enter confirms (`on_ok`) and Escape cancels (`on_cancel`).
- **observed** In `PopupMenu`, the first Down key selects a label row.

### C6 Interface scale — PASS

`Theme::update(cx, |theme| theme.font_size = px(16.0 * percent / 100.0))`. The library root sets the window rem size from this value.

| Item | Result | Evidence |
|---|---|---|
| 90 % | observed | rem 14.4 px, `c6-01-scale-90.png` |
| 150 % | observed | rem 24 px, `c6-04-scale-150-rail-collapsed.png` |
| Text and library controls grow together | observed | buttons, inputs, menus, tooltips |
| Layout holds | observed | no overlap; text wraps |

Limits found:

- **observed** Sizes given in pixels do not scale. The dialog keeps its 448 px width at 150 % while its text grows (`c6-06-scale-150-dialog.png`). The spike gives its own sizes in rems.
- **observed** At 150 % in a 1320-point window, the three side columns at fixed rem widths leave 180 points for the content (`c6-03-scale-150.png`). The product needs rules for which panel collapses.

### C7 Drag and drop — PARTIAL

| Item | Result | Evidence |
|---|---|---|
| (b) Drag a person to another voice channel | observed | preview follows the pointer, target highlighted, `c7-02-dragging.png`; event `Bruno moved to Focus room by drag` |
| (b) Same move through a menu | observed | event `Ana moved to HR by menu` |
| (a) Files dragged from Finder | not verified | see below |

(a) What was tried: a drag of two files from a Finder window with HID-level events at three speeds; a drag from a helper window that starts a real file drag session (`scripts/drag_source.swift`). The session started and ended with no drop. A control window that accepts file drops received nothing either, so the tools cannot drive a system drag session. What gpui provides, from its source: the window registers for file drags (`registerForDraggedTypes` in `gpui-pre-macos`), and elements receive them with `on_drop::<ExternalPaths>`. The spike screen has this handler.

To close this gate by hand: `scripts/run.sh c7`, drag a file from Finder onto the dashed area, read `drag and drop: file received …` in `out/events.txt`.

### C8 Second window — PASS

| Item | Result | Evidence |
|---|---|---|
| Open a second window from the first (`cx.open_window`) | observed | `c8-02-call-window.png` |
| Both update from shared state | observed | one `Entity<CallState>` observed by both; counter 53 and 54 in two captures 0.5 s apart; mute toggled in the call window shows in both (`c8-03-crop.png`, `c8-04-crop.png`) |
| The window that is not in front keeps updating | observed | same captures |
| Return by button, from either window | observed | events `call returned to the main window`, `call window released` |
| Close with the window close control | observed | event `call window released`; main window shows "not detached" |

Always on top:

- **observed** `WindowKind::Floating` gives window level 3 (`out/bin/windows` reads the level). The window is an `NSPanel`: its accessibility subrole is `AXDialog`, it has a zoom control and no full-screen control.
- From the source: `WindowKind::PopUp` gives level 101, does not activate, and joins all spaces.
- From the source: the kind is set when the window is created. gpui has no call that changes the level later. A pin toggle needs either a new window of the other kind or a call to `NSWindow` through the raw window handle that `Window` exposes.

Limit found: **observed** a window cannot be removed through its handle from inside its own event handler; `window.remove_window()` on the handler's own window parameter works.

### C9 Video tiles — PASS

A background thread produces 1280×720 frames at 30 per second for each tile and sends them over a channel.

- Surface path: `gpui::surface(CVPixelBuffer)`, NV12 full range (the Metal renderer asserts this format), IOSurface-backed, three buffers per tile reused in turn. Each frame copies the luma and chroma planes (1.4 MB) and draws marks.
- Image path: `gpui::img(ImageSource::Render(Arc<RenderImage>))` with a new BGRA image per frame (3.7 MB), and `window.drop_image` for the previous one.

Measured in the application over 8 s after 2 s of warm-up (`out/c9-bench.txt`). "Shown" counts frames that the window drew, per tile. CPU is user plus system time of the whole process (`getrusage`), as a share of one core, and includes frame generation.

| Path | Tiles | Shown fps per tile | Process CPU | of which frame generation |
|---|---|---|---|---|
| surface | 1 | 30.0 | 7 % | 0 % |
| surface | 4 | 30.0 | 9 % | 2 % |
| surface | 9 | 30.0 | 10 % | 3 % |
| image | 1 | 30.0 | 12 % | 1 % |
| image | 4 | 30.0 | 22 % | 3 % |
| image | 9 | 30.0 | 43 % | 6 % |

- No frame was replaced before it was shown in any configuration.
- `ps` samples taken once per second from outside agree: 7, 9, 10, 11, 21, 44 %.
- RSS up to 136 MB on the surface path and 199 MB on the image path with 9 tiles.
- **not verified** GPU load and the cost in the window server. The window server process showed 47–50 % during the run and 31–49 % after it, with other work on the machine, so its share could not be separated.
- Evidence: `c9-01-surface-1.png`, `c9-03-img-9.png`.

### C10 Notifications — PASS

API: `cx.show_system_notification(SystemNotification { tag, title, body, actions })`, `cx.on_system_notification_response(..)`, `cx.dismiss_system_notification(tag)`. On macOS it uses `UNUserNotificationCenter`.

| Item | Result | Evidence |
|---|---|---|
| Needs an application bundle | observed | without a bundle gpui logs `system notifications disabled: not running from an app bundle`; with `out/JuntoSpike.app` (ad hoc signature) it posts |
| macOS asks for permission | observed | prompt "Junto gpui spike Notifications" on the first post; accepted with one click, no password |
| Notification with title, body and two actions | observed | Notification Center listed "Luca mentioned you", the body, and the actions Reply and Mark as read |
| Action click received | observed | event `action 'reply' received for tag 'junto-spike-mention'` |
| Body click received | observed | event `body clicked for tag 'junto-spike-mention'` |
| Banner on screen | not observed | the system log says `muted by display state (displayShared)`: a screen recording was active on the machine and macOS hides banners in that state by default. The notifications went to Notification Center, where the clicks were made. |

Limits: the gpui type has a tag, a title, a body and action buttons. It has no sound, image or text-reply field. The same tag replaces the earlier notification. There is one response handler for the application.

### C11 Accessibility — recorded

gpui builds an AccessKit tree. An element appears in it when it has an id and a role. Read with the AX API (`out/bin/ax_dump`, `out/c11-ax-tree.txt`); VoiceOver was not started because it speaks.

What a screen reader can read — **observed**:

- Library `Button`: `AXButton` with its label as title, action `AXPress`. `accessibility_label("Mute microphone")` replaces an icon-only label.
- A plain element with `.role(Role::Button).aria_label(..)`: `AXButton` with that title.
- `.role(Role::Switch).aria_toggled(..)`: `AXCheckBox`, subrole `AXSwitch`, value 0 or 1.
- Library `Switch` and `Checkbox`: exposed with label and value.
- Library `Input` with `aria_label`: `AXTextField` with that title. `Textarea`: text area with its placeholder.
- A heading with a `text!` child, and a list with items.
- Menus and menu items of `PopupMenu`.
- `AXPress` from the accessibility side runs the click handler (events `mute button pressed`, `leave button pressed`).

What it cannot read — **observed**:

- Message text. The Markdown `TextView` puts nothing in the tree. The long list screen exposes its buttons only (`out/c11-ax-tree-long-list.txt`).
- Plain string children of elements without a role.
- The `MessageScroller` jump button has no label.
- A focusable element without a role: gpui logs that the focused element has no accessibility node.

Answer to the gate: the labelled controls, not the content. A person who depends on a screen reader can operate buttons, switches, inputs and menus, and cannot read a conversation unless the product gives each message an accessible node.

### C12 Keyboard — PASS

| Item | Result | Evidence |
|---|---|---|
| Tab order | observed | First → Second → input → Third → plain focusable element → server tile → top bar buttons; Shift+Tab goes back. Focus read through the AX API after each key. The AX API reported no focused element for the input and for the plain element; typed text and Enter showed that they had focus. |
| Focus ring | observed | `c12-01-crop.png`, `c12-04-crop.png` |
| Text typed into the input reached by Tab | observed | `c12-04-crop.png` |
| Enter or Space activates the focused control | observed | event `raw div activated` |
| Shortcut bound to an action (Cmd+K) | observed | events `Cmd+K action ran, count 1` and `count 2` |
| Keyboard path to the hover menu, by shortcut (Cmd+Shift+M) | observed | event `server menu opened by keyboard shortcut`, arrows and Enter chose `Client Alpha`, `c12-03-shortcut-and-menu.png` |
| Keyboard path to the hover menu, by Tab then Space | observed | `c12-02-menu-by-focus-and-space.png`, arrows and Enter chose `Studio` |

## What the product must build itself

No library tested here provides these:

- An accessible node for each message, if a screen reader is to read conversations (C11).
- The video tile: frames from the call library into `CVPixelBuffer` NV12 on macOS and into BGRA images elsewhere, with pacing and release of old images (C9).
- The mention chip and other custom inline elements, as Markdown plugins (20 lines for the chip).
- An image resolver for local files and for authenticated attachment URLs (`TextView::image_source`).
- Selectable author and time text, if a copied selection must contain them.
- Rules for which panel collapses at each scale and window width (C6).
- A runtime "keep on top" toggle (C8).
- The application bundle and its signature, required for notifications (C10), and the routing from a notification response to a window and a channel.
- Focus handling per screen so that shortcuts reach the visible screen (C5).

## Needs that would require GPL code

None was met. No GPL-licensed crate of the Zed repository was opened. Only `gpui-pre*` crates (Apache-2.0), their examples, and the component library were read.

## Not verified

- C2: press-and-hold accent menu; any full input method.
- C3: link hover feedback.
- C7 (a): file drop from Finder.
- C9: GPU and window server cost.
- C10: a banner on screen.
- Windows and Linux. This spike ran on macOS only.
- A dependency set from git revisions.

## Effects on the machine, and what is left in place

- macOS now has a notification permission entry for the bundle identifier `junto.spike.gpui-client` (allowed). It can be removed in System Settings → Notifications. No setting was changed by hand.
- The clipboard was replaced three times for the paste and copy tests and restored each time from its plain-text content. Content of another kind would not have been restored; the clipboard held plain text.
- The pointer was moved by the scroll and drag tools and put back. A Finder window was opened on `out/dropfiles` and closed. Notification Center was opened and closed.
- Another source of input was active on the machine during the run: keys that the test did not send reached the spike window once, and one paste inserted clipboard text that the test had not set. The capture that showed it was deleted.
- Deleted: `target/` (1.8 GB), `out/JuntoSpike.app`, `out/bin/`.
- Left in `out/` (not tracked): screenshots, `events.txt`, benchmark files, the accessibility dumps, the generated test images, and `out/fonts/` (six font files and `OFL.txt`, 0.4 MB), which the application loads at start.
- Left and meant to be tracked: `Cargo.toml`, `Cargo.lock`, `src/`, `scripts/`, this file.
