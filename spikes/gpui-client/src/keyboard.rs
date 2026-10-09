use gpui::{
    Anchor, App, AppContext as _, Context, DismissEvent, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, actions, div,
};
use gpui_component::{
    Sizable as _,
    button::Button,
    h_flex,
    input::{Input, InputState},
    menu::PopupMenu,
    popover::Popover,
    v_flex,
};

use crate::{
    CountShortcut, OpenServerMenu,
    support::{Palette, note, u},
};

actions!(keyboard, [PickStudio, PickClientAlpha, PickServerSettings]);

pub struct KeyboardGate {
    focus_handle: FocusHandle,
    shortcut_count: usize,
    pressed: SharedString,
    picked: SharedString,
    menu_open: bool,
    menu: Entity<PopupMenu>,
    search: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl KeyboardGate {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let menu_context = focus_handle.clone();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.action_context(menu_context)
                .label("Acme Studio")
                .menu("Studio", Box::new(PickStudio))
                .menu("Client Alpha", Box::new(PickClientAlpha))
                .separator()
                .menu("Server settings", Box::new(PickServerSettings))
        });
        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.menu_open = false;
            note(cx, "keyboard: server menu dismissed");
            cx.notify();
        });
        Self {
            focus_handle,
            shortcut_count: 0,
            pressed: "none".into(),
            picked: "none".into(),
            menu_open: false,
            menu,
            search: cx.new(|cx| InputState::new(window, cx).placeholder("Search")),
            _subscriptions: vec![dismissed],
        }
    }

    fn open_menu(&mut self, how: &'static str, cx: &mut Context<Self>) {
        if !self.menu_open {
            self.menu_open = true;
            note(cx, format!("keyboard: server menu opened by {how}"));
            cx.notify();
        }
    }

    fn pick(&mut self, what: &'static str, cx: &mut Context<Self>) {
        self.picked = what.into();
        note(cx, format!("keyboard: menu item chosen: {what}"));
        cx.notify();
    }
}

impl Focusable for KeyboardGate {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KeyboardGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let plain_button = |name: &'static str, cx: &mut Context<Self>| {
            Button::new(name)
                .small()
                .label(name)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pressed = name.into();
                    note(cx, format!("keyboard: button activated: {name}"));
                    cx.notify();
                }))
        };

        v_flex()
            .id("keyboard")
            .key_context("KeyboardGate")
            .track_focus(&self.focus_handle)
            .size_full()
            .p(u(16.0))
            .gap(u(14.0))
            .on_action(cx.listener(|this, _: &CountShortcut, _, cx| {
                this.shortcut_count += 1;
                note(cx, format!("keyboard: Cmd+K action ran, count {}", this.shortcut_count));
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenServerMenu, _, cx| {
                this.open_menu("keyboard shortcut", cx)
            }))
            .on_action(cx.listener(|this, _: &PickStudio, _, cx| this.pick("Studio", cx)))
            .on_action(
                cx.listener(|this, _: &PickClientAlpha, _, cx| this.pick("Client Alpha", cx)),
            )
            .on_action(
                cx.listener(|this, _: &PickServerSettings, _, cx| this.pick("Server settings", cx)),
            )
            .child("Tab and Shift+Tab move between the controls. Space or Enter activates a button.")
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(plain_button("First", cx))
                    .child(plain_button("Second", cx))
                    .child(div().w(u(200.0)).child(Input::new(&self.search)))
                    .child(plain_button("Third", cx))
                    .child(
                        div()
                            .id("raw-focusable")
                            .focusable()
                            .tab_stop(true)
                            .px(u(10.0))
                            .py(u(4.0))
                            .rounded(u(6.0))
                            .border_1()
                            .border_color(Palette::border())
                            .focus(|style| style.border_color(Palette::terracotta()).bg(Palette::white()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pressed = "raw div".into();
                                note(cx, "keyboard: raw div activated");
                                cx.notify();
                            }))
                            .child("Raw focusable div"),
                    ),
            )
            .child(format!("last activated control: {}", self.pressed))
            .child(format!(
                "Cmd+K is bound to an action. It ran {} times.",
                self.shortcut_count
            ))
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(
                        div()
                            .id("keyboard-server-tile-hover")
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if *hovered {
                                    this.open_menu("hover", cx);
                                }
                            }))
                            .child(
                                Popover::new("keyboard-server-menu")
                                    .anchor(Anchor::TopLeft)
                                    .open(self.menu_open)
                                    .track_focus(&self.menu.focus_handle(cx))
                                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                                        this.menu_open = *open;
                                        cx.notify();
                                    }))
                                    .trigger(
                                        Button::new("keyboard-server-tile")
                                            .small()
                                            .label("Acme Studio"),
                                    )
                                    .appearance(false)
                                    .content({
                                        let menu = self.menu.clone();
                                        move |_, _, _| menu.clone()
                                    }),
                            ),
                    )
                    .child("Hover the server tile, or press Cmd+Shift+M, to open the same menu."),
            )
            .child(format!("menu item chosen: {}", self.picked))
    }
}
