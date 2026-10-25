//! The title bar of the windows (ADR 0703). On macOS the main window reaches the top edge and
//! the system draws the window buttons over it: the headers at the top of the window are its
//! title bar. Windows leaves the title bar of the main window to the application, and so does
//! a Wayland compositor without server decorations for every window: the application then
//! draws a strip with the window buttons above the content.

use std::collections::HashSet;

use gpui::{
    AnyElement, App, Context, CursorStyle, Decorations, Global, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, ResizeEdge, Role,
    SharedString, StatefulInteractiveElement, Styled, Tiling, Window, WindowControlArea, WindowId,
    div, prelude::FluentBuilder, px, svg,
};

use super::Shell;
use crate::assets::Icon;
use crate::text;
use crate::theme::theme;
use crate::ui::StyleExt;

/// The height of the strip and the width of one window button, in pixels of the display: the
/// measures of the caption buttons of Windows 11. They follow the scale of the display and not
/// the interface scale of Junto, as the title bars of other applications do.
const STRIP_HEIGHT: f32 = 32.0;
const BUTTON_WIDTH: f32 = 46.0;
const GLYPH_SIZE: f32 = 16.0;
/// The band inside each edge of a window with a drawn frame on Linux where a press resizes
/// the window.
const RESIZE_BAND: f32 = 4.0;

/// The windows whose content reaches the title bar: on macOS the main window.
#[derive(Default)]
struct ContentTitleWindows(HashSet<WindowId>);

impl Global for ContentTitleWindows {}

/// A press in a part of a title bar that may become a move of the window.
#[derive(Default)]
struct TitlePress(bool);

impl Global for TitlePress {}

/// The frame in a test, whatever the system: `drawn` draws the strip, with the buttons of
/// Windows on Windows and with those of Linux on the other systems; on the other systems also
/// the resize bands, for the edges that `tiling` leaves free and unless `maximized`.
#[cfg(feature = "test-support")]
#[derive(Clone, Copy, Default)]
pub struct DrawnFrameInTests {
    pub drawn: bool,
    pub tiling: Tiling,
    pub maximized: bool,
}

#[cfg(feature = "test-support")]
impl Global for DrawnFrameInTests {}

/// What a title bar asks of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleBarRequest {
    /// Move the window with the pointer.
    Move,
    /// The action of a double press on a title bar that the system setting names (macOS).
    DoublePress,
    Minimize,
    /// Maximize or restore.
    Zoom,
    /// Resize the window from this edge with the pointer.
    Resize(ResizeEdge),
    /// The window menu of the compositor at the pointer.
    WindowMenu,
}

/// The requests of the title bars in a test: the test window of gpui carries out none of
/// them, so a test that installs this global reads them instead.
#[cfg(feature = "test-support")]
#[derive(Clone, Default)]
pub struct TitleBarRequests(pub std::rc::Rc<std::cell::RefCell<Vec<TitleBarRequest>>>);

#[cfg(feature = "test-support")]
impl Global for TitleBarRequests {}

fn request(kind: TitleBarRequest, window: &mut Window, cx: &mut App) {
    #[cfg(feature = "test-support")]
    if let Some(requests) = cx.try_global::<TitleBarRequests>() {
        requests.0.borrow_mut().push(kind);
        return;
    }
    let _ = cx;
    match kind {
        TitleBarRequest::Move => window.start_window_move(),
        TitleBarRequest::DoublePress => window.titlebar_double_click(),
        TitleBarRequest::Minimize => window.minimize_window(),
        TitleBarRequest::Zoom => window.zoom_window(),
        TitleBarRequest::Resize(edge) => window.start_window_resize(edge),
        TitleBarRequest::WindowMenu => window.show_window_menu(window.mouse_position()),
    }
}

/// Records that the content of this window reaches its title bar, so that its headers act as
/// the title bar: the main window on macOS.
pub fn content_reaches_title_bar(window: &Window, cx: &mut App) {
    cx.default_global::<ContentTitleWindows>()
        .0
        .insert(window.window_handle().window_id());
}

/// Whether the content of this window reaches its title bar.
pub(crate) fn reaches_title_bar(window: &Window, cx: &App) -> bool {
    cx.try_global::<ContentTitleWindows>()
        .is_some_and(|windows| windows.0.contains(&window.window_handle().window_id()))
}

/// The space above the panel of a layer, in design pixels: in a window whose content reaches
/// its title bar the panel starts under the band of [`layer_band`].
pub(crate) fn cover_top(window: &Window, cx: &App) -> f32 {
    const MARGIN: f32 = 16.0;
    if reaches_title_bar(window, cx) {
        MARGIN.max(super::window_controls_height(cx))
    } else {
        MARGIN
    }
}

/// A band at the top of a window whose content reaches its title bar, over an open layer: the
/// layer covers the headers, and the band keeps the move of the window and its double press. A
/// press there does not close the layer.
pub(crate) fn layer_band(window: &Window, cx: &App) -> Option<AnyElement> {
    reaches_title_bar(window, cx).then(|| {
        header_zone(
            div()
                .id("title-band")
                .debug_selector(|| "title-band".to_owned())
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(crate::theme::u(super::window_controls_height(cx)))
                .occlude(),
        )
        .into_any_element()
    })
}

/// Whether the application draws the frame of this window: the strip with the window buttons
/// and, on Linux, the bands that resize it. `hides_system_title`: the window asked the system
/// for no title bar (the main window).
pub(crate) fn draws_frame(window: &Window, hides_system_title: bool, cx: &App) -> bool {
    #[cfg(feature = "test-support")]
    if let Some(frame) = cx.try_global::<DrawnFrameInTests>() {
        return frame.drawn;
    }
    let _ = cx;
    if cfg!(windows) {
        return hides_system_title;
    }
    cfg!(target_os = "linux") && matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Makes the parts of `element` that hold no control a part of the title bar of a window whose
/// content reaches it: a press and a move there move the window, and a double press does what
/// the system setting of a double press on a title bar says (zoom, fill, minimize or nothing). A
/// control takes its press first: gpui gives the press to the focusable element under the
/// pointer and marks it as handled, so a press on a button or a field stays theirs.
pub(crate) fn header_zone<E: InteractiveElement>(element: E) -> E {
    moves_window(element, TitleBarRequest::DoublePress, reaches_title_bar)
}

fn moves_window<E: InteractiveElement>(
    element: E,
    double: TitleBarRequest,
    applies: impl Fn(&Window, &App) -> bool + 'static,
) -> E {
    element
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                if window.default_prevented() || !applies(window, cx) {
                    return;
                }
                if event.click_count == 2 {
                    cx.set_global(TitlePress(false));
                    request(double, window, cx);
                } else {
                    cx.set_global(TitlePress(true));
                }
            },
        )
        .on_mouse_move(|event: &MouseMoveEvent, window, cx| {
            let pressed = cx.try_global::<TitlePress>().is_some_and(|press| press.0);
            if pressed && event.pressed_button == Some(MouseButton::Left) {
                cx.set_global(TitlePress(false));
                request(TitleBarRequest::Move, window, cx);
            }
        })
        .on_mouse_up(MouseButton::Left, |_, _, cx| {
            cx.set_global(TitlePress(false));
        })
}

#[derive(Clone, Copy, PartialEq)]
enum Control {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Control {
    fn selector(self) -> &'static str {
        match self {
            Self::Minimize => "window-minimize",
            Self::Maximize => "window-maximize",
            Self::Restore => "window-restore",
            Self::Close => "window-close",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Minimize => text::WINDOW_MINIMIZE,
            Self::Maximize => text::WINDOW_MAXIMIZE,
            Self::Restore => text::WINDOW_RESTORE,
            Self::Close => text::WINDOW_CLOSE,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Minimize => Icon::WindowMinimize,
            Self::Maximize => Icon::WindowMaximize,
            Self::Restore => Icon::WindowRestore,
            Self::Close => Icon::WindowClose,
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
}

impl Shell {
    /// The strip at the top of a window whose frame the application draws: the title, and the
    /// buttons that minimize, maximize or restore, and close the window. On Windows the system
    /// handles the strip and its buttons by their areas (gpui answers its hit test): a press
    /// and a move on the strip move the window, a double press maximizes and restores it, the
    /// pointer on the maximize button offers the snap layouts, and the buttons act at the
    /// release as on every window. On Linux the application handles them.
    pub(crate) fn render_title_strip(
        &self,
        title: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let system_areas = cfg!(windows);
        // A double press on the strip of a Linux window maximizes and restores it.
        let supported = window.window_controls();
        let mut controls = Vec::new();
        if system_areas || supported.minimize {
            controls.push(Control::Minimize);
        }
        if system_areas || supported.maximize {
            controls.push(if window.is_maximized() {
                Control::Restore
            } else {
                Control::Maximize
            });
        }
        controls.push(Control::Close);
        let title = div()
            .id("title-strip-drag")
            .debug_selector(|| "title-strip-drag".to_owned())
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .pl(px(12.0))
            .typeface(theme.types.label, &theme)
            .text_size(px(12.0))
            .ink(colors.bone_label)
            .child(div().truncate().child(title))
            .map(|area| {
                if system_areas {
                    area.window_control_area(WindowControlArea::Drag)
                } else {
                    moves_window(area, TitleBarRequest::Zoom, |_, _| true)
                        .on_mouse_down(MouseButton::Right, |_, window, cx| {
                            request(TitleBarRequest::WindowMenu, window, cx)
                        })
                }
            });
        div()
            .id("title-strip")
            .debug_selector(|| "title-strip".to_owned())
            .flex()
            .flex_none()
            .w_full()
            .h(px(STRIP_HEIGHT))
            .bg(Hsla::from(colors.basalt))
            .border_b_1()
            .border_color(Hsla::from(colors.basalt_edge))
            .child(title)
            .children(
                controls
                    .into_iter()
                    .map(|control| self.render_window_button(control, system_areas, cx)),
            )
            .into_any_element()
    }

    fn render_window_button(
        &self,
        control: Control,
        system_areas: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme(cx).colors;
        let (hover, pressed, hover_ink) = if control == Control::Close {
            (
                Hsla::from(colors.cinnabar),
                Hsla::from(colors.cinnabar_deep),
                Hsla::from(colors.on_cinnabar),
            )
        } else {
            let bone = Hsla::from(colors.bone);
            (bone.opacity(0.1), bone.opacity(0.06), bone)
        };
        let group = control.selector();
        div()
            .id(control.selector())
            .group(group)
            .role(Role::Button)
            .aria_label(control.label())
            .debug_selector(|| control.selector().to_owned())
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(BUTTON_WIDTH))
            .h_full()
            .hover(move |style| style.bg(hover))
            .active(move |style| style.bg(pressed))
            .child(
                svg()
                    .path(control.icon().path())
                    .size(px(GLYPH_SIZE))
                    .flex_none()
                    .text_color(Hsla::from(colors.bone_label))
                    .group_hover(group, move |style| style.text_color(hover_ink)),
            )
            .map(|button| {
                if system_areas {
                    button.window_control_area(control.area())
                } else {
                    button
                        // The strip moves the window at a press: a press on a button is not one.
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            match control {
                                Control::Minimize => request(TitleBarRequest::Minimize, window, cx),
                                Control::Maximize | Control::Restore => {
                                    request(TitleBarRequest::Zoom, window, cx)
                                }
                                Control::Close => this.close_window(window, cx),
                            }
                        }))
                }
            })
            .into_any_element()
    }

    /// Closes the window as a close by the system does.
    pub(crate) fn close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let detached = self.detached;
        self.state.update(cx, |state, cx| {
            if detached {
                state.closed_call_window(cx);
            } else {
                state.return_call(cx);
            }
        });
        window.remove_window();
    }
}

/// The bands inside the edges of a window whose frame the application draws on Linux: a press
/// there resizes the window from that edge or corner. An edge that the compositor tiled has
/// none, and a maximized window has none.
pub(crate) fn resize_bands(window: &Window, cx: &App) -> Vec<AnyElement> {
    // Windows resizes the window from the borders of its frame.
    if cfg!(windows) {
        return Vec::new();
    }
    let (tiling, maximized) = edges(window, cx);
    if maximized {
        return Vec::new();
    }
    let band = px(RESIZE_BAND);
    let corner = px(RESIZE_BAND * 3.0);
    let mut bands = Vec::new();
    let mut add =
        |edge: ResizeEdge, place: fn(gpui::Div, gpui::Pixels, gpui::Pixels) -> gpui::Div| {
            if !free(edge, tiling) {
                return;
            }
            let cursor = match edge {
                ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
                ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
            };
            bands.push(
                place(div().absolute(), band, corner)
                    .id(SharedString::from(format!("resize-{edge:?}")))
                    .debug_selector(move || format!("resize-{edge:?}"))
                    .occlude()
                    .cursor(cursor)
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        request(TitleBarRequest::Resize(edge), window, cx);
                    })
                    .into_any_element(),
            );
        };
    add(ResizeEdge::Top, |d, band, _| {
        d.top_0().left_0().right_0().h(band)
    });
    add(ResizeEdge::Bottom, |d, band, _| {
        d.bottom_0().left_0().right_0().h(band)
    });
    add(ResizeEdge::Left, |d, band, _| {
        d.top_0().bottom_0().left_0().w(band)
    });
    add(ResizeEdge::Right, |d, band, _| {
        d.top_0().bottom_0().right_0().w(band)
    });
    add(ResizeEdge::TopLeft, |d, _, corner| {
        d.top_0().left_0().size(corner)
    });
    add(ResizeEdge::TopRight, |d, _, corner| {
        d.top_0().right_0().size(corner)
    });
    add(ResizeEdge::BottomLeft, |d, _, corner| {
        d.bottom_0().left_0().size(corner)
    });
    add(ResizeEdge::BottomRight, |d, _, corner| {
        d.bottom_0().right_0().size(corner)
    });
    bands
}

/// The tiled edges of a window whose frame the application draws, and whether it is maximized.
fn edges(window: &Window, cx: &App) -> (Tiling, bool) {
    #[cfg(feature = "test-support")]
    if let Some(frame) = cx.try_global::<DrawnFrameInTests>() {
        return (frame.tiling, frame.maximized);
    }
    let _ = cx;
    match window.window_decorations() {
        Decorations::Client { tiling } => (tiling, window.is_maximized()),
        Decorations::Server => (Tiling::tiled(), true),
    }
}

fn free(edge: ResizeEdge, tiling: Tiling) -> bool {
    match edge {
        ResizeEdge::Top => !tiling.top,
        ResizeEdge::Bottom => !tiling.bottom,
        ResizeEdge::Left => !tiling.left,
        ResizeEdge::Right => !tiling.right,
        ResizeEdge::TopLeft => !tiling.top && !tiling.left,
        ResizeEdge::TopRight => !tiling.top && !tiling.right,
        ResizeEdge::BottomLeft => !tiling.bottom && !tiling.left,
        ResizeEdge::BottomRight => !tiling.bottom && !tiling.right,
    }
}
