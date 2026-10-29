use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, ScrollHandle,
    Styled, Subscription, Task, Window, div,
};
use gpui_base::input::InputState;
use junto_client_core::Owner;
use junto_client_core::address::ServerAddress;
use junto_client_core::links::{self, AppLink};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{MessageId, ServerId};
use junto_client_core::protocol::message::{Block, Document};
use junto_client_core::protocol::origin::Origin;

use super::{Layer, Shell};
use crate::{
    assets::Icon,
    state::{
        feed::Names,
        forwarding::{Destination, Preview},
    },
    text,
    theme::{Theme, theme, u},
    ui::{
        Button, ButtonKind, ChannelRow, Dialog, Field, Notice, ScrollArea, StyleExt, dialog_footer,
    },
};

pub(crate) struct ForwardDialog {
    pub server_id: ServerId,
    pub owner: Owner,
    origin: Origin,
    message: MessageId,
    find: Entity<InputState>,
    selected: Option<Destination>,
    request: Option<Task<()>>,
    error: Option<String>,
    scroll: ScrollHandle,
    _find: Subscription,
}

fn uri(origin: &Origin, message_id: MessageId) -> Option<String> {
    Some(links::format(&AppLink::Message {
        server: ServerAddress::parse(origin.as_str()).ok()?,
        id: message_id,
    }))
}

impl Shell {
    pub(super) fn open_forward(
        &mut self,
        server_id: ServerId,
        message: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.state.read(cx).server(server_id) else {
            return;
        };
        let Some(owner) = server.owner() else {
            return;
        };
        let find = Field::state(text::FORWARD_FIND, window, cx);
        let subscription = cx.subscribe(&find, |_, _, _: &gpui_base::input::InputEvent, cx| {
            cx.notify()
        });
        let dialog = ForwardDialog {
            server_id,
            owner,
            origin: server.entry().address.origin().clone(),
            message,
            find: find.clone(),
            selected: None,
            request: None,
            error: None,
            scroll: ScrollHandle::new(),
            _find: subscription,
        };
        self.open_layer(Layer::Forward(Box::new(dialog)), window, cx);
        self.focus_field(&find, window, cx);
    }

    pub(super) fn render_forward_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layer::Forward(dialog) = &self.layer else {
            return div().into_any_element();
        };
        let find = dialog.find.clone();
        let query = find.read(cx).value().to_lowercase();
        let selected_target = dialog
            .selected
            .clone()
            .filter(|target| self.state.read(cx).forward_destination_allowed(target))
            .map(|mut target| {
                target.label = self.state.read(cx).forward_destination_label(&target);
                target
            });
        let selected = selected_target
            .as_ref()
            .map(|target| (target.server_id, target.place));
        let waiting = dialog.request.is_some();
        let error = dialog.error.clone();
        let scroll = dialog.scroll.clone();
        let mut targets = self.state.read(cx).forward_destinations();
        if let Some(target) = selected_target
            && !targets
                .iter()
                .any(|row| row.server_id == target.server_id && row.place == target.place)
        {
            targets.push(target);
        }
        let thread_link = matches!(
            links::parse(find.read(cx).value().as_ref()),
            Some(AppLink::Message { .. })
        );
        let theme = theme(cx);
        let height = (window.viewport_size().height - u(360.0).to_pixels(window.rem_size())).clamp(
            u(80.0).to_pixels(window.rem_size()),
            u(260.0).to_pixels(window.rem_size()),
        );
        let mut choices = ScrollArea::new("forward-destinations", &scroll, text::FORWARD_FIND)
            .selector("forward-destinations");
        let mut count = 0;
        for target in targets.into_iter().filter(|target| {
            target.label.to_lowercase().contains(&query)
                || selected == Some((target.server_id, target.place))
        }) {
            let selector = target.selector();
            let element_id = gpui::ElementId::Name(selector.clone().into());
            let chosen = selected == Some((target.server_id, target.place));
            let icon = match target.place {
                PlaceId::Channel(_) => Icon::Hash,
                PlaceId::Direct(_) => Icon::Person,
                PlaceId::Thread { .. } => Icon::Chat,
            };
            choices = choices.child(
                div().debug_selector(move || selector.clone()).child(
                    ChannelRow::new(element_id, icon, target.label.clone())
                        .selected(chosen)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Layer::Forward(dialog) = &mut this.layer {
                                dialog.selected = Some(target.clone());
                                dialog.error = None;
                            }
                            cx.notify();
                        })),
                ),
            );
            count += 1;
        }
        if count == 0 {
            choices = choices.child(
                div()
                    .p(u(12.0))
                    .ink(theme.colors.ink_3)
                    .child(text::FORWARD_NONE),
            );
        }
        Dialog::new(text::FORWARD_MESSAGE, self.layer_focus())
            .selector("forward-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }))
            .child(Field::new(&find, text::FORWARD_FIND).selector("forward-find"))
            .child(
                Button::new("forward-thread-link")
                    .label(text::FORWARD_THREAD_LINK)
                    .selector("forward-thread-link")
                    .disabled(waiting || !thread_link)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.choose_forward_thread(window, cx)),
                    ),
            )
            .child(div().h(height).flex().flex_col().child(choices))
            .child(
                crate::ui::notice_slot("forward-error-slot")
                    .centred()
                    .children(error.map(|error| Notice::error(error).selector("forward-error"))),
            )
            .child(dialog_footer([
                Button::new("forward-cancel")
                    .label(text::CANCEL)
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new("forward-send")
                    .label(text::FORWARD_SEND)
                    .selector("forward-send")
                    .kind(ButtonKind::Primary)
                    .disabled(waiting || selected.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.send_forward(window, cx)))
                    .into_any_element(),
            ]))
            .into_any_element()
    }

    fn send_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Layer::Forward(dialog) = &self.layer else {
            return;
        };
        if dialog.request.is_some() {
            return;
        }
        let Some(target) = dialog.selected.clone() else {
            return;
        };
        let (origin, id) = (dialog.origin.clone(), dialog.message);
        let receive = self.state.update(cx, |state, cx| {
            state.forward_message(origin, id, target, cx)
        });
        self.forward_request(
            async move {
                receive
                    .await
                    .unwrap_or_else(|_| Err(text::FORWARD_UNAVAILABLE.to_owned()))
                    .map(|()| None)
            },
            window,
            cx,
        );
    }

    fn choose_forward_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Layer::Forward(dialog) = &self.layer else {
            return;
        };
        if dialog.request.is_some() {
            return;
        }
        let Some(AppLink::Message { server, id }) =
            links::parse(dialog.find.read(cx).value().as_ref())
        else {
            return;
        };
        let receive = self
            .state
            .update(cx, |state, cx| state.resolve_forward_thread(server, id, cx));
        self.forward_request(
            async move {
                receive
                    .await
                    .unwrap_or_else(|_| Err(text::FORWARD_UNAVAILABLE.to_owned()))
                    .map(Some)
            },
            window,
            cx,
        );
    }

    fn forward_request(
        &mut self,
        future: impl Future<Output = Result<Option<Destination>, String>> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Layer::Forward(dialog) = &self.layer else {
            return;
        };
        let opened = dialog.find.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = future.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Layer::Forward(dialog) = &mut this.layer else {
                    return;
                };
                if dialog.find != opened {
                    return;
                }
                match result {
                    Ok(None) => this.close_layer(window, cx),
                    Ok(Some(target)) => {
                        dialog.selected = Some(target);
                        dialog.request = None;
                        dialog.error = None;
                        cx.notify();
                    }
                    Err(error) => {
                        dialog.request = None;
                        dialog.error = Some(error);
                        cx.notify();
                    }
                }
            });
        });
        if let Layer::Forward(dialog) = &mut self.layer {
            dialog.request = Some(task);
        }
        cx.notify();
    }

    pub(super) fn copied_document(
        &self,
        document: &Document,
        names: &Names,
        cx: &Context<Self>,
    ) -> String {
        document
            .blocks
            .iter()
            .map(|block| match block {
                Block::Forward { server, message_id } => {
                    match self.state.read(cx).forward_preview(server, *message_id) {
                        Preview::Read(_) => format!(
                            "{}\n{}",
                            text::FORWARDED_MESSAGE,
                            uri(server, *message_id).unwrap_or_default()
                        ),
                        _ => format!(
                            "{} · {}",
                            text::FORWARDED_MESSAGE,
                            text::FORWARD_ORIGIN_UNAVAILABLE
                        ),
                    }
                }
                block => crate::state::feed::copy_text(
                    &Document {
                        blocks: vec![block.clone()],
                    },
                    names,
                ),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub(super) fn render_forward(
        &mut self,
        origin: &Origin,
        id: MessageId,
        _theme: &Theme,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview = self.state.read(cx).forward_preview(origin, id);
        let available = matches!(&preview, Preview::Read(_));
        let (selector, heading, body) = match preview {
            Preview::Read(source) => {
                let author = source
                    .message
                    .author_name
                    .as_deref()
                    .unwrap_or(text::UNKNOWN_PERSON);
                let heading = format!("{} · {author} · {}", text::FORWARDED_MESSAGE, source.place);
                (format!("forward-message:{id}"), heading, Vec::new())
            }
            Preview::Reading => (
                format!("forward-reading:{id}"),
                text::FORWARDED_MESSAGE.to_owned(),
                vec![div().child(text::FORWARD_LOADING).into_any_element()],
            ),
            Preview::Unavailable => (
                format!("forward-unavailable:{id}"),
                text::FORWARDED_MESSAGE.to_owned(),
                vec![
                    div()
                        .child(text::FORWARD_ORIGIN_UNAVAILABLE)
                        .into_any_element(),
                ],
            ),
        };
        let address = ServerAddress::parse(origin.as_str()).ok();
        let link = uri(origin, id);
        let caption = crate::selectable::SelectableText::value(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(u(12.5))
                .child(heading.clone()),
            if available {
                link.clone().unwrap_or(heading)
            } else {
                heading
            },
        );
        let open = Button::new(("forward-origin", id.as_uuid().as_u128() as u64))
            .kind(ButtonKind::Ghost)
            .label(text::VIEW_ORIGINAL)
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(address) = &address {
                    let known = this
                        .state
                        .update(cx, |state, cx| state.open_message_link(address, id, cx));
                    if !known && let Some(link) = &link {
                        cx.open_url(link);
                    }
                }
            }));
        div()
            .debug_selector(move || selector.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.0))
                    .child(caption)
                    .when(available, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .debug_selector(move || format!("forward-origin:{id}"))
                                .child(open),
                        )
                    }),
            )
            .children(body)
            .into_any_element()
    }
}
