//! The AFK channel of a workspace (ADR 0523), a part of the overview of the workspace: the
//! switch, the time without speech and input after which a person is moved, and the voice
//! channel that takes the person.

use gpui::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div};
use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::AfkSettings;

use super::{group_label, line, line_text, named, sentence};
use crate::assets::Icon;
use crate::shell::Shell;
use crate::text;
use crate::text::admin::afk as txt;
use crate::theme::{theme, u};
use crate::ui::{Button, ChannelRow, Notice, Segmented, Toggle};

/// The times that the page offers, in minutes.
const TIMES: [u32; 4] = [5, 15, 30, 60];

impl Shell {
    pub(super) fn render_afk_settings(
        &self,
        server: ServerId,
        workspace: WorkspaceId,
        channels: &[(ChannelId, String)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        if !self.state.read(cx).supports_afk(server) {
            return div().into_any_element();
        }
        let mut form = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(group_label(txt::TITLE, &theme));
        let Some(admin) = self.state.read(cx).afk_admin(server, workspace) else {
            return form
                .child(
                    div().flex().flex_none().child(
                        Button::new("afk-settings-open")
                            .label(txt::SHOW)
                            .selector("afk-settings-open")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.read_afk_admin(server, workspace, cx)
                                });
                            })),
                    ),
                )
                .into_any_element();
        };
        if let Some(error) = admin.error {
            form = form.child(Notice::error(error).selector("afk-settings-error"));
        }
        let Some(settings) = admin.settings else {
            return form
                .child(
                    div().flex().flex_none().child(
                        Button::new("afk-settings-retry")
                            .label(if admin.changing {
                                text::admin::READING
                            } else {
                                text::TRY_AGAIN
                            })
                            .selector("afk-settings-retry")
                            .disabled(admin.changing)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.read_afk_admin(server, workspace, cx)
                                });
                            })),
                    ),
                )
                .into_any_element();
        };
        let held = admin.changing.then_some(text::admin::SENDING);
        let mut toggle = settings.clone();
        toggle.enabled = !toggle.enabled;
        form = form.child(
            line(&theme)
                .child(line_text(txt::MOVE_IDLE.to_owned(), None, &theme))
                .child(named(
                    "afk-enabled".to_owned(),
                    Toggle::new(
                        "afk-enabled",
                        txt::MOVE_IDLE,
                        settings.enabled,
                        (text::ON, text::OFF),
                    )
                    .held(held)
                    .on_click(change(server, workspace, toggle, cx)),
                )),
        );
        let times = Segmented::new(
            "afk-time",
            txt::IDLE_TIME,
            TIMES.map(|minutes| SharedString::from(txt::minutes(minutes))),
        )
        .selected(
            TIMES
                .iter()
                .position(|minutes| *minutes == settings.idle_minutes),
        )
        .selector("afk-time");
        let times = if admin.changing {
            times
        } else {
            let shown = settings.clone();
            let select = cx.listener(move |this, index: &usize, _, cx| {
                if let Some(minutes) = TIMES.get(*index).copied() {
                    let mut value = shown.clone();
                    value.idle_minutes = minutes;
                    this.state.update(cx, |state, cx| {
                        state.change_afk_admin(server, workspace, value, cx)
                    });
                }
            });
            times.on_select(move |index, window, cx| select(&index, window, cx))
        };
        form = form
            .child(sentence(txt::IDLE_TIME, &theme).mt(u(6.0)))
            .child(div().flex().flex_none().mt(u(4.0)).child(times))
            .child(sentence(txt::CHANNEL, &theme).mt(u(10.0)));
        let mut choices = div().flex().flex_col().flex_none().mt(u(4.0));
        for (channel_id, name) in channels {
            let mut value = settings.clone();
            value.channel_id = *channel_id;
            let chosen = *channel_id == settings.channel_id;
            let row = ChannelRow::new(name.clone(), Icon::Speaker, name.clone()).selected(chosen);
            let row = if admin.changing || chosen {
                row
            } else {
                row.on_click(change(server, workspace, value, cx))
            };
            choices = choices.child(named(format!("afk-channel-{channel_id}"), row));
        }
        form.child(choices)
            .child(sentence(txt::EXEMPT, &theme).mt(u(6.0)))
            .into_any_element()
    }
}

fn change(
    server: ServerId,
    workspace: WorkspaceId,
    value: AfkSettings,
    cx: &mut Context<Shell>,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    cx.listener(move |this, _, _, cx| {
        this.state.update(cx, |state, cx| {
            state.change_afk_admin(server, workspace, value.clone(), cx)
        });
    })
}
