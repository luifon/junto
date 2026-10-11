//! Starting the application: the fonts, the theme, the keys, and the main window.

use gpui::{
    App, AppContext as _, Bounds, Entity, KeyBinding, Menu, MenuItem, TitlebarOptions,
    WindowBounds, WindowHandle, WindowOptions, actions, point, px, size,
};
use gpui_base::Root;
use junto_client_core::Client;

use crate::shell::Shell;
use crate::state::{AppState, Timing};
use crate::text;
use crate::theme::{self, Theme};

actions!(junto, [Quit]);

/// The size of a new main window and its smallest size, in pixels at 100 %.
const WINDOW_SIZE: (f32, f32) = (1440.0, 900.0);
const WINDOW_MIN_SIZE: (f32, f32) = (720.0, 480.0);

/// The identifier of the application for the desktop: the window class on Linux and the
/// name of its desktop entry (ADR 0701).
pub const APP_ID: &str = "chat.junto.Junto";

/// How the main window opens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowStart {
    /// The interface scale of this start. Absent: the stored setting.
    pub scale_percent: Option<u32>,
    /// Bring the window to the front and give it the keyboard focus.
    pub activate: bool,
    pub timing: Timing,
}

impl Default for WindowStart {
    fn default() -> Self {
        Self {
            scale_percent: None,
            activate: true,
            timing: Timing::default(),
        }
    }
}

/// Prepares an application: the component library, the fonts and the theme. A test and the
/// screen tool call it too.
pub fn init(cx: &mut App) -> gpui::Result<()> {
    gpui_base::init(cx);
    crate::assets::load_fonts(cx)?;
    theme::install(Theme::stone(), cx);
    // A write of a file that a window started is done before the application ends.
    cx.on_app_quit(|cx| {
        crate::state::finish_writes(cx);
        async {}
    })
    .detach();
    cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    Ok(())
}

/// The state of the application over an opened client.
pub fn state(client: Client, start: WindowStart, cx: &mut App) -> Entity<AppState> {
    let state = cx.new(|cx| AppState::new(client, start.timing, cx));
    if let Some(scale) = start.scale_percent {
        state.update(cx, |state, cx| state.use_scale_for_this_start(scale, cx));
    }
    state
}

/// Opens the main window.
pub fn open_main_window(
    state: Entity<AppState>,
    start: WindowStart,
    cx: &mut App,
) -> gpui::Result<WindowHandle<Root>> {
    cx.set_menus(vec![Menu {
        name: text::APP_NAME.into(),
        items: vec![MenuItem::action(text::QUIT, Quit)],
        disabled: false,
    }]);
    let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
    let scale = state.read(cx).settings().scale_percent;
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(text::APP_NAME.into()),
                // The server book reaches the top edge of the window; macOS draws the
                // window controls over it.
                appears_transparent: true,
                traffic_light_position: Some(point(px(12.0), px(13.0))),
            }),
            focus: start.activate,
            show: true,
            app_id: Some(APP_ID.to_owned()),
            window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
            ..WindowOptions::default()
        },
        move |window, cx| {
            window.set_rem_size(theme::rem_size(scale));
            let shell = cx.new(|cx| Shell::new(state, window, cx));
            cx.new(|cx| Root::new(shell, window, cx))
        },
    )?;
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    if start.activate {
        cx.activate(true);
    }
    Ok(window)
}
