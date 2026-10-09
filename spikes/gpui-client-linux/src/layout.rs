use std::time::{Duration, Instant};

use gpui::{
    Anchor, App, AppContext as _, Context, DismissEvent, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, actions, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputState},
    menu::{ContextMenuExt as _, PopupMenu},
    popover::Popover,
    tooltip::Tooltip,
    v_flex,
};

use crate::{
    OpenServerMenu,
    rich::message_view,
    support::{Palette, note, tabular_numerals, u},
};

actions!(
    layout,
    [
        OpenStudio,
        OpenClientAlpha,
        OpenClientBeta,
        ServerSettings,
        ReplyInThread,
        CopyMessageText,
        MakePermanent
    ]
);

pub struct LayoutGate {
    focus_handle: FocusHandle,
    rail_collapsed: bool,
    thread_open: bool,
    server_menu_open: bool,
    server_menu: Entity<PopupMenu>,
    join_address: Entity<InputState>,
    call_started: Instant,
    last_action: SharedString,
    _tick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl LayoutGate {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let menu_context = focus_handle.clone();
        let server_menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.action_context(menu_context)
                .label("Acme Studio")
                .menu("Studio", Box::new(OpenStudio))
                .menu("Client Alpha", Box::new(OpenClientAlpha))
                .menu("Client Beta", Box::new(OpenClientBeta))
                .separator()
                .menu("Server settings", Box::new(ServerSettings))
        });
        let dismissed = cx.subscribe(&server_menu, |this, _, _: &DismissEvent, cx| {
            this.server_menu_open = false;
            note(cx, "layout: server menu dismissed");
            cx.notify();
        });
        let join_address = cx.new(|cx| InputState::new(window, cx).placeholder("host:port"));
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            focus_handle,
            rail_collapsed: false,
            thread_open: true,
            server_menu_open: false,
            server_menu,
            join_address,
            call_started: Instant::now() - Duration::from_secs(42 * 60 + 17),
            last_action: "none".into(),
            _tick: tick,
            _subscriptions: vec![dismissed],
        }
    }

    fn acted(&mut self, what: &'static str, cx: &mut Context<Self>) {
        self.last_action = what.into();
        note(cx, format!("layout: action {what}"));
        cx.notify();
    }

    fn open_server_menu(&mut self, how: &'static str, cx: &mut Context<Self>) {
        if !self.server_menu_open {
            self.server_menu_open = true;
            note(cx, format!("layout: server menu opened by {how}"));
            cx.notify();
        }
    }

    fn open_join_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.join_address.clone();
        note(cx, "layout: join dialog opened");
        window.open_dialog(cx, move |dialog, _, _| {
            let input_for_ok = input.clone();
            dialog
                .title("Join a server")
                .child("Enter the address your admin gave you.")
                .child(Input::new(&input))
                .on_ok(move |_, _, cx| {
                    let value = input_for_ok.read(cx).value();
                    note(cx, format!("layout: join dialog confirmed with {value:?}"));
                    true
                })
                .on_cancel(|_, _, cx| {
                    note(cx, "layout: join dialog cancelled");
                    true
                })
        });
    }

    fn server_tile(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let collapsed = self.rail_collapsed;
        div()
            .id("server-tile-hover")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.open_server_menu("hover", cx);
                }
            }))
            .child(
                Popover::new("server-menu")
                    .anchor(Anchor::TopLeft)
                    .open(self.server_menu_open)
                    .track_focus(&self.server_menu.focus_handle(cx))
                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                        this.server_menu_open = *open;
                        cx.notify();
                    }))
                    .trigger(
                        Button::new("server-tile")
                            .label(if collapsed { "AS" } else { "Acme Studio" }),
                    )
                    .appearance(false)
                    .content({
                        let menu = self.server_menu.clone();
                        move |_, _, _| menu.clone()
                    }),
            )
    }

    fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let collapsed = self.rail_collapsed;
        v_flex()
            .flex_none()
            .h_full()
            .w(if collapsed { u(64.0) } else { u(200.0) })
            .bg(Palette::rail())
            .text_color(Palette::rail_text())
            .p(u(8.0))
            .gap(u(8.0))
            .child(
                h_flex()
                    .justify_between()
                    .when(!collapsed, |this| {
                        this.child(div().text_size(u(12.0)).child("Server book"))
                    })
                    .child(
                        Button::new("collapse-rail")
                            .small()
                            .label(if collapsed { "»" } else { "«" })
                            .tooltip("Collapse or expand the server book")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rail_collapsed = !this.rail_collapsed;
                                this.acted("toggle rail", cx);
                            })),
                    ),
            )
            .child(self.server_tile(cx))
            .when(!collapsed, |this| {
                this.children(["Studio", "Client Alpha", "Client Beta"].map(|name| {
                    div().pl(u(16.0)).text_size(u(14.0)).child(name)
                }))
                .child(div().mt(u(8.0)).child("Harbor"))
                .child(div().pl(u(16.0)).text_size(u(14.0)).child("Lounge"))
            })
            .child(div().flex_1())
            .child(
                Button::new("join-server")
                    .small()
                    .label(if collapsed { "+" } else { "Add server" })
                    .tooltip("Join a server by address")
                    .on_click(cx.listener(|this, _, window, cx| this.open_join_dialog(window, cx))),
            )
    }

    fn channels(&self) -> impl IntoElement + use<> {
        let elapsed = self.call_started.elapsed().as_secs();
        let timer: SharedString = format!(
            "{:02}:{:02}:{:02}",
            elapsed / 3600,
            (elapsed / 60) % 60,
            elapsed % 60
        )
        .into();
        let channel = |name: &'static str, active: bool| {
            h_flex()
                .gap(u(6.0))
                .px(u(8.0))
                .py(u(3.0))
                .rounded(u(4.0))
                .when(active, |this| this.bg(Palette::border()))
                .child(div().text_color(Palette::terracotta()).child("#"))
                .child(name)
        };
        v_flex()
            .flex_none()
            .h_full()
            .w(u(240.0))
            .bg(Palette::panel())
            .border_r_1()
            .border_color(Palette::border())
            .child(
                v_flex()
                    .p(u(12.0))
                    .child(div().text_size(u(12.0)).text_color(Palette::muted()).child("Acme Studio"))
                    .child(div().text_size(u(20.0)).child("Studio")),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px(u(8.0))
                    .gap(u(2.0))
                    .child(
                        div()
                            .text_size(u(12.0))
                            .text_color(Palette::muted())
                            .child("Text channels"),
                    )
                    .child(channel("general", true))
                    .child(channel("client-alpha", false))
                    .child(channel("deploys", false))
                    .child(channel("random", false))
                    .child(
                        div()
                            .mt(u(10.0))
                            .text_size(u(12.0))
                            .text_color(Palette::muted())
                            .child("Voice channels"),
                    )
                    .children(["Daily", "Focus room", "HR"].map(|name| {
                        h_flex().gap(u(6.0)).px(u(8.0)).py(u(3.0)).child("🔊").child(name)
                    })),
            )
            .child(
                v_flex()
                    .flex_none()
                    .p(u(10.0))
                    .gap(u(4.0))
                    .bg(Palette::green())
                    .text_color(Palette::white())
                    .child(
                        h_flex()
                            .justify_between()
                            .child("In call · Daily")
                            .child(
                                div()
                                    .id("timer-tabular")
                                    .font_features(tabular_numerals())
                                    .child(timer.clone())
                                    .tooltip(|window, cx| {
                                        Tooltip::new("Call time, tabular numerals (tnum)")
                                            .build(window, cx)
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .text_size(u(12.0))
                            .child("proportional numerals")
                            .child(div().child(timer)),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .text_size(u(12.0))
                            .child("tnum 11:11:11")
                            .child(div().font_features(tabular_numerals()).child("11:11:11")),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .text_size(u(12.0))
                            .child("no tnum 11:11:11")
                            .child(div().child("11:11:11")),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .p(u(10.0))
                    .gap(u(8.0))
                    .border_t_1()
                    .border_color(Palette::border())
                    .child(
                        div()
                            .size(u(32.0))
                            .rounded(u(6.0))
                            .bg(Palette::terracotta()),
                    )
                    .child(v_flex().child("Luca").child(
                        div().text_size(u(12.0)).text_color(Palette::muted()).child("@luca"),
                    )),
            )
    }

    fn content(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let messages = [
            ("Ana", "The **v0.9** checklist is up. Build: `cargo build --release`"),
            (
                "Bruno",
                "The export worker ran out of memory on large rooms. Now it drops muted tracks.",
            ),
            (
                "Diego",
                "Should recordings keep one track per speaker, or only the mixdown?",
            ),
            (
                "Luca",
                "[Bruno](mention:Bruno) can you review the retention settings before lunch?",
            ),
        ];
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap(u(10.0))
                    .px(u(16.0))
                    .py(u(10.0))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(div().text_size(u(22.0)).child("# general"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(Palette::muted())
                            .child("Studio-wide talk. Decisions go in threads."),
                    )
                    .child(
                        Button::new("pins")
                            .small()
                            .label("Pins")
                            .tooltip("Pinned messages")
                            .on_click(cx.listener(|this, _, _, cx| this.acted("pins", cx))),
                    )
                    .child(
                        Button::new("thread-toggle")
                            .small()
                            .label("Thread")
                            .tooltip("Show or hide the right panel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.thread_open = !this.thread_open;
                                this.acted("toggle thread panel", cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("layout-messages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(u(16.0))
                    .gap(u(12.0))
                    .children(messages.into_iter().enumerate().map(|(index, (author, text))| {
                        h_flex()
                            .id(("layout-message", index))
                            .items_start()
                            .gap(u(10.0))
                            .p(u(4.0))
                            .rounded(u(4.0))
                            .hover(|style| style.bg(Palette::panel()))
                            .child(
                                div()
                                    .flex_none()
                                    .size(u(36.0))
                                    .rounded(u(6.0))
                                    .bg(Palette::ochre()),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().font_weight(gpui::FontWeight::BOLD).child(author))
                                    .child(message_view(("layout-text", index), text)),
                            )
                            .context_menu(|menu, _, _| {
                                menu.menu("Reply in thread", Box::new(ReplyInThread))
                                    .menu("Copy text", Box::new(CopyMessageText))
                                    .separator()
                                    .menu("Make permanent", Box::new(MakePermanent))
                            })
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .m(u(12.0))
                    .p(u(10.0))
                    .rounded(u(6.0))
                    .border_1()
                    .border_color(Palette::border())
                    .bg(Palette::white())
                    .text_color(Palette::muted())
                    .child(format!(
                        "Message #general    (last action: {})",
                        self.last_action
                    )),
            )
    }

    fn thread_panel(&self) -> impl IntoElement + use<> {
        v_flex()
            .flex_none()
            .h_full()
            .w(u(320.0))
            .bg(Palette::panel())
            .border_l_1()
            .border_color(Palette::border())
            .p(u(12.0))
            .gap(u(8.0))
            .child(div().text_size(u(18.0)).child("Thread  # general"))
            .child(message_view(
                "thread-root",
                "Should recordings keep one track per speaker, or only the mixdown? Storage is the tradeoff.",
            ))
            .child(div().text_size(u(12.0)).text_color(Palette::muted()).child("4 replies"))
            .child(message_view(
                "thread-reply",
                "One per speaker. Transcripts are much better with separate tracks.",
            ))
    }
}

impl Focusable for LayoutGate {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LayoutGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .id("layout")
            .key_context("Layout")
            .track_focus(&self.focus_handle)
            .size_full()
            .items_start()
            .on_action(cx.listener(|this, _: &OpenServerMenu, _, cx| {
                this.open_server_menu("keyboard shortcut", cx)
            }))
            .on_action(cx.listener(|this, _: &OpenStudio, _, cx| this.acted("menu: Studio", cx)))
            .on_action(
                cx.listener(|this, _: &OpenClientAlpha, _, cx| this.acted("menu: Client Alpha", cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenClientBeta, _, cx| this.acted("menu: Client Beta", cx)),
            )
            .on_action(
                cx.listener(|this, _: &ServerSettings, _, cx| this.acted("menu: Server settings", cx)),
            )
            .on_action(
                cx.listener(|this, _: &ReplyInThread, _, cx| this.acted("context: Reply in thread", cx)),
            )
            .on_action(
                cx.listener(|this, _: &CopyMessageText, _, cx| this.acted("context: Copy text", cx)),
            )
            .on_action(
                cx.listener(|this, _: &MakePermanent, _, cx| this.acted("context: Make permanent", cx)),
            )
            .child(self.rail(cx))
            .child(self.channels())
            .child(self.content(cx))
            .when(self.thread_open, |this| this.child(self.thread_panel()))
    }
}
