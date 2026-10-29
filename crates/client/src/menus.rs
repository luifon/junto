use crate::{
    app::{CloseWindow, Quit},
    shell::{Open, SettingsSection, Shell},
    text,
};
use gpui::KeyBinding;
use gpui::{App, Menu, MenuItem, OsAction, Window, actions};
use gpui_base::input;

actions!(
    junto,
    [
        About,
        Settings,
        KeyboardShortcuts,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        ZoomWindow,
        FullScreen,
        BringAllToFront,
        ZoomIn,
        ZoomOut,
        ResetZoom
    ]
);

struct MenuState {
    fullscreen: bool,
}
impl gpui::Global for MenuState {}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(MenuState { fullscreen: false });
    install(false, cx);
    cx.on_action(|_: &Settings, cx| open_settings(None, cx));
    cx.on_action(|_: &About, cx| open_settings(Some(SettingsSection::About), cx));
    cx.on_action(|_: &KeyboardShortcuts, cx| open_settings(Some(SettingsSection::Keybinds), cx));
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &BringAllToFront, cx| {
        cx.defer(|cx| {
            let active = cx.active_window();
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            if let Some(active) = active {
                let _ = active.update(cx, |_, window, _| window.activate_window());
            } else {
                crate::notify::bring_to_front(None, cx);
            }
            cx.activate(true);
        });
    });
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, Some("Shell")),
        KeyBinding::new("ctrl-cmd-f", FullScreen, Some("Shell")),
    ]);
    sync_shortcuts(&Default::default(), cx);
}

fn open_settings(section: Option<SettingsSection>, cx: &mut App) {
    cx.defer(move |cx| {
        crate::notify::bring_to_front(None, cx);
        if let Some(main) = crate::app::main_window(cx) {
            let _ = main.update(cx, |root, window, cx| {
                if let Ok(root) = root.downcast::<gpui_base::Root>()
                    && let Ok(shell) = root.read(cx).view().clone().downcast::<Shell>()
                {
                    shell.update(cx, |shell, cx| {
                        shell.open(Open::Settings, window, cx);
                        if let Some(section) = section {
                            shell.open_settings_section(section, window, cx);
                        }
                    });
                }
            });
        }
    });
}

pub(crate) fn refresh(window: &Window, cx: &mut App) {
    if !window.is_window_active() {
        return;
    }
    let fullscreen = window.is_fullscreen();
    if cx
        .try_global::<MenuState>()
        .is_some_and(|state| state.fullscreen != fullscreen)
    {
        cx.set_global(MenuState { fullscreen });
        install(fullscreen, cx);
    }
}

fn menu(name: &'static str, items: Vec<MenuItem>) -> Menu {
    Menu {
        name: name.into(),
        items,
        disabled: false,
    }
}

fn configuration(fullscreen: bool) -> Vec<Menu> {
    vec![
        menu(
            text::APP_NAME,
            vec![
                MenuItem::action(text::menus::ABOUT, About),
                MenuItem::separator(),
                MenuItem::action(text::menus::SETTINGS, Settings),
                MenuItem::separator(),
                MenuItem::submenu(menu(text::menus::SERVICES, vec![])),
                MenuItem::separator(),
                MenuItem::action(text::menus::HIDE, Hide),
                MenuItem::action(text::menus::HIDE_OTHERS, HideOthers),
                MenuItem::action(text::menus::SHOW_ALL, ShowAll),
                MenuItem::separator(),
                MenuItem::action(text::QUIT, Quit),
            ],
        ),
        menu(
            text::menus::FILE,
            vec![MenuItem::action(text::menus::CLOSE_WINDOW, CloseWindow)],
        ),
        menu(
            text::menus::EDIT,
            vec![
                MenuItem::os_action(text::menus::UNDO, input::Undo, OsAction::Undo),
                MenuItem::os_action(text::menus::REDO, input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action(text::menus::CUT, input::Cut, OsAction::Cut),
                MenuItem::os_action(text::menus::COPY, input::Copy, OsAction::Copy),
                MenuItem::os_action(text::menus::PASTE, input::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action(
                    text::menus::SELECT_ALL,
                    input::SelectAll,
                    OsAction::SelectAll,
                ),
            ],
        ),
        menu(
            text::menus::VIEW,
            vec![
                MenuItem::action(text::menus::ZOOM_IN, ZoomIn),
                MenuItem::action(text::menus::ZOOM_OUT, ZoomOut),
                MenuItem::action(text::menus::ACTUAL_SIZE, ResetZoom),
                MenuItem::separator(),
                MenuItem::action(
                    if fullscreen {
                        text::menus::EXIT_FULL_SCREEN
                    } else {
                        text::menus::ENTER_FULL_SCREEN
                    },
                    FullScreen,
                ),
            ],
        ),
        menu(
            text::menus::WINDOW,
            vec![
                MenuItem::action(text::menus::MINIMIZE, Minimize),
                MenuItem::action(text::menus::ZOOM, ZoomWindow),
                MenuItem::separator(),
                MenuItem::action(text::menus::BRING_ALL_TO_FRONT, BringAllToFront),
            ],
        ),
        menu(
            text::menus::HELP,
            vec![MenuItem::action(
                text::menus::KEYBOARD_SHORTCUTS,
                KeyboardShortcuts,
            )],
        ),
    ]
}

fn install(fullscreen: bool, cx: &App) {
    cx.set_menus(configuration(fullscreen));
    #[cfg(target_os = "macos")]
    junto_client_core::menu_bar::register_application_menus(
        text::APP_NAME,
        text::menus::SERVICES,
        text::menus::HELP,
    );
}

pub(crate) fn sync_shortcuts(bindings: &junto_client_core::keybinds::Keybinds, cx: &mut App) {
    use junto_client_core::keybinds::KeyAction;
    if !cx.has_global::<MenuState>() {
        return;
    }
    let mut kept: Vec<_> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| {
            let action = binding.action().as_any();
            !action.is::<Settings>()
                && !action.is::<ZoomIn>()
                && !action.is::<ZoomOut>()
                && !action.is::<ResetZoom>()
        })
        .cloned()
        .collect();
    for action in [
        KeyAction::OpenSettings,
        KeyAction::ZoomIn,
        KeyAction::ZoomOut,
        KeyAction::ResetZoom,
    ] {
        let Some(keys) = bindings.get(action).keys else {
            continue;
        };
        if bindings.action_for(&keys) != Some(action) {
            continue;
        }
        let stroke = keys
            .to_string()
            .split('+')
            .map(|key| match key {
                "control" => "ctrl",
                "meta" => "cmd",
                "comma" => ",",
                "slash" => "/",
                "equal" => "=",
                "minus" => "-",
                "bracketleft" => "[",
                "bracketright" => "]",
                other => other,
            })
            .collect::<Vec<_>>()
            .join("-");
        kept.push(match action {
            KeyAction::OpenSettings => KeyBinding::new(&stroke, Settings, None),
            KeyAction::ZoomIn => KeyBinding::new(&stroke, ZoomIn, Some("Shell")),
            KeyAction::ZoomOut => KeyBinding::new(&stroke, ZoomOut, Some("Shell")),
            KeyAction::ResetZoom => KeyBinding::new(&stroke, ResetZoom, Some("Shell")),
            _ => continue,
        });
    }
    cx.clear_key_bindings();
    cx.bind_keys(kept);
    install(cx.global::<MenuState>().fullscreen, cx);
}
