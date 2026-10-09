mod a11y;
mod call_window;
mod composer;
mod dnd;
mod keyboard;
mod layout;
mod long_list;
mod notify;
mod rich;
mod shell;
mod support;
mod video;

use std::{path::PathBuf, sync::Arc};

use gpui::{
    App, AppContext as _, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions,
    actions, px, size,
};
use gpui_component::{Root, Theme};

use crate::{
    shell::{Gate, Shell},
    support::{BASE_FONT_PX, FONT_FAMILY, Palette},
};

actions!(
    spike,
    [Quit, NextGate, PreviousGate, ScaleUp, ScaleDown, CountShortcut, OpenServerMenu]
);

struct Arguments {
    gate: Gate,
    scale: u32,
    out_dir: PathBuf,
}

fn arguments() -> Arguments {
    let mut parsed = Arguments {
        gate: Gate::Composer,
        scale: 100,
        out_dir: std::env::var_os("SPIKE_OUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("out")),
    };
    let mut values = std::env::args().skip(1);
    while let Some(flag) = values.next() {
        match flag.as_str() {
            "--gate" => {
                if let Some(gate) = values.next().and_then(|name| Gate::parse(&name)) {
                    parsed.gate = gate;
                }
            }
            "--scale" => {
                if let Some(scale) = values.next().and_then(|value| value.parse().ok()) {
                    parsed.scale = scale;
                }
            }
            "--out" => {
                if let Some(dir) = values.next() {
                    parsed.out_dir = PathBuf::from(dir);
                }
            }
            _ => {}
        }
    }
    parsed
}

pub fn apply_scale(percent: u32, cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.font_size = px(BASE_FONT_PX * percent as f32 / 100.0);
    });
}

fn apply_theme(cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.font_family = FONT_FAMILY.into();
        theme.colors.background = Palette::stone();
        theme.colors.foreground = Palette::ink();
        theme.colors.border = Palette::border();
        theme.colors.muted_foreground = Palette::muted();
        theme.colors.primary = Palette::terracotta();
        theme.colors.primary_hover = Palette::terracotta();
        theme.colors.primary_active = Palette::terracotta();
        theme.colors.primary_foreground = Palette::white();
        theme.colors.link = Palette::terracotta();
    });
}

fn main() {
    let arguments = arguments();
    support::init_out_dir(arguments.out_dir.clone());
    support::init_logger();
    support::record(&format!(
        "start pid={} gate={:?} scale={}",
        std::process::id(),
        arguments.gate,
        arguments.scale
    ));

    let http = Arc::new(
        reqwest_client::ReqwestClient::user_agent("junto-gpui-spike").expect("http client"),
    );

    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .with_http_client(http)
        .run(move |cx: &mut App| {
            cx.set_app_identity("junto.spike.gpui-client", "Junto gpui spike");
            gpui_component::init(cx);
            let fonts = support::load_fonts(cx);
            support::note(cx, format!("fonts loaded from files: {fonts}"));
            apply_theme(cx);
            apply_scale(arguments.scale, cx);
            cx.set_global(support::prepare_assets());

            cx.bind_keys([
                KeyBinding::new("secondary-q", Quit, None),
                KeyBinding::new("secondary-]", NextGate, None),
                KeyBinding::new("secondary-[", PreviousGate, None),
                KeyBinding::new("secondary-=", ScaleUp, None),
                KeyBinding::new("secondary--", ScaleDown, None),
                KeyBinding::new("secondary-k", CountShortcut, None),
                KeyBinding::new("secondary-shift-m", OpenServerMenu, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            call_window::CallState::install(cx);
            notify::install_response_handler(cx);

            let bounds = Bounds::centered(None, size(px(1320.0), px(860.0)), cx);
            let gate = arguments.gate;
            let scale = arguments.scale;
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Junto gpui spike".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let shell = cx.new(|cx| Shell::new(gate, scale, window, cx));
                    cx.new(|cx| Root::new(shell, window, cx))
                },
            )
            .expect("main window");
            cx.activate(true);
        });
}
