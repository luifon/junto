//! The page of the recordings of a workspace (ADR 0784): a list that is read in pages on
//! demand. A row lists the files and the transcript of its recording for download. The
//! page shows no player, and no control that removes a recording.

use super::list::Row;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
};
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};
use junto_client_core::protocol::transcript::TranscriptState;
use junto_client_core::transfers::DownloadStage;

use super::{line, named, page_title, sentence};
use crate::assets::Icon;
use crate::shell::Shell;
use crate::shell::recorded::file_title;
use crate::state::RecordedFile;
use crate::state::admin::AdminPage;
use crate::state::admin::view::AdminView;
use crate::state::admin::view::recordings::{RecordingLine, RecordingsPage, RecordingsRead};
use crate::state::recorded::{CardState, FileKind, RecordingCard};
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

impl Shell {
    /// One recording. The name of the channel and the name of the person are text of a
    /// server: each has a line of its own that is cut at the width of the row. A row with
    /// saved files has "Download", which lists them under the text of the row.
    fn render_recording_row(
        &mut self,
        server_id: ServerId,
        index: usize,
        row: &RecordingLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let recording_id = row.id;
        let detail = |text: String| {
            div()
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .truncate()
                .child(text)
        };
        let files = row
            .files
            .as_ref()
            .map(|card| self.render_row_files(index, card, theme, cx));
        let lines = div()
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
            )))
            .children(files);
        let mut content = line(theme).items_start().child(lines);
        if row.downloads {
            content = content.child(
                named(
                    format!("recording-download-{recording_id}"),
                    Button::new(("recording-download", index))
                        .icon(Icon::Download)
                        .icon_colour(colors.cypress_ink)
                        .label(text::DOWNLOAD)
                        .tooltip(text::RECORDING_FILES)
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.toggle_recording_files(server_id, recording_id, cx);
                            });
                        })),
                )
                .mt(u(2.0)),
            );
        }
        named(format!("recording-row-{recording_id}"), content)
            .flex()
            .flex_col()
            .into_any_element()
    }

    /// The files of a recording and its transcript as text, each with a control that saves
    /// it to the download directory, and the downloads with their progress, as the card of
    /// the recording has them (ADR 0744).
    fn render_row_files(
        &mut self,
        index: usize,
        card: &RecordingCard,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        let panel = div()
            .flex()
            .flex_col()
            .gap(u(6.0))
            .mt(u(6.0))
            .debug_selector(move || format!("recording-files-{recording_id}"));
        let detail = match &card.state {
            CardState::Reading => {
                return panel
                    .child(sentence(text::RECORDING_READING, theme))
                    .into_any_element();
            }
            // The server answers "not found" also for a recording that the account may
            // read later (ADR 0604): both states can be read again.
            CardState::Gone | CardState::Failed => {
                let reason = if card.state == CardState::Gone {
                    text::RECORDING_GONE
                } else {
                    text::RECORDING_NOT_READ
                };
                return panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(8.0))
                            .child(sentence(reason, theme).flex_1())
                            .child(
                                Button::new(("recording-files-reload", index))
                                    .icon(Icon::Retry)
                                    .label(text::TRY_AGAIN)
                                    .h(u(28.0))
                                    .px(u(10.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.reload_recording(server_id, recording_id, cx);
                                        });
                                    })),
                            ),
                    )
                    .into_any_element();
            }
            CardState::Shown(detail) => detail,
        };
        let mut choices = div().flex().flex_wrap().gap(u(6.0));
        for (position, file) in detail.files.iter().enumerate() {
            let (segment_id, stem) = (file.segment_id, file.stem.clone());
            let mut button = Button::new(("recording-file", index * 1000 + position))
                .icon(match file.kind {
                    FileKind::Video => Icon::Play,
                    FileKind::Microphone => Icon::Microphone,
                    FileKind::ShareSound => Icon::Screen,
                })
                .label(format!(
                    "{} · {}",
                    file_title(file),
                    text::file_size(file.bytes)
                ))
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.save_recording_file(
                            server_id,
                            recording_id,
                            RecordedFile::Segment(segment_id),
                            &stem,
                            false,
                            cx,
                        );
                    });
                }));
            if file.problem.is_some() {
                button = button.tooltip(text::FILE_FAILED_CHECK);
            }
            choices = choices.child(named(
                format!("recording-file-{recording_id}-{position}"),
                button,
            ));
        }
        if matches!(
            detail.transcript.state,
            TranscriptState::Ready | TranscriptState::Partial
        ) {
            let stem = detail.text_stem.clone();
            choices = choices.child(named(
                format!("recording-text-{recording_id}"),
                Button::new(("recording-text", index))
                    .icon(Icon::List)
                    .label(text::RECORDING_TRANSCRIPT_TEXT)
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| {
                            state.save_recording_file(
                                server_id,
                                recording_id,
                                RecordedFile::Text,
                                &stem,
                                false,
                                cx,
                            );
                        });
                    })),
            ));
        }
        let problems: Vec<AnyElement> = detail
            .files
            .iter()
            .filter_map(|file| {
                let problem = file.problem.as_ref()?;
                Some(sentence(format!("{}: {problem}", file_title(file)), theme).into_any_element())
            })
            .collect();
        // The files that are saved, and what happened to each.
        let mut downloads: Vec<AnyElement> = Vec::new();
        for (position, file) in detail.files.iter().enumerate() {
            if let Some(progress) = &file.download
                && progress.stage != DownloadStage::Cancelled
            {
                let control = self.recorded_download(
                    card,
                    RecordedFile::Segment(file.segment_id),
                    index * 1000 + position,
                    progress,
                    theme,
                    cx,
                );
                downloads.push(Self::download_row(
                    file_title(file),
                    control,
                    position,
                    theme,
                ));
            }
        }
        if let Some(progress) = &detail.text_download
            && progress.stage != DownloadStage::Cancelled
        {
            let control = self.recorded_download(
                card,
                RecordedFile::Text,
                index * 1000 + 999,
                progress,
                theme,
                cx,
            );
            downloads.push(Self::download_row(
                text::RECORDING_TRANSCRIPT_TEXT.to_owned(),
                control,
                999,
                theme,
            ));
        }
        let refusal = card.refused.clone().map(|reason| {
            div().flex_none().child(
                Notice::error(reason)
                    .selector("recording-files-refused")
                    .action(
                        Button::new(("recording-files-dismiss", index))
                            .label(text::DISMISS)
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.dismiss_recording_refusal(server_id, recording_id, cx);
                                });
                            })),
                    ),
            )
        });
        let mut panel = panel.child(choices).children(problems).children(refusal);
        if !downloads.is_empty() {
            panel = panel.child(div().flex().flex_col().gap(u(2.0)).children(downloads));
        }
        panel.into_any_element()
    }

    /// The page asks for its list again when it is shown again.
    pub(crate) fn reset_recordings_ui(&mut self) {
        self.admin.recordings.asked = None;
    }

    /// Shows the end of the open page of the administration. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_to_end(&mut self) {
        self.admin.list.requested_end = true;
        self.admin.list.state.scroll_to_end();
        self.admin.page_scroll.scroll_to_bottom();
    }

    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_by(&mut self, distance: gpui::Pixels) {
        if self.admin.list.state.item_count() > 0 {
            self.admin.list.state.scroll_by(distance);
        } else {
            let mut offset = self.admin.page_scroll.offset();
            offset.y -= distance;
            self.admin.page_scroll.set_offset(offset);
        }
    }

    /// Moves the open page of the administration to a place between its start (0) and its
    /// end (1), as a drag of the scroll bar does. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_to(&mut self, fraction: f32) {
        if self.admin.list.state.item_count() > 0 {
            let state = &self.admin.list.state;
            let end = state.max_offset_for_scrollbar().y;
            state.set_offset_from_scrollbar(gpui::point(gpui::px(0.0), -end * fraction));
        } else {
            let end = self.admin.page_scroll.max_offset().y;
            self.admin
                .page_scroll
                .set_offset(gpui::point(gpui::px(0.0), -end * fraction));
        }
    }

    /// The height of the content of the open page of the administration, as its scroll
    /// bar takes it.
    #[cfg(feature = "test-support")]
    pub fn admin_page_extent(&self) -> gpui::Pixels {
        let state = &self.admin.list.state;
        state.max_offset_for_scrollbar().y + state.viewport_bounds().size.height
    }

    #[cfg(feature = "test-support")]
    pub fn admin_page_at_end(&self) -> bool {
        self.admin.list.state.is_scrolled_to_end() == Some(true)
    }

    #[cfg(feature = "test-support")]
    pub fn scroll_admin_page_to_start(&mut self) {
        self.admin.list.state.scroll_to(gpui::ListOffset::default());
        self.admin
            .page_scroll
            .set_offset(gpui::point(gpui::px(0.0), gpui::px(0.0)));
    }

    pub(crate) fn render_recordings_page(
        &mut self,
        view: &AdminView,
        page: &RecordingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let list = std::iter::once(Row::new("recordings-header", false).part())
            .chain(
                page.rows
                    .iter()
                    .map(|row| Row::new(format!("recording-{}", row.id), row.downloads)),
            )
            .chain(std::iter::once(Row::new("recordings-footer", true).part()))
            .collect();
        self.admin.list.take(Some(view), list, window, cx);
        let page = page.clone();
        self.render_admin_list(
            move |this, index, _, cx| {
                let theme = theme(cx);
                if index == 0 {
                    return page_title(
                        text::admin::page_name(AdminPage::Recordings),
                        txt::HELP,
                        &theme,
                    )
                    .mb(u(8.0))
                    .into_any_element();
                }
                if let Some(row) = page.rows.get(index - 1) {
                    return this.render_recording_row(server_id, index, row, &theme, cx);
                }
                this.render_recordings_footer(server_id, &page, cx)
            },
            window,
            cx,
        )
    }

    fn render_recordings_footer(
        &mut self,
        server_id: ServerId,
        page: &RecordingsPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let workspace_id = page.workspace_id;
        let mut column = div().flex().flex_col().gap(u(8.0)).mt(u(8.0));
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
