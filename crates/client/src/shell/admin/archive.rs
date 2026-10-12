//! The page of the archived channels and of the retention of a workspace, and the page of
//! the archive, the export and the deletion of one channel (ADR 0410, ADR 0411, ADR 0409).

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div,
};
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};
use junto_client_core::protocol::structure::ChannelKind;

use super::{
    Form, FormField, Mode, Question, detail_or, group_label, line, line_text, named, page_title,
    refusal, row_action, sentence,
};
use crate::assets::Icon;
use crate::shell::{Pane, Shell};
use crate::state::admin::view::{
    AdminView, ArchivePage, ArchivedLine, ChannelArchivePage, ChannelSettings, Listed, Scope,
    ServerPage,
};
use crate::state::admin::{AdminPage, AskKind, ChannelPage, Made};
use crate::state::view::ArchivedState;
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, Notice, StyleExt, icon};

/// The smallest number of days of each setting, and the sentence that states its range.
const fn days_range(what: AskKind) -> (u32, &'static str) {
    match what {
        AskKind::ArchivePeriod => (7, text::admin::ARCHIVE_DAYS_RANGE),
        _ => (1, text::admin::RETENTION_DAYS_RANGE),
    }
}

const MOST_DAYS: u32 = 36_500;

/// The days that a person typed, when they are a number in the range of the setting.
fn typed_days(text: &str, what: AskKind) -> Result<u32, &'static str> {
    let (least, range) = days_range(what);
    match text.parse::<u32>() {
        Ok(days) if (least..=MOST_DAYS).contains(&days) => Ok(days),
        _ => Err(range),
    }
}

impl Shell {
    /// The archive state of the channel that the main area shows.
    pub(crate) fn shown_archived(&self) -> Option<ArchivedState> {
        self.view
            .as_ref()
            .and_then(|view| view.workspace())
            .and_then(|workspace| workspace.archived)
    }

    /// The statement in place of the composer of an archived channel: nobody can write
    /// in it, when the server deletes it, and the restore for a person who may do it.
    pub(crate) fn render_archived_bar(
        &mut self,
        pane: Pane,
        archived: ArchivedState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let target = self.view.as_ref().and_then(|view| {
            let server_id = view.selected.as_ref()?.id;
            let channel_id = view.workspace()?.channel.as_ref()?.id;
            Some((server_id, channel_id))
        });
        let restore = target
            .filter(|_| archived.may_restore && pane == Pane::Main)
            .map(|(server_id, channel_id)| {
                Button::new("archived-restore")
                    .label(text::admin::RESTORE)
                    .tooltip(text::admin::RESTORE_HELP)
                    .selector("archived-restore")
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| {
                            state.restore_channel(server_id, channel_id, cx);
                        });
                    }))
            });
        div()
            .id(pane.id("archived-bar"))
            .debug_selector(move || pane.id("archived-bar").to_owned())
            .flex()
            .flex_none()
            .items_center()
            .gap(u(10.0))
            .min_h(u(44.0))
            .py(u(8.0))
            .px(u(16.0))
            .border_t_1()
            .border_color(gpui::Hsla::from(colors.rule))
            .bg(gpui::Hsla::from(colors.limestone))
            .child(icon(Icon::Lock, 16.0, colors.bronze))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.control_strong, &theme)
                            .ink(colors.ink)
                            .child(text::admin::ARCHIVED_COMPOSER),
                    )
                    .child(sentence(
                        format!(
                            "{} {}",
                            text::admin::archived_since(archived.archived_at),
                            text::admin::deletes_at(archived.deletes_at)
                        ),
                        &theme,
                    )),
            )
            .children(restore)
            .into_any_element()
    }

    /// Opens the settings of the channel that the main area shows.
    pub(crate) fn open_shown_channel_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.view.as_ref().and_then(|view| {
            let workspace = view.workspace()?;
            Some((workspace.id, workspace.channel.as_ref()?.id))
        });
        if let Some((workspace_id, channel_id)) = target {
            self.open_channel_settings(workspace_id, channel_id, ChannelPage::Overview, window, cx);
        }
    }

    /// Asks for an export of the direct conversation that the main area shows, and opens
    /// the page of the exports, which shows its state.
    pub(crate) fn export_shown_conversation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.view.as_ref().and_then(|view| {
            let server_id = view.selected.as_ref()?.id;
            Some((server_id, view.workspace()?.conversation.as_ref()?.id))
        });
        if let Some((server_id, conversation_id)) = target {
            self.open_server_admin(AdminPage::Exports, window, cx);
            self.state.update(cx, |state, cx| {
                state.export_conversation(server_id, conversation_id, cx);
            });
        }
    }

    fn send_days(
        &mut self,
        server_id: ServerId,
        workspace_id: Option<WorkspaceId>,
        what: AskKind,
        days: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        self.admin_sent(what);
        self.state
            .update(cx, |state, cx| match (what, workspace_id) {
                (AskKind::ArchivePeriod, Some(workspace_id)) => {
                    state.set_archive_period(server_id, workspace_id, days, cx);
                }
                (AskKind::FileRetention, Some(workspace_id)) => {
                    state.set_file_retention(server_id, workspace_id, days, cx);
                }
                (AskKind::RecordingRetention, Some(workspace_id)) => {
                    state.set_recording_retention(server_id, workspace_id, days, cx);
                }
                (AskKind::DirectRetention, _) => state.set_direct_retention(server_id, days, cx),
                _ => {}
            });
    }

    fn archive_workspace(view: &AdminView) -> Option<WorkspaceId> {
        match &view.scope {
            Scope::Server {
                page: ServerPage::Archive(page),
                ..
            } => Some(page.workspace_id),
            _ => None,
        }
    }

    /// Sends the number of days of the open row.
    pub(crate) fn submit_days(
        &mut self,
        view: &AdminView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Mode::Days(what)) = self.admin.mode else {
            return;
        };
        match typed_days(&self.admin_value(cx), what) {
            Ok(days) => {
                let workspace_id = Self::archive_workspace(view);
                self.send_days(view.server_id, workspace_id, what, Some(days), cx);
            }
            Err(range) => self.admin.invalid = Some(range),
        }
        cx.notify();
    }

    /// One setting of days: its name, its value, and the form that changes it.
    #[allow(clippy::too_many_arguments)]
    fn render_days(
        &mut self,
        view: &AdminView,
        workspace_id: Option<WorkspaceId>,
        what: AskKind,
        title: &'static str,
        help: &'static str,
        selector: &'static str,
        value: Option<u32>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let content: AnyElement = match self.admin.mode {
            Some(Mode::Days(open)) if open == what => {
                let never = Button::new("days-never")
                    .label(text::admin::NEVER)
                    .selector("days-never")
                    .disabled(view.sending(what))
                    .h(u(30.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send_days(server_id, workspace_id, what, None, cx);
                    }));
                self.render_form(
                    Form {
                        field: FormField::Value,
                        label: text::admin::DAYS_FIELD,
                        field_selector: "days-field",
                        send: text::SAVE,
                        sending: view.sending(what),
                        help: Some(help),
                        extra: vec![never.into_any_element()],
                    },
                    theme,
                    cx,
                )
                .into_any_element()
            }
            _ => div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(u(8.0))
                .child(line_text(
                    title,
                    Some(SharedString::from(text::admin::days(value))),
                    theme,
                ))
                .child(row_action(
                    selector,
                    text::admin::CHANGE,
                    selector.to_owned(),
                    None,
                    cx.listener(move |this, _, window, cx| {
                        this.set_admin_mode(Some(Mode::Days(what)), "", window, cx);
                    }),
                ))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(line(theme).child(content))
            .children(refusal(view.refused(what), "days-refused"))
            .into_any_element()
    }

    fn render_archived_row(
        &mut self,
        view: &AdminView,
        page: &ArchivePage,
        row: &ArchivedLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, workspace_id, channel_id) = (view.server_id, page.workspace_id, row.id);
        let number = channel_id.as_uuid().as_u128() as u64;
        let detail = format!(
            "{} {}",
            text::admin::archived_since(row.archived_at),
            text::admin::deletes_at(row.deletes_at)
        );
        let glyph = match row.kind {
            ChannelKind::Text => Icon::Hash,
            ChannelKind::Voice => Icon::Speaker,
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(named(
                format!("archived-row-{channel_id}"),
                line(theme)
                    .child(icon(glyph, 15.0, theme.colors.ink_2))
                    .child(line_text(row.name.clone(), Some(detail.into()), theme))
                    .child(row_action(
                        ("archived-open", number),
                        text::admin::OPEN,
                        format!("archived-open-{channel_id}"),
                        None,
                        cx.listener(move |this, _, window, cx| {
                            this.state.update(cx, |state, cx| {
                                state.select_channel(workspace_id, channel_id, cx);
                            });
                            this.close_layer(window, cx);
                        }),
                    ))
                    .child(row_action(
                        ("archived-restore", number),
                        text::admin::RESTORE,
                        format!("archived-restore-{channel_id}"),
                        (!page.may_manage)
                            .then_some(text::MANAGE_NO_PERMISSION)
                            .or(view
                                .sending(AskKind::RestoreChannel(channel_id))
                                .then_some(text::admin::SENDING)),
                        cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.restore_channel(server_id, channel_id, cx);
                            });
                        }),
                    ))
                    .child(row_action(
                        ("archived-settings", number),
                        text::admin::SETTINGS,
                        format!("archived-settings-{channel_id}"),
                        None,
                        cx.listener(move |this, _, window, cx| {
                            this.open_channel_settings(
                                workspace_id,
                                channel_id,
                                ChannelPage::Archive,
                                window,
                                cx,
                            );
                        }),
                    )),
            ))
            .children(refusal(
                view.refused(AskKind::RestoreChannel(channel_id)),
                "archived-refused",
            ))
            .into_any_element()
    }

    pub(crate) fn render_archive_page(
        &mut self,
        view: &AdminView,
        page: &ArchivePage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::Archive),
                text::admin::ARCHIVE_PAGE_HELP,
                &theme,
            ))
            .child(group_label(text::admin::ARCHIVED_CHANNELS, &theme));
        match &page.channels {
            Listed::Loading => {
                let part = AdminPart::Archived(workspace_id);
                column = column.child(self.render_reading(server_id, part, None, &theme, cx));
            }
            Listed::Failed(failure) => {
                let part = AdminPart::Archived(workspace_id);
                column =
                    column.child(self.render_reading(server_id, part, Some(*failure), &theme, cx));
            }
            Listed::Loaded(rows) if rows.is_empty() => {
                column = column.child(
                    sentence(text::admin::NO_ARCHIVED, &theme)
                        .py(u(6.0))
                        .debug_selector(|| "no-archived".to_owned()),
                );
            }
            Listed::Loaded(rows) => {
                for row in rows {
                    column = column.child(self.render_archived_row(view, page, row, &theme, cx));
                }
                if !page.may_manage {
                    column = column.child(sentence(text::admin::RESTORE_NEEDS, &theme).mt(u(6.0)));
                }
            }
        }
        if let Some(settings) = &page.settings {
            column = column.child(group_label(text::admin::RETENTION, &theme));
            let part = AdminPart::WorkspaceSettings(workspace_id);
            match settings {
                Listed::Loading => {
                    column = column.child(self.render_reading(server_id, part, None, &theme, cx));
                }
                Listed::Failed(failure) => {
                    column = column.child(self.render_reading(
                        server_id,
                        part,
                        Some(*failure),
                        &theme,
                        cx,
                    ));
                }
                Listed::Loaded(days) => {
                    for (what, title, help, selector, value) in [
                        (
                            AskKind::ArchivePeriod,
                            text::admin::ARCHIVE_PERIOD,
                            text::admin::ARCHIVE_PERIOD_HELP,
                            "days-archive-period",
                            days.archive_period,
                        ),
                        (
                            AskKind::FileRetention,
                            text::admin::FILE_RETENTION,
                            text::admin::FILE_RETENTION_HELP,
                            "days-file-retention",
                            days.file_retention,
                        ),
                        (
                            AskKind::RecordingRetention,
                            text::admin::RECORDING_RETENTION,
                            text::admin::RECORDING_RETENTION_HELP,
                            "days-recording-retention",
                            days.recording_retention,
                        ),
                    ] {
                        column = column.child(self.render_days(
                            view,
                            Some(workspace_id),
                            what,
                            title,
                            help,
                            selector,
                            value,
                            &theme,
                            cx,
                        ));
                    }
                }
            }
        }
        if let Some(direct) = &page.direct_retention {
            let part = AdminPart::DirectRetention;
            column = match direct {
                Listed::Loading => {
                    column.child(self.render_reading(server_id, part, None, &theme, cx))
                }
                Listed::Failed(failure) => {
                    column.child(self.render_reading(server_id, part, Some(*failure), &theme, cx))
                }
                Listed::Loaded(value) => column.child(self.render_days(
                    view,
                    None,
                    AskKind::DirectRetention,
                    text::admin::DIRECT_RETENTION,
                    text::admin::DIRECT_RETENTION_HELP,
                    "days-direct-retention",
                    *value,
                    &theme,
                    cx,
                )),
            };
        }
        column.into_any_element()
    }

    /// Archive, export and deletion of one channel.
    pub(crate) fn render_channel_archive(
        &mut self,
        view: &AdminView,
        settings: &ChannelSettings,
        page: &ChannelArchivePage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let (server_id, channel_id) = (view.server_id, settings.channel_id);
        let manage = (!settings.may_manage).then_some(text::MANAGE_NO_PERMISSION);
        let mut column = div().flex().flex_col().child(page_title(
            text::admin::channel_page_name(ChannelPage::Archive),
            text::admin::CHANNEL_ARCHIVE_HELP,
            &theme,
        ));
        // Archive or restore.
        column = match page.archived {
            Some((archived_at, deletes_at)) => column
                .child(
                    div().flex_none().mb(u(6.0)).child(
                        Notice::new(format!(
                            "{} {}",
                            text::admin::archived_since(archived_at),
                            text::admin::deletes_at(deletes_at)
                        ))
                        .selector("channel-archived-state"),
                    ),
                )
                .child(
                    line(&theme)
                        .child(line_text(
                            text::admin::RESTORE,
                            detail_or(manage, text::admin::RESTORE_HELP),
                            &theme,
                        ))
                        .child(row_action(
                            "channel-restore",
                            text::admin::RESTORE,
                            "channel-restore".to_owned(),
                            manage.or(view
                                .sending(AskKind::RestoreChannel(channel_id))
                                .then_some(text::admin::SENDING)),
                            cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.restore_channel(server_id, channel_id, cx);
                                });
                            }),
                        )),
                ),
            None => match self.admin.mode {
                Some(Mode::ArchiveChannel(id)) if id == channel_id => {
                    column.child(line(&theme).child(self.render_row_question(
                        Question {
                            sentence: text::admin::archive_channel_question(&settings.name).into(),
                            confirm: text::admin::ARCHIVE,
                            confirm_selector: "channel-archive-confirm",
                            cancel_selector: "channel-archive-cancel",
                            sending: view.sending(AskKind::ArchiveChannel(channel_id)),
                        },
                        &theme,
                        cx,
                        move |this, _, cx| {
                            this.admin_sent(AskKind::ArchiveChannel(channel_id));
                            this.state.update(cx, |state, cx| {
                                state.archive_channel(server_id, channel_id, cx);
                            });
                        },
                    )))
                }
                _ => column.child(
                    line(&theme)
                        .child(line_text(
                            text::admin::ARCHIVE,
                            detail_or(manage, text::admin::ARCHIVE_HELP),
                            &theme,
                        ))
                        .child(row_action(
                            "channel-archive",
                            text::admin::ARCHIVE,
                            "channel-archive".to_owned(),
                            manage,
                            cx.listener(move |this, _, window, cx| {
                                this.set_admin_mode(
                                    Some(Mode::ArchiveChannel(channel_id)),
                                    "",
                                    window,
                                    cx,
                                );
                            }),
                        )),
                ),
            },
        };
        column = column.children(refusal(
            view.refused(AskKind::ArchiveChannel(channel_id))
                .or(view.refused(AskKind::RestoreChannel(channel_id))),
            "channel-archive-refused",
        ));
        // The export.
        column = column.child(
            line(&theme)
                .child(line_text(
                    text::admin::EXPORT,
                    detail_or(
                        (!page.may_export).then_some(text::admin::EXPORT_NEEDS),
                        text::admin::EXPORT_HELP,
                    ),
                    &theme,
                ))
                .child(row_action(
                    "channel-export",
                    text::admin::EXPORT,
                    "channel-export".to_owned(),
                    (!page.may_export)
                        .then_some(text::admin::EXPORT_NEEDS)
                        .or(view
                            .sending(AskKind::RequestExport)
                            .then_some(text::admin::SENDING)),
                    cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| {
                            state.export_channel(server_id, channel_id, cx);
                        });
                    }),
                )),
        );
        if matches!(view.made, Some(Made::Export(_))) {
            column = column.child(
                div().flex_none().mt(u(6.0)).child(
                    Notice::new(text::admin::EXPORT_ASKED)
                        .selector("export-asked")
                        .action(
                            Button::new("show-exports")
                                .label(text::admin::SHOW_EXPORTS)
                                .selector("show-exports")
                                .h(u(28.0))
                                .px(u(10.0))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_server_admin(AdminPage::Exports, window, cx);
                                })),
                        ),
                ),
            );
        }
        column = column.children(refusal(
            view.refused(AskKind::RequestExport),
            "channel-export-refused",
        ));
        // The deletion.
        let owner_only = (page.permanent && !page.is_owner).then_some(text::admin::OWNER_DELETES);
        column = match self.admin.mode {
            Some(Mode::DeleteChannel(id)) if id == channel_id => column.child(
                line(&theme).child(
                    self.render_row_question(
                        Question {
                            sentence: text::admin::delete_channel_question(
                                &settings.name,
                                page.permanent,
                            )
                            .into(),
                            confirm: text::admin::DELETE,
                            confirm_selector: "channel-delete-confirm",
                            cancel_selector: "channel-delete-cancel",
                            sending: view.sending(AskKind::DeleteChannel(channel_id)),
                        },
                        &theme,
                        cx,
                        move |this, _, cx| {
                            this.admin_sent(AskKind::DeleteChannel(channel_id));
                            this.state.update(cx, |state, cx| {
                                state.delete_channel(server_id, channel_id, cx);
                            });
                        },
                    ),
                ),
            ),
            _ => column.child(
                line(&theme)
                    .child(line_text(
                        text::admin::DELETE_CHANNEL,
                        detail_or(manage.or(owner_only), text::admin::DELETE_CHANNEL_HELP),
                        &theme,
                    ))
                    .child(row_action(
                        "channel-delete",
                        text::admin::DELETE,
                        "channel-delete".to_owned(),
                        manage.or(owner_only),
                        cx.listener(move |this, _, window, cx| {
                            this.set_admin_mode(
                                Some(Mode::DeleteChannel(channel_id)),
                                "",
                                window,
                                cx,
                            );
                        }),
                    )),
            ),
        };
        column
            .children(refusal(
                view.refused(AskKind::DeleteChannel(channel_id)),
                "channel-delete-refused",
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::typed_days;
    use crate::state::admin::AskKind;
    use crate::text;

    #[test]
    fn typed_days_are_a_number_in_the_range_of_the_setting() {
        assert_eq!(typed_days("7", AskKind::ArchivePeriod), Ok(7));
        assert_eq!(typed_days("36500", AskKind::ArchivePeriod), Ok(36_500));
        for refused in ["6", "36501", "", "ten", "-7", "7.5"] {
            assert_eq!(
                typed_days(refused, AskKind::ArchivePeriod),
                Err(text::admin::ARCHIVE_DAYS_RANGE),
                "{refused}"
            );
        }
        assert_eq!(typed_days("1", AskKind::FileRetention), Ok(1));
        assert_eq!(
            typed_days("0", AskKind::DirectRetention),
            Err(text::admin::RETENTION_DAYS_RANGE)
        );
    }
}
