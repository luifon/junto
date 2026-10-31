use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div,
};
use junto_client_core::protocol::structure::{AfkSettings, ChannelKind};

use super::named;
use crate::shell::Shell;
use crate::state::admin::view::{AdminView, ChannelsPage};
use crate::theme::u;
use crate::ui::{Button, Notice};

impl Shell {
    pub(super) fn render_afk_settings(
        &self,
        view: &AdminView,
        page: &ChannelsPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server = view.server_id;
        let workspace = page.workspace_id;
        if !page.may_manage || !self.state.read(cx).supports_afk(server) {
            return div().into_any_element();
        }
        let mut form = div().flex().flex_col().gap(u(8.0)).py(u(8.0));
        let Some(admin) = self.state.read(cx).afk_admin(server, workspace) else {
            return form
                .child(
                    Button::new("afk-settings-open")
                        .label("AFK settings")
                        .selector("afk-settings-open")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.read_afk_admin(server, workspace, cx)
                            });
                        })),
                )
                .into_any_element();
        };
        form = form.child("AFK channel");
        if let Some(error) = admin.error {
            form = form.child(Notice::error(error).selector("afk-settings-error"));
        }
        let Some(settings) = admin.settings else {
            return form
                .child(
                    Button::new("afk-settings-retry")
                        .label(if admin.changing {
                            "Loading AFK settings"
                        } else {
                            "Retry AFK settings"
                        })
                        .disabled(admin.changing)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.read_afk_admin(server, workspace, cx)
                            });
                        })),
                )
                .into_any_element();
        };
        let mut toggle = settings.clone();
        toggle.enabled = !toggle.enabled;
        form = form
            .child(
                Button::new("afk-enabled")
                    .label(if settings.enabled {
                        "AFK enabled · Turn off"
                    } else {
                        "AFK disabled · Turn on"
                    })
                    .selector("afk-enabled")
                    .disabled(admin.changing)
                    .on_click(change(server, workspace, toggle, cx)),
            )
            .child("Move people after no microphone speech and no keyboard or mouse input for:");
        let mut times = div().flex().gap(u(8.0));
        for (minutes, selector) in [
            (5, "afk-time-5"),
            (15, "afk-time-15"),
            (30, "afk-time-30"),
            (60, "afk-time-60"),
        ] {
            let mut value = settings.clone();
            value.idle_minutes = minutes;
            times = times.child(
                Button::new(("afk-time", minutes as u64))
                    .label(format!(
                        "{minutes} minutes{}",
                        if settings.idle_minutes == minutes {
                            " ✓"
                        } else {
                            ""
                        }
                    ))
                    .selector(selector)
                    .disabled(admin.changing)
                    .on_click(change(server, workspace, value, cx)),
            );
        }
        form = form.child(times).child("Meeting mode, direct calls and group calls are exempt. The AFK channel allows listening only.");
        let mut choices = div()
            .id("afk-channel-options")
            .flex()
            .flex_col()
            .max_h(u(180.0))
            .overflow_y_scroll()
            .gap(u(4.0));
        for channel in page
            .groups
            .iter()
            .flat_map(|group| &group.channels)
            .filter(|channel| channel.kind == ChannelKind::Voice)
        {
            let mut value = settings.clone();
            value.channel_id = channel.id;
            choices = choices.child(named(
                format!("afk-channel-{}", channel.id),
                Button::new(("afk-channel", channel.id.as_uuid().as_u128() as u64))
                    .label(format!(
                        "{}{}",
                        channel.name,
                        if channel.id == settings.channel_id {
                            " ✓"
                        } else {
                            ""
                        }
                    ))
                    .disabled(admin.changing)
                    .on_click(change(server, workspace, value, cx)),
            ));
        }
        form.child(choices).into_any_element()
    }
}

fn change(
    server: junto_client_core::protocol::ids::ServerId,
    workspace: junto_client_core::protocol::ids::WorkspaceId,
    value: AfkSettings,
    cx: &mut Context<Shell>,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    cx.listener(move |this, _, _, cx| {
        this.state.update(cx, |state, cx| {
            state.change_afk_admin(server, workspace, value.clone(), cx)
        });
    })
}
