use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::book::Owner;
use junto_client_core::protocol::{
    WorkspacePermission,
    ids::{ChannelId, ServerId},
    structure::ChannelKind,
};
use uuid::Uuid;

use super::{Layer, Shell};
use crate::{
    state::view::ChannelRow,
    text,
    theme::{Theme, u},
    ui::{Button, ButtonKind, Dialog, Field, MenuItem, Notice, RowAction, StyleExt, dialog_footer},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Source {
    owner: Owner,
    channel: ChannelId,
}

pub(crate) struct Editor {
    source: Source,
    opened: Uuid,
    running: bool,
    error: Option<String>,
}

impl Shell {
    fn voice_status_source(
        &self,
        server: ServerId,
        channel: ChannelId,
        cx: &gpui::App,
    ) -> Option<Source> {
        let state = self.state.read(cx);
        let handle = state.server(server)?;
        let owner = handle.owner()?;
        let live = handle.live().view();
        let model = live.model.as_ref()?;
        let row = model.channel(channel)?;
        if row.channel.kind != ChannelKind::Voice {
            return None;
        }
        let permissions = model.permissions_in(channel)?;
        let can_set = permissions.contains(&WorkspacePermission::SetVoiceChannelStatus);
        let manages = permissions.contains(&WorkspacePermission::ManageChannel);
        let inside = row.call.as_ref().is_some_and(|call| {
            call.participants
                .iter()
                .any(|person| person.account_id == owner.account_id)
        });
        (can_set && (inside || manages)).then_some(Source { owner, channel })
    }

    pub(super) fn voice_status_editor_stale(&self, cx: &gpui::App) -> bool {
        let Layer::VoiceStatus(editor) = &self.layer else {
            return false;
        };
        self.voice_status_source(
            editor.source.owner.entry.server_id,
            editor.source.channel,
            cx,
        ) != Some(editor.source)
    }

    fn edit_voice_status(&mut self, source: Source, window: &mut Window, cx: &mut Context<Self>) {
        if self.voice_status_source(source.owner.entry.server_id, source.channel, cx)
            != Some(source)
        {
            return;
        }
        let status = self
            .state
            .read(cx)
            .server(source.owner.entry.server_id)
            .and_then(|server| {
                server
                    .live()
                    .view()
                    .model
                    .as_ref()?
                    .channel(source.channel)
                    .map(|channel| channel.channel.voice_status.clone())
            })
            .unwrap_or_default();
        self.voice_status_field
            .update(cx, |field, cx| field.set_value(status, window, cx));
        self.open_layer(
            Layer::VoiceStatus(Editor {
                source,
                opened: Uuid::new_v4(),
                running: false,
                error: None,
            }),
            window,
            cx,
        );
        self.focus_field(&self.voice_status_field.clone(), window, cx);
    }

    pub(super) fn voice_status_item(
        &self,
        server: ServerId,
        channel: &ChannelRow,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let source = self.voice_status_source(server, channel.id, cx)?;
        Some(
            MenuItem::new(
                "voice-status-edit",
                if channel.voice_status.is_empty() {
                    "Set voice status"
                } else {
                    "Edit voice status"
                },
            )
            .selector("voice-status-edit")
            .on_click(
                cx.listener(move |this, _, window, cx| this.edit_voice_status(source, window, cx)),
            )
            .into_any_element(),
        )
    }

    pub(super) fn voice_status_row(
        &self,
        server: ServerId,
        channel: &ChannelRow,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if channel.kind != ChannelKind::Voice || channel.voice_status.is_empty() {
            return None;
        }
        let id = channel.id;
        let value = channel.voice_status.clone();
        let source = self.voice_status_source(server, id, cx);
        let owner = self
            .state
            .read(cx)
            .server(server)
            .and_then(|server| server.owner());
        let (row, actions) = crate::ui::collect_actions(|| {
            RowAction::new(
                "Copy status",
                cx.listener(move |this, _, _, cx| {
                    if this
                        .state
                        .read(cx)
                        .server(server)
                        .and_then(|server| server.owner())
                        != owner
                    {
                        return;
                    }
                    let current = this.state.read(cx).server(server).and_then(|server| {
                        server
                            .live()
                            .view()
                            .model
                            .as_ref()?
                            .channel(id)
                            .map(|channel| channel.channel.voice_status.clone())
                    });
                    if let Some(current) = current {
                        this.copy_text(current, cx);
                    }
                }),
            )
            .register();
            if let Some(source) = source {
                RowAction::new(
                    "Edit voice status",
                    cx.listener(move |this, _, window, cx| {
                        this.edit_voice_status(source, window, cx)
                    }),
                )
                .register();
            }
            div()
                .min_w_0()
                .truncate()
                .pl(u(38.0))
                .pr(u(10.0))
                .pb(u(4.0))
                .typeface(theme.types.small, theme)
                .child(text::label(&value))
        });
        let key: SharedString = format!("voice-status:{owner:?}:{id}").into();
        let row = div()
            .id(key.clone())
            .focusable()
            .tab_stop(true)
            .role(Role::ListItem)
            .debug_selector(move || format!("voice-status:{id}"))
            .child(row);
        Some(crate::ui::bind_action_menu(row, key, actions, window, cx).into_any_element())
    }

    pub(super) fn save_voice_status(
        &mut self,
        clear: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.voice_status_editor_stale(cx) {
            self.close_layer(window, cx);
            return;
        }
        let value = if clear {
            String::new()
        } else {
            self.voice_status_field.read(cx).value().trim().to_owned()
        };
        let Layer::VoiceStatus(editor) = &mut self.layer else {
            return;
        };
        if editor.running {
            return;
        }
        if value.chars().count() > 500 || value.chars().any(char::is_control) {
            editor.error = Some("Use at most 500 characters on one line.".to_owned());
            cx.notify();
            return;
        }
        let source = editor.source;
        let opened = editor.opened;
        let Some(server) = self.state.read(cx).server(source.owner.entry.server_id) else {
            return;
        };
        let api = server.api();
        editor.running = true;
        editor.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = api.set_voice_channel_status(source.channel, &value).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if !matches!(&this.layer, Layer::VoiceStatus(editor) if editor.opened == opened) {
                    return;
                }
                if this.voice_status_editor_stale(cx) {
                    this.close_layer(window, cx);
                    return;
                }
                match result {
                    Ok(_) => this.close_layer(window, cx),
                    Err(_) => {
                        if let Layer::VoiceStatus(editor) = &mut this.layer {
                            editor.running = false;
                            editor.error =
                                Some("The voice status could not be saved. Try again.".to_owned());
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(super) fn voice_status_changed(&mut self, cx: &mut Context<Self>) {
        if let Layer::VoiceStatus(editor) = &mut self.layer {
            editor.error = None;
            cx.notify();
        }
    }

    pub(super) fn render_voice_status(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Layer::VoiceStatus(editor) = &self.layer else {
            return div().into_any_element();
        };
        let running = editor.running;
        Dialog::new("Voice channel status", self.layer_focus())
            .selector("voice-status-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }))
            .child(Field::new(&self.voice_status_field, "Status").selector("voice-status-input"))
            .children(editor.error.clone().map(|error| {
                div()
                    .debug_selector(|| "voice-status-error".to_owned())
                    .child(Notice::error(error))
            }))
            .child(dialog_footer([
                Button::new("voice-status-clear")
                    .label("Clear status")
                    .selector("voice-status-clear")
                    .disabled(running)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.save_voice_status(true, window, cx)),
                    )
                    .into_any_element(),
                Button::new("voice-status-cancel")
                    .label(text::CANCEL)
                    .selector("voice-status-cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new("voice-status-save")
                    .kind(ButtonKind::Primary)
                    .label(text::SAVE)
                    .selector("voice-status-save")
                    .disabled(running)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.save_voice_status(false, window, cx)
                        }),
                    )
                    .into_any_element(),
            ]))
            .into_any_element()
    }
}
