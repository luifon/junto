use gpui::{
    AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, ScrollHandle,
    Styled, Subscription, WeakFocusHandle, Window, div, prelude::FluentBuilder,
};
use gpui_base::input::InputState;

use super::{Layer, Shell};
use crate::{
    assets::Icon,
    state::switcher::{Destination, SwitchView},
    text,
    theme::{theme, u},
    ui::{Button, ChannelRow, Dialog, Field, FieldEvent, Notice, ScrollArea, StyleExt, UnreadSign},
};

pub(super) struct Switcher {
    input: Entity<InputState>,
    scroll: ScrollHandle,
    selected: Option<Destination>,
    previous: Option<Layer>,
    return_focus: Option<WeakFocusHandle>,
}

impl Switcher {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            input: Field::state(text::switcher::QUERY, window, cx),
            scroll: ScrollHandle::new(),
            selected: None,
            previous: None,
            return_focus: None,
        }
    }

    pub(super) fn observe(&self, window: &mut Window, cx: &mut Context<Shell>) -> Subscription {
        Field::observe(
            &self.input,
            window,
            cx,
            |shell: &mut Shell, event, window, cx| match event {
                FieldEvent::Changed => shell.switcher_changed(cx),
                FieldEvent::Submitted => shell.choose_switcher(window, cx),
            },
        )
    }
}

impl Shell {
    pub(crate) fn open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.layer, Layer::Profile) {
            self.close_profile(window, cx);
        }
        if !matches!(self.layer, Layer::QuickSwitcher) {
            self.close_hover_menu(cx);
            self.switcher.previous = Some(std::mem::replace(&mut self.layer, Layer::QuickSwitcher));
            self.switcher.return_focus = window.focused(cx).map(|focus| focus.downgrade());
            self.switcher.selected = None;
            self.switcher
                .input
                .update(cx, |input, cx| input.set_value("", window, cx));
            let owner = cx.entity_id();
            self.state
                .update(cx, |state, cx| state.open_switcher(owner, cx));
        }
        self.switcher
            .input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(crate) fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layer = self.switcher.previous.take().unwrap_or(Layer::None);
        let owner = cx.entity_id();
        self.state
            .update(cx, |state, cx| state.close_switcher(owner, cx));
        if let Some(focus) = self
            .switcher
            .return_focus
            .take()
            .and_then(|focus| focus.upgrade())
        {
            focus.focus(window, cx);
        } else if matches!(self.layer, Layer::None) {
            self.focus.focus(window, cx);
        } else {
            self.layer_focus.focus(window, cx);
        }
        cx.notify();
    }

    fn switcher_changed(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.layer, Layer::QuickSwitcher) {
            return;
        }
        self.switcher.selected = None;
        self.switcher
            .scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)));
        let part = self.switcher.input.read(cx).value().to_string();
        let owner = cx.entity_id();
        self.state
            .update(cx, |state, cx| state.type_switcher(owner, &part, cx));
    }

    fn switcher_rows(&self, cx: &Context<Self>) -> SwitchView {
        self.state
            .read(cx)
            .switcher_view(cx.entity_id())
            .unwrap_or_default()
    }

    fn switcher_selection(&self, view: &SwitchView) -> Option<Destination> {
        self.switcher
            .selected
            .filter(|selected| view.rows.iter().any(|row| row.destination == *selected))
            .or_else(|| view.rows.first().map(|row| row.destination))
    }

    fn choose_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.switcher_rows(cx);
        if let Some(destination) = self.switcher_selection(&view) {
            self.open_switcher_destination(destination, window, cx);
        }
    }

    fn open_switcher_destination(
        &mut self,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity_id();
        if !self.state.update(cx, |state, cx| {
            state.choose_switcher(owner, destination, cx)
        }) {
            return;
        }
        self.close_switcher(window, cx);
        self.close_layer(window, cx);
        if self.detached {
            crate::notify::bring_to_front(None, cx);
        }
    }

    pub(super) fn switcher_key(
        &mut self,
        keys: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !matches!(self.layer, Layer::QuickSwitcher) || keys.modifiers.modified() {
            return false;
        }
        if matches!(keys.key.as_str(), "enter" | "space")
            && window
                .context_stack()
                .iter()
                .any(|context| context.contains("ProfileAccount"))
        {
            return false;
        }
        let forward = match keys.key.as_str() {
            "down" => true,
            "up" => false,
            "enter" => {
                self.choose_switcher(window, cx);
                cx.stop_propagation();
                return true;
            }
            "escape" => {
                self.close_switcher(window, cx);
                cx.stop_propagation();
                return true;
            }
            _ => return false,
        };
        let view = self.switcher_rows(cx);
        if let Some(selected) = self.switcher_selection(&view)
            && let Some(index) = view.rows.iter().position(|row| row.destination == selected)
        {
            let next = if forward {
                (index + 1) % view.rows.len()
            } else {
                (index + view.rows.len() - 1) % view.rows.len()
            };
            self.switcher.selected = Some(view.rows[next].destination);
            self.switcher.scroll.scroll_to_item(next);
            cx.notify();
        }
        cx.stop_propagation();
        true
    }

    pub(crate) fn render_switcher(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = self.switcher_rows(cx);
        let selected = self.switcher_selection(&view);
        let colors = theme(cx);
        let mut list = ScrollArea::new(
            "switcher-results",
            &self.switcher.scroll,
            text::switcher::TITLE,
        )
        .selector("switcher-results");
        for (index, row) in view.rows.iter().enumerate() {
            let destination = row.destination;
            let icon = match destination {
                Destination::Server(_) => Icon::Layers,
                Destination::Channel(..) if row.voice => Icon::Speaker,
                Destination::Channel(..) => Icon::Hash,
                _ => Icon::Person,
            };
            let mut choice = ChannelRow::new(
                gpui::SharedString::from(format!("switcher-{destination:?}")),
                icon,
                row.name.clone(),
            )
            .detail(row.context.clone())
            .selected(selected == Some(destination))
            .unread(row.unread.then(|| UnreadSign {
                mentions: None,
                label: text::switcher::UNREAD.into(),
            }))
            .on_click(cx.listener(move |shell, _, window, cx| {
                shell.open_switcher_destination(destination, window, cx)
            }));
            if let Some(account) = row.account_id {
                choice = choice
                    .avatar(account.as_uuid().as_u128())
                    .presence(self.faces(destination.server()).presence(account))
                    .name_color(self.faces(destination.server()).color(account))
                    .profile(crate::ui::OpenProfile {
                        server: destination.server(),
                        account,
                    })
                    .picture(self.faces(destination.server()).image(account));
            }
            list = list.child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("switcher-row:{index}"))
                    .child(choice),
            );
        }
        let owner = cx.entity_id();
        list = list
            .when(view.rows.is_empty() && !view.reading, |list| {
                list.child(Notice::new(text::switcher::EMPTY).selector("switcher-empty"))
            })
            .when(view.limited, |list| {
                list.child(Notice::new(text::switcher::LIMITED).selector("switcher-limited"))
            })
            .when(view.reading, |list| {
                list.child(Notice::new(text::switcher::READING).selector("switcher-reading"))
            })
            .when(view.failed, |list| {
                list.child(
                    Notice::error(text::switcher::FAILED)
                        .selector("switcher-failed")
                        .action(
                            Button::new("switcher-retry")
                                .label(text::TRY_AGAIN)
                                .selector("switcher-retry")
                                .on_click(cx.listener(move |shell, _, _, cx| {
                                    shell.state.update(cx, |state, cx| {
                                        state.more_switcher(owner, true, cx)
                                    })
                                })),
                        ),
                )
            });
        Dialog::new(text::switcher::TITLE, self.layer_focus())
            .selector("quick-switcher")
            .on_close(Self::handler(cx, |shell, window, cx| {
                shell.close_switcher(window, cx)
            }))
            .child(
                div()
                    .typeface(colors.types.small, &colors)
                    .ink(colors.colors.ink_2)
                    .child(text::switcher::HELP),
            )
            .child(div().mt(u(10.)).child(
                Field::new(&self.switcher.input, text::switcher::QUERY).selector("switcher-query"),
            ))
            .child(div().mt(u(8.)).h(u(320.)).flex().flex_col().child(list))
            .when(view.more, |dialog| {
                dialog.child(
                    Button::new("switcher-more")
                        .label(text::switcher::MORE)
                        .selector("switcher-more")
                        .disabled(view.reading)
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            shell
                                .state
                                .update(cx, |state, cx| state.more_switcher(owner, false, cx))
                        })),
                )
            })
            .into_any_element()
    }
}
