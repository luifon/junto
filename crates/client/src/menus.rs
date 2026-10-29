use crate::{
    app::{CloseWindow, Quit},
    shell::{Open, SettingsSection, Shell},
    text,
};
#[cfg(target_os = "macos")]
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
        BringAllToFront
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
        KeyBinding::new("cmd-,", Settings, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, Some("Shell")),
        KeyBinding::new("ctrl-cmd-f", FullScreen, Some("Shell")),
    ]);
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
                MenuItem::action("About Junto", About),
                MenuItem::separator(),
                MenuItem::action("Settings…", Settings),
                MenuItem::separator(),
                MenuItem::submenu(menu("Services", vec![])),
                MenuItem::separator(),
                MenuItem::action("Hide Junto", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action(text::QUIT, Quit),
            ],
        ),
        menu("File", vec![MenuItem::action("Close Window", CloseWindow)]),
        menu(
            "Edit",
            vec![
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
            ],
        ),
        menu(
            "View",
            vec![MenuItem::action(
                if fullscreen {
                    "Exit Full Screen"
                } else {
                    "Enter Full Screen"
                },
                FullScreen,
            )],
        ),
        menu(
            "Window",
            vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", ZoomWindow),
                MenuItem::separator(),
                MenuItem::action("Bring All to Front", BringAllToFront),
            ],
        ),
        menu(
            "Help",
            vec![MenuItem::action("Keyboard Shortcuts", KeyboardShortcuts)],
        ),
    ]
}

fn install(fullscreen: bool, cx: &App) {
    cx.set_menus(configuration(fullscreen));
    #[cfg(target_os = "macos")]
    junto_client_core::menu_bar::register_application_menus(text::APP_NAME, "Services", "Help");
}
