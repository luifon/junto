//! The page of the recordings of a workspace (ADR 0784): a list that is read in pages on
//! demand. It shows no player, and no control that saves or removes a recording.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
};
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::{line, named, page_title, sentence};
use crate::shell::Shell;
use crate::state::admin::AdminPage;
use crate::state::admin::view::AdminView;
use crate::state::admin::view::recordings::{RecordingLine, RecordingsPage, RecordingsRead};
use crate::text;
use crate::text::admin::recordings as txt;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, Notice, StyleExt};

/// What the page holds of the window itself.
#[derive(Debug, Default)]
pub(crate) struct Ui {
    /// The list whose first read the page asked for.
    pub(crate) asked: Option<(ServerId, WorkspaceId)>,
}

/// One recording. The name of the channel and the name of the person are text of a
/// server: each has a line of its own that is cut at the width of the row.
fn recording_row(row: &RecordingLine, theme: &Theme) -> AnyElement {
    let colors = theme.colors;
    let detail = |text: String| {
        div()
            .typeface(theme.types.small, theme)
            .ink(colors.ink_2)
            .truncate()
            .child(text)
    };
    let content = line(theme).child(
        div()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .typeface(theme.types.control_strong, theme)
                    .ink(colors.ink)
                    .truncate()
                    .child(txt::channel(row)),
            )
            .child(detail(txt::when(row)))
            .child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(colors.ink)
                    .child(txt::state(row)),
            )
            .child(detail(txt::started_by(row)))
            .child(detail(txt::transcript(
                row.transcript,
                row.transcript_lines,
            ))),
    );
    named(format!("recording-row-{}", row.id), content)
        .flex()
        .flex_col()
        .into_any_element()
}

impl Shell {
    /// The page asks for its list again when it is shown again.
    pub(crate) fn reset_recordings_ui(&mut self) {
        self.admin.recordings.asked = None;
    }

    /// Shows the end of the open page of the administration. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_to_end(&mut self) {
        self.admin.page_scroll.scroll_to_bottom();
    }

    pub(crate) fn render_recordings_page(
        &mut self,
        view: &AdminView,
        page: &RecordingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let server_id = view.server_id;
        let workspace_id = page.workspace_id;
        // The page reads the list that it shows when it is drawn first. The read is asked
        // for once for each workspace: a read that cannot start does not ask again with
        // every frame.
        if page.state == RecordingsRead::NotRead
            && self.admin.recordings.asked != Some((server_id, workspace_id))
        {
            self.admin.recordings.asked = Some((server_id, workspace_id));
            cx.defer_in(window, move |this, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.show_recordings(server_id, workspace_id, cx);
                });
            });
        }
        let mut column = div().flex().flex_col().gap(u(8.0)).child(page_title(
            text::admin::page_name(AdminPage::Recordings),
            txt::HELP,
            &theme,
        ));
        let mut rows = div().flex().flex_col().flex_none();
        for row in &page.rows {
            rows = rows.child(recording_row(row, &theme));
        }
        column = column.child(rows);
        if page.rows.is_empty() && page.state == RecordingsRead::Loaded {
            column = column.child(
                sentence(txt::NO_RECORDINGS, &theme)
                    .py(u(6.0))
                    .debug_selector(|| "recordings-empty".to_owned()),
            );
        }
        // The end of the list: a read that runs or failed, the older recordings, the bound.
        match &page.state {
            RecordingsRead::NotRead | RecordingsRead::Reading => {
                column = column.child(
                    sentence(text::admin::READING, &theme)
                        .py(u(6.0))
                        .debug_selector(|| "recordings-reading".to_owned()),
                );
            }
            RecordingsRead::Failed {
                sentence: reason,
                older,
            } => {
                let older = *older;
                column = column.child(
                    div().flex_none().child(
                        Notice::error(reason.clone())
                            .selector("recordings-failed")
                            .action(
                                Button::new("recordings-retry")
                                    .label(txt::READ_AGAIN)
                                    .selector("recordings-retry")
                                    .h(u(28.0))
                                    .px(u(10.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            if older {
                                                state.read_older_recordings(server_id, cx);
                                            } else {
                                                state.show_recordings(server_id, workspace_id, cx);
                                            }
                                        });
                                    })),
                            ),
                    ),
                );
            }
            RecordingsRead::Loaded if page.older => {
                column = column.child(
                    div().flex().flex_none().child(
                        Button::new("recordings-older")
                            .label(txt::LOAD_OLDER)
                            .selector("recordings-older")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.read_older_recordings(server_id, cx);
                                });
                            })),
                    ),
                );
            }
            RecordingsRead::Loaded => {}
        }
        if page.at_bound {
            column = column.child(
                sentence(txt::BOUND, &theme).debug_selector(|| "recordings-bound".to_owned()),
            );
        }
        if matches!(page.state, RecordingsRead::Loaded | RecordingsRead::Reading) {
            let reading = page.state == RecordingsRead::Reading;
            column = column.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(10.0))
                    .child(
                        sentence(txt::count(page.rows.len()), &theme)
                            .flex_1()
                            .debug_selector(|| "recordings-count".to_owned()),
                    )
                    .child(
                        Button::new("recordings-reload")
                            .label(txt::READ_AGAIN)
                            .selector("recordings-reload")
                            .disabled(reading)
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.show_recordings(server_id, workspace_id, cx);
                                });
                            })),
                    ),
            );
        }
        column.into_any_element()
    }
}
