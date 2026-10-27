//! A finished recording in the chat of its voice channel (ADR 0744, view G of the design):
//! the card with its files and the start of its transcript, the menu of its files, the
//! question before its removal, and the transcript in the side panel.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ListAlignment,
    ListOffset, ListState, ParentElement, RenderImage, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, list,
    prelude::FluentBuilder, px,
};
use gpui_base::{Scrollbar, ScrollbarMode};
use junto_client_core::model::OpenRecording;
use junto_client_core::protocol::ids::{AccountId, RecordingId, ServerId};
use junto_client_core::protocol::transcript::TranscriptState;
use junto_client_core::transfers::{DownloadProgress, DownloadStage};

use super::notices::side_panel;
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::RecordedFile;
use crate::state::feed::RecordingRow;
use crate::state::recorded::{
    CardDetail, CardState, FileKind, FileRow, LineFrom, LineRow, RecordingCard, SHOWN_LINES,
    TranscriptRequest, TranscriptShown, When, line_row,
};
use crate::state::transcript_work::TranscriptChange;
use crate::state::view::{TranscriptPanel, TranscriptPanelState};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Avatar, Button, ButtonKind, Chip, Dialog, Menu, MenuItem, Notice, SideHeader, StatePanel,
    StyleExt, dialog_footer, icon, menu_label,
};

/// The list of the lines of the transcript in the side panel. The rows are the count of
/// the lines, the lines that were read, and the rows after them.
pub(crate) struct TranscriptList {
    pub(crate) list: ListState,
    /// The recording whose lines the list holds, as it was drawn last.
    held: Option<(RecordingId, Arc<OpenRecording>)>,
    /// The rows after the lines.
    tail: usize,
    /// The line that the list went to last, for a result of a search, with the number
    /// of that opening of the panel.
    went: Option<(u32, u64)>,
}

impl TranscriptList {
    pub(crate) fn new() -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Top, px(400.0)),
            held: None,
            tail: 0,
            went: None,
        }
    }

    /// Takes the lines of a recording. The rows that did not change keep their measured
    /// heights, and the list keeps its position.
    fn take(&mut self, recording_id: RecordingId, open: &Arc<OpenRecording>, tail: usize) {
        let new = &open.lines.lines;
        let old = match &self.held {
            Some((held, old)) if *held == recording_id => old.clone(),
            _ => {
                self.list.reset(1 + new.len() + tail);
                self.held = Some((recording_id, open.clone()));
                self.tail = tail;
                self.went = None;
                return;
            }
        };
        if !Arc::ptr_eq(&old, open) {
            let old = &old.lines.lines;
            let same = |one: &Arc<_>, other: &Arc<_>| Arc::ptr_eq(one, other);
            let prefix = old
                .iter()
                .zip(new.iter())
                .take_while(|(one, other)| same(one, other))
                .count();
            let suffix = old[prefix..]
                .iter()
                .rev()
                .zip(new[prefix..].iter().rev())
                .take_while(|(one, other)| same(one, other))
                .count();
            let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
            if prefix != old_end || prefix != new_end {
                self.list.splice(1 + prefix..1 + old_end, new_end - prefix);
            }
            self.held = Some((recording_id, open.clone()));
        }
        if self.tail != tail {
            let start = 1 + new.len();
            self.list.splice(start..start + self.tail, tail);
            self.tail = tail;
        }
    }

    /// Moves the list to one of its rows. A test uses it.
    #[cfg(feature = "test-support")]
    pub fn show_row(&self, row: usize) {
        self.list.scroll_to(ListOffset {
            item_ix: row,
            offset_in_item: px(0.0),
        });
    }

    /// Goes to a line once, when the read lines hold it.
    fn go_to(&mut self, line: u32, jump: u64) {
        if self.went == Some((line, jump)) {
            return;
        }
        let Some((_, open)) = &self.held else {
            return;
        };
        let Ok(index) = open
            .lines
            .lines
            .binary_search_by_key(&line, |held| held.line)
        else {
            return;
        };
        self.went = Some((line, jump));
        self.list.scroll_to(ListOffset {
            item_ix: 1 + index,
            offset_in_item: px(0.0),
        });
    }
}

/// The file of a recording as its menu and its row name it.
pub(super) fn file_title(file: &FileRow) -> String {
    match (file.kind, &file.person) {
        (FileKind::Video, _) => text::RECORDING_VIDEO.to_owned(),
        (FileKind::Microphone, person) => text::recording_microphone(person.as_deref()),
        (FileKind::ShareSound, person) => text::recording_share_sound(person.as_deref()),
    }
}

impl Shell {
    /// Saves a file of a recording, or opens the video when `open` is set.
    fn save_recorded(
        &mut self,
        card: &RecordingCard,
        file: RecordedFile,
        stem: String,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        self.state.update(cx, |state, cx| {
            state.save_recording_file(server_id, recording_id, file, &stem, open, cx);
        });
    }

    /// Opens the list of the files of a recording, at its card.
    pub fn open_recording_files(
        &mut self,
        recording_id: RecordingId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A click on the button of an open list closes the list: the press closed it, and
        // the click must not open it again.
        if std::mem::take(&mut self.recording_files_closed_by_press) {
            return;
        }
        self.open_layer(Layer::RecordingFiles(recording_id), window, cx);
    }

    /// Asks whether a recording is to be removed.
    pub fn ask_to_remove_recording(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(
            Layer::ConfirmRemoveRecording(server_id, recording_id),
            window,
            cx,
        );
    }

    /// Moves the list of the transcript panel to one of its rows. A test uses it.
    #[cfg(feature = "test-support")]
    pub fn show_transcript_row(&mut self, row: usize, cx: &mut Context<Self>) {
        self.transcript_list.show_row(row);
        cx.notify();
    }

    /// Shows the transcript of a recording of the shown voice channel in the side panel.
    pub fn open_transcript(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        line: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        let view = self.state.read(cx).view();
        let Some((workspace_id, channel_id)) = view
            .workspace()
            .and_then(|workspace| Some((workspace.id, workspace.channel.as_ref()?.id)))
        else {
            return;
        };
        self.members_open = false;
        self.state.update(cx, |state, cx| {
            state.open_transcript(server_id, workspace_id, channel_id, recording_id, line, cx);
        });
        cx.notify();
    }

    /// The state of the download of one file, as words and a control.
    pub(super) fn recorded_download(
        &mut self,
        card: &RecordingCard,
        file: RecordedFile,
        index: usize,
        progress: &DownloadProgress,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        let small = |sentence: SharedString, colour| {
            div()
                .flex_none()
                .text_size(u(12.5))
                .ink(colour)
                .child(sentence)
        };
        let forget = |id: &'static str, label: &'static str, cx: &mut Context<Self>| {
            Button::new((id, index))
                .kind(ButtonKind::Ghost)
                .icon(Icon::X)
                .tooltip(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.forget_recording_download(server_id, recording_id, file, cx);
                    });
                }))
        };
        let row = div().flex().flex_none().items_center().gap(u(4.0));
        match &progress.stage {
            DownloadStage::Receiving => row
                .child(small(
                    if progress.interrupted {
                        text::DOWNLOAD_INTERRUPTED.into()
                    } else if progress.bytes == 0 {
                        text::received(progress.received).into()
                    } else {
                        text::transfer_progress(progress.received, progress.bytes).into()
                    },
                    colors.ink_2,
                ))
                .child(forget("cancel-recorded", text::CANCEL_DOWNLOAD, cx))
                .into_any_element(),
            DownloadStage::Checking => row
                .child(small(text::DOWNLOAD_CHECKING.into(), colors.ink_2))
                .into_any_element(),
            DownloadStage::Saved(path) => {
                let path = path.clone();
                row.child(small(text::DOWNLOAD_SAVED.into(), colors.cypress_ink))
                    .child(
                        div()
                            .flex_none()
                            .debug_selector(move || match file {
                                RecordedFile::Segment(segment_id) => {
                                    format!("show-recorded-{segment_id}")
                                }
                                RecordedFile::Text => format!("show-recorded-text-{recording_id}"),
                            })
                            .child(
                                Button::new(("show-recorded", index))
                                    .kind(ButtonKind::Ghost)
                                    .icon(Icon::FolderOpen)
                                    .tooltip(text::SHOW_IN_FOLDER)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.show_in_folder(path.clone(), cx)
                                        });
                                    })),
                            ),
                    )
                    .child(forget("forget-recorded", text::DISMISS, cx))
                    .into_any_element()
            }
            DownloadStage::Failed(failure) => {
                // The system did not let Junto write into the download directory (ADR 0708).
                let refused = *failure
                    == junto_client_core::transfers::TransferFailure::File(
                        std::io::ErrorKind::PermissionDenied,
                    );
                let system = self.state.read(cx).permission_system();
                let sentence = if refused {
                    crate::text::permissions::download_refused(system)
                } else {
                    text::transfer_failure(*failure)
                };
                let permission = if refused {
                    self.permission_button(
                        "recorded-download-permission-action",
                        crate::permissions::Permission::Downloads,
                        cx,
                    )
                } else {
                    None
                };
                row.child(icon(Icon::Warning, 14.0, colors.cinnabar))
                    .child(small(sentence.into(), colors.cinnabar_ink))
                    .children(permission)
                    .child(forget("forget-recorded", text::DISMISS, cx))
                    .into_any_element()
            }
            DownloadStage::Cancelled => div().into_any_element(),
        }
    }

    /// The list of the files of a recording: the video, each audio file, and the
    /// transcript as text.
    fn render_recording_files(
        &mut self,
        card: &RecordingCard,
        detail: &CardDetail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut items: Vec<AnyElement> =
            vec![menu_label(text::DOWNLOAD, false, cx).into_any_element()];
        for (index, file) in detail.files.iter().enumerate() {
            let (held, stem, segment_id) = (card.clone(), file.stem.clone(), file.segment_id);
            let mut item = MenuItem::new(("recorded-file", index), file_title(file))
                .icon(match file.kind {
                    FileKind::Video => Icon::Play,
                    FileKind::Microphone => Icon::Microphone,
                    FileKind::ShareSound => Icon::Screen,
                })
                .trailing(text::file_size(file.bytes))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.save_recorded(
                        &held,
                        RecordedFile::Segment(segment_id),
                        stem.clone(),
                        false,
                        cx,
                    );
                    this.close_layer(window, cx);
                }));
            if file.problem.is_some() {
                item = item
                    .detail(text::FILE_FAILED_CHECK)
                    .detail_icon(Icon::Warning);
            }
            items.push(
                div()
                    .flex_none()
                    .debug_selector(move || format!("recorded-file-{index}"))
                    .child(item)
                    .into_any_element(),
            );
        }
        if matches!(
            detail.transcript.state,
            TranscriptState::Ready | TranscriptState::Partial
        ) {
            let (held, stem) = (card.clone(), detail.text_stem.clone());
            items.push(
                MenuItem::new("recorded-text", text::RECORDING_TRANSCRIPT_TEXT)
                    .icon(Icon::Document)
                    .selector("recorded-text")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.save_recorded(&held, RecordedFile::Text, stem.clone(), false, cx);
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .left_0()
            .top(u(32.0))
            .child(deferred(
                anchored().snap_to_window().child(
                    Menu::new("recording-files", text::RECORDING_FILES, self.layer_focus())
                        .width(340.0)
                        .selector("recording-files")
                        .on_close(Self::handler(cx, |this, window, cx| {
                            this.recording_files_closed_by_press =
                                !window.last_input_was_keyboard();
                            this.close_layer(window, cx);
                        }))
                        .children(items)
                        .child(div().h(u(4.0))),
                ),
            ))
            .into_any_element()
    }

    /// One line of a transcript (view G of the design): its time, its speaker with the
    /// source of the sound, and its text.
    fn transcript_line(
        row: &LineRow,
        picture: Option<Arc<RenderImage>>,
        wide: bool,
        marked: bool,
        theme: &Theme,
    ) -> AnyElement {
        let corrected = row.corrected.then(|| {
            div()
                .flex_none()
                .text_size(u(12.5))
                .font_weight(FontWeight::NORMAL)
                .ink(theme.colors.ink_3)
                .child(text::LINE_CORRECTED)
                .into_any_element()
        });
        let spoken = div()
            .flex_1()
            .min_w_0()
            .text_size(u(14.5))
            .line_height(u(22.0))
            .ink(theme.colors.ink)
            .child(SharedString::from(row.text.clone()))
            .into_any_element();
        Self::line_frame(row, picture, wide, marked, theme, corrected, spoken, None)
    }

    /// A line of a transcript with the parts that the card or the side panel give it: the
    /// mark of a correction beside the speaker, the text, and a control at the right.
    /// `picture` is the picture of the person of a microphone line (ADR 0783).
    #[allow(clippy::too_many_arguments)]
    fn line_frame(
        row: &LineRow,
        picture: Option<Arc<RenderImage>>,
        wide: bool,
        marked: bool,
        theme: &Theme,
        corrected: Option<AnyElement>,
        spoken: AnyElement,
        trailing: Option<AnyElement>,
    ) -> AnyElement {
        let colors = theme.colors;
        let line = row.line;
        let source: SharedString = match &row.source {
            LineFrom::Microphone => text::LINE_MICROPHONE.into(),
            LineFrom::Shared { of } => text::shared_audio_of(of).into(),
        };
        let mark: AnyElement = match row.source {
            LineFrom::Microphone => Avatar::new(row.account_id.as_uuid().as_u128(), 22.0)
                .radius(2.0)
                .image(picture)
                .into_any_element(),
            LineFrom::Shared { .. } => div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(u(22.0))
                .rounded(u(2.0))
                .border_1()
                .border_color(Hsla::from(colors.rule))
                .bg(Hsla::from(colors.plaster))
                .child(icon(Icon::Screen, 13.0, colors.bronze))
                .into_any_element(),
        };
        let who = div()
            .flex_none()
            .when(wide, |who| who.w(u(236.0)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(7.0))
                    .text_size(u(14.0))
                    .line_height(u(22.0))
                    .font_weight(FontWeight::BOLD)
                    .ink(colors.ink)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(row.name.clone())),
                    )
                    .when_some(row.was.clone(), |name, was| {
                        name.child(Chip::new(text::was_speaker(&was)))
                    })
                    .children(corrected),
            )
            .child(
                div()
                    .text_size(u(12.5))
                    .line_height(u(16.0))
                    .ink(colors.ink_2)
                    .child(source),
            );
        let time = div()
            .flex_none()
            .w(u(68.0))
            .whitespace_nowrap()
            .font_family(theme.fonts.mono.clone())
            .font_weight(FontWeight::MEDIUM)
            .text_size(u(12.5))
            .line_height(u(22.0))
            .ink(colors.cypress_ink)
            .child(SharedString::from(row.time.clone()));
        let frame = div()
            .id(("transcript-line", line as usize))
            .role(Role::ListItem)
            .aria_label(format!("{}, {}: {}", row.time, row.name, row.text))
            .debug_selector(move || format!("transcript-line:{line}"))
            .flex()
            .flex_none()
            .w_full()
            .items_start()
            .gap(u(10.0))
            .min_h(u(34.0))
            .py(u(5.0))
            .pl(u(10.0))
            .pr(u(8.0))
            .border_t_1()
            .border_color(Hsla::from(colors.rule_soft))
            .when(marked, |frame| {
                frame.relative().bg(Hsla::from(colors.target_wash)).child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size(px(1.0))
                        .debug_selector(move || format!("marked-line:{line}")),
                )
            });
        if wide {
            frame
                .child(time)
                .child(mark)
                .child(who)
                .child(spoken)
                .children(trailing)
                .into_any_element()
        } else {
            // In the side panel the text stands under the speaker.
            frame
                .child(time)
                .child(mark)
                .child(div().flex_1().min_w_0().child(who).child(spoken))
                .children(trailing)
                .into_any_element()
        }
    }

    /// The transcript of a card: its header with the count and the language, the first
    /// lines, and the control that shows all of them.
    fn card_transcript(
        &mut self,
        card: &RecordingCard,
        transcript: &TranscriptShown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        let state: SharedString =
            text::transcript_state(transcript.state, transcript.failure).into();
        let ready = matches!(
            transcript.state,
            TranscriptState::Ready | TranscriptState::Partial
        );
        let mut parts = vec![
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(8.0))
                .h(u(36.0))
                .pl(u(10.0))
                .pr(u(8.0))
                .bg(Hsla::from(colors.plaster))
                .border_t_1()
                .border_color(Hsla::from(colors.rule))
                .text_size(u(13.5))
                .ink(colors.ink_2)
                .child(icon(Icon::ChevronDown, 14.0, colors.ink_2))
                .child(
                    div()
                        .flex_none()
                        .text_size(u(14.0))
                        .font_weight(FontWeight::BOLD)
                        .ink(colors.ink)
                        .child(text::TRANSCRIPT),
                )
                .child(div().flex_none().child(if ready {
                    SharedString::from(text::lines(transcript.total))
                } else {
                    state
                }))
                .when(ready && !transcript.language.is_empty(), |header| {
                    header.child(Chip::new(text::transcript_language(&transcript.language)))
                })
                .children(self.request_transcript_button(card, transcript, cx))
                .into_any_element(),
        ];
        if !ready {
            return parts;
        }
        let faces = self.faces(server_id);
        for row in transcript.lines.iter().take(SHOWN_LINES) {
            let picture = faces.image(row.account_id);
            parts.push(Self::transcript_line(row, picture, true, false, theme));
        }
        let shown = transcript.lines.len().min(SHOWN_LINES);
        parts.push(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(8.0))
                .h(u(32.0))
                .px(u(10.0))
                .border_t_1()
                .border_color(Hsla::from(colors.rule_soft))
                .text_size(u(13.0))
                .ink(colors.ink_2)
                .child(SharedString::from(if transcript.refresh_failed {
                    text::TRANSCRIPT_REFRESH_FAILED.to_owned()
                } else if transcript.failed {
                    text::TRANSCRIPT_LINES_FAILED.to_owned()
                } else {
                    text::lines_shown(shown, transcript.total)
                }))
                .child({
                    let ring =
                        crate::ui::focus_ring(colors.terracotta_deep, theme.metrics.focus_ring);
                    div()
                        .id(("show-transcript", recording_id.as_uuid().as_u128() as usize))
                        .role(Role::Button)
                        .aria_label(text::SHOW_ALL)
                        .debug_selector(move || format!("show-transcript:{recording_id}"))
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .ink(colors.cypress_ink)
                        .cursor_pointer()
                        .focusable()
                        .tab_stop(true)
                        .focus_visible(move |style| style.shadow(ring))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_transcript(server_id, recording_id, None, cx);
                        }))
                        .child(text::SHOW_ALL)
                })
                .into_any_element(),
        );
        parts
    }

    /// The control that asks for the transcript again, at the end of the title line of the
    /// transcript. The line keeps its height when the control comes and goes.
    fn request_transcript_button(
        &mut self,
        card: &RecordingCard,
        transcript: &TranscriptShown,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        let label = if transcript.state == TranscriptState::Failed {
            text::transcripts::REQUEST_AGAIN
        } else {
            text::transcripts::REQUEST
        };
        let button = Button::new("request-transcript")
            .icon(Icon::Retry)
            .selector("request-transcript")
            .h(u(26.0))
            .px(u(8.0));
        let button = match card.request {
            TranscriptRequest::NotOffered => return None,
            TranscriptRequest::NotAllowed => button
                .label(label)
                .tooltip(text::transcripts::REQUEST_NEEDS)
                .disabled(true),
            TranscriptRequest::Allowed { sending } => button
                .label(if sending {
                    text::transcripts::REQUESTING
                } else {
                    label
                })
                .tooltip(text::transcripts::REQUEST_HELP)
                .disabled(sending)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.request_transcript(server_id, recording_id, cx);
                    });
                })),
        };
        Some(
            div()
                .flex()
                .flex_1()
                .justify_end()
                .child(button)
                .into_any_element(),
        )
    }

    /// Opens the parts of a recording with their time ranges, at its card.
    pub fn open_recording_parts(
        &mut self,
        recording_id: RecordingId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if std::mem::take(&mut self.recording_parts_closed_by_press) {
            return;
        }
        self.open_layer(Layer::RecordingParts(recording_id), window, cx);
    }

    /// The parts of a recording: each file of the composite and of each track, its state,
    /// and the time range that the recorder reported for it (ADR 0601). It floats over the
    /// chat, so the rows of the chat do not move.
    fn render_recording_parts(
        &mut self,
        detail: &CardDetail,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let mut items: Vec<AnyElement> =
            vec![menu_label(text::transcripts::PARTS_HELP, false, cx).into_any_element()];
        for (index, part) in detail.parts.iter().enumerate() {
            items.push(
                div()
                    .debug_selector(move || format!("recording-part:{index}"))
                    .flex()
                    .items_start()
                    .gap(u(6.0))
                    .px(u(12.0))
                    .py(u(5.0))
                    .text_size(u(13.0))
                    .line_height(u(18.0))
                    .when(part.failed, |row| {
                        row.child(div().mt(u(2.0)).child(icon(
                            Icon::Warning,
                            13.0,
                            colors.cinnabar,
                        )))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .ink(colors.ink)
                                    .child(SharedString::from(part.title.clone())),
                            )
                            .child(
                                div()
                                    .ink(if part.failed {
                                        colors.cinnabar_ink
                                    } else {
                                        colors.ink_2
                                    })
                                    .child(SharedString::from(part.state.clone())),
                            )
                            .when_some(part.span.clone(), |column, span| {
                                column.child(
                                    div()
                                        .debug_selector(move || {
                                            format!("recording-part-span:{index}")
                                        })
                                        .font_family(theme.fonts.mono.clone())
                                        .ink(colors.ink_2)
                                        .child(SharedString::from(span)),
                                )
                            }),
                    )
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .left_0()
            .top(u(32.0))
            .child(deferred(
                anchored().snap_to_window().child(
                    Menu::new(
                        "recording-parts-list",
                        text::transcripts::PARTS_HELP,
                        self.layer_focus(),
                    )
                    .width(380.0)
                    .selector("recording-parts-list")
                    .on_close(Self::handler(cx, |this, window, cx| {
                        this.recording_parts_closed_by_press = !window.last_input_was_keyboard();
                        this.close_layer(window, cx);
                    }))
                    .children(items)
                    .child(div().h(u(6.0))),
                ),
            ))
            .into_any_element()
    }

    /// The card of a finished recording, in place of the system line that posted it.
    pub(crate) fn render_recording_card(
        &mut self,
        index: usize,
        row: &RecordingRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let card = &row.card;
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        let note = |sentence: SharedString, selector: &'static str| {
            div()
                .id(selector)
                .debug_selector(move || selector.to_owned())
                .px(u(10.0))
                .py(u(9.0))
                .text_size(u(13.5))
                .line_height(u(20.0))
                .ink(colors.ink_2)
                .child(sentence)
                .into_any_element()
        };
        let (title, tag): (SharedString, Option<SharedString>) = match &card.state {
            CardState::Shown(detail) => (
                text::recording_of(&detail.channel).into(),
                Some(
                    text::transcript_state(detail.transcript.state, detail.transcript.failure)
                        .into(),
                ),
            ),
            _ => (text::RECORDING.into(), None),
        };
        let body: Vec<AnyElement> =
            match &card.state {
                CardState::Reading => {
                    vec![note(text::RECORDING_READING.into(), "recording-reading")]
                }
                // The server answers "not found" also for a recording that the account
                // may read later, such as one of a call that still runs in a room that
                // hides who is inside (ADR 0604): both states can be read again.
                CardState::Gone | CardState::Failed => {
                    let gone = card.state == CardState::Gone;
                    vec![
                        div()
                            .flex()
                            .items_center()
                            .gap(u(8.0))
                            .px(u(10.0))
                            .py(u(8.0))
                            .debug_selector(move || {
                                if gone {
                                    "recording-gone".to_owned()
                                } else {
                                    "recording-not-read".to_owned()
                                }
                            })
                            .child(div().flex_1().text_size(u(13.5)).ink(colors.ink_2).child(
                                if gone {
                                    text::RECORDING_GONE
                                } else {
                                    text::RECORDING_NOT_READ
                                },
                            ))
                            .child(
                                Button::new(("reload-recording", index))
                                    .icon(Icon::Retry)
                                    .label(text::TRY_AGAIN)
                                    .selector("reload-recording")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.reload_recording(server_id, recording_id, cx);
                                        });
                                    })),
                            )
                            .into_any_element(),
                    ]
                }
                CardState::Shown(detail) => self.card_body(index, card, detail, &theme, cx),
            };
        div()
            .id(("recording", index))
            .role(Role::ListItem)
            .aria_label(title.clone())
            .debug_selector(move || format!("recording:{recording_id}"))
            .flex()
            .w_full()
            .gap(u(10.0))
            .pt(u(4.0))
            .pb(u(6.0))
            .pl(u(16.0))
            .pr(u(18.0))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(u(34.0))
                    .mt(u(2.0))
                    .rounded(u(theme.metrics.radius))
                    .border_1()
                    .border_color(Hsla::from(colors.rule))
                    .bg(Hsla::from(colors.travertine_hi))
                    .child(icon(Icon::Record, 18.0, colors.cinnabar)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(u(8.0))
                            .line_height(u(19.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .text_size(u(14.5))
                                    .ink(colors.ink)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(u(12.5))
                                    .ink(colors.ink_3)
                                    .child(SharedString::from(row.time.clone())),
                            )
                            .when_some(tag, |header, tag| {
                                header.child(
                                    div()
                                        .flex_none()
                                        .text_size(u(12.5))
                                        .ink(colors.ink_3)
                                        .child(tag),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .mt(u(5.0))
                            .w_full()
                            .max_w(u(880.0))
                            .overflow_hidden()
                            .rounded(u(theme.metrics.radius))
                            .border_1()
                            .border_color(Hsla::from(colors.rule))
                            .bg(Hsla::from(colors.travertine_hi))
                            .children(body),
                    ),
            )
            .into_any_element()
    }

    fn card_body(
        &mut self,
        index: usize,
        card: &RecordingCard,
        detail: &CardDetail,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let (server_id, recording_id) = (card.server_id, card.recording_id);
        let files_open =
            matches!(&self.layer, Layer::RecordingFiles(open) if *open == recording_id);
        let menu = files_open.then(|| self.render_recording_files(card, detail, cx));
        let line = |sentence: String| {
            div()
                .text_size(u(13.5))
                .line_height(u(20.0))
                .ink(colors.ink_2)
                .child(SharedString::from(sentence))
        };
        let job: Option<AnyElement> = text::recording_job(detail.job).map(|(sentence, failed)| {
            div()
                .id("recording-job")
                .debug_selector(|| "recording-job".to_owned())
                .flex()
                .items_start()
                .gap(u(6.0))
                .mt(u(2.0))
                .text_size(u(13.5))
                .line_height(u(20.0))
                .ink(if failed {
                    colors.cinnabar_ink
                } else {
                    colors.ink_2
                })
                .when(failed, |job| {
                    job.child(
                        div()
                            .mt(u(3.0))
                            .child(icon(Icon::Warning, 14.0, colors.cinnabar)),
                    )
                })
                .child(SharedString::from(sentence))
                .into_any_element()
        });
        let mut actions: Vec<AnyElement> = Vec::new();
        if let Some(video) = detail.video {
            let stem = detail
                .files
                .iter()
                .find(|file| file.segment_id == video)
                .map(|file| file.stem.clone())
                .unwrap_or_default();
            let held = card.clone();
            actions.push(
                Button::new(("open-recording", index))
                    .icon(Icon::Open)
                    .icon_colour(colors.cypress_ink)
                    .label(text::OPEN)
                    .tooltip(text::OPEN_RECORDING)
                    .selector("open-recording")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_recorded(
                            &held,
                            RecordedFile::Segment(video),
                            stem.clone(),
                            true,
                            cx,
                        );
                    }))
                    .into_any_element(),
            );
        }
        if !detail.parts.is_empty() {
            let parts_open =
                matches!(&self.layer, Layer::RecordingParts(open) if *open == recording_id);
            let list = parts_open.then(|| self.render_recording_parts(detail, cx));
            actions.push(
                div()
                    .relative()
                    .flex_none()
                    .child(
                        Button::new(("recording-parts", index))
                            .icon(Icon::List)
                            .label(text::transcripts::parts(detail.parts.len()))
                            .tooltip(text::transcripts::PARTS_HELP)
                            .selector("recording-parts")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_recording_parts(recording_id, window, cx);
                            })),
                    )
                    .children(list)
                    .into_any_element(),
            );
        }
        if !detail.files.is_empty() {
            actions.push(
                div()
                    .relative()
                    .flex_none()
                    .child(
                        Button::new(("download-recording", index))
                            .icon(Icon::Download)
                            .icon_colour(colors.cypress_ink)
                            .label(text::DOWNLOAD)
                            .tooltip(text::RECORDING_FILES)
                            .selector("download-recording")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_recording_files(recording_id, window, cx);
                            })),
                    )
                    .children(menu)
                    .into_any_element(),
            );
        }
        actions.push(div().flex_1().into_any_element());
        // The removal: offered to a person whose roles hold the permission, and stated
        // as not allowed to another person.
        actions.push(if card.may_remove {
            Button::new(("remove-recording", index))
                .icon(Icon::Trash)
                .label(if card.removing {
                    text::RECORDING_REMOVING
                } else {
                    text::REMOVE
                })
                .tooltip(text::REMOVE_RECORDING)
                .disabled(card.removing)
                .selector("remove-recording")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.ask_to_remove_recording(server_id, recording_id, window, cx);
                }))
                .into_any_element()
        } else {
            Button::new(("remove-recording", index))
                .icon(Icon::Trash)
                .label(text::REMOVE)
                .tooltip(text::RECORDING_REMOVE_NEEDS_PERMISSION)
                .disabled(true)
                .selector("remove-recording")
                .into_any_element()
        });
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
        let length = detail.length.map(text::media_length);
        let top = div()
            .flex()
            .gap(u(14.0))
            .p(u(8.0))
            .child(
                // The place of the picture of the video: the client has no player, and
                // "Open" plays the file with the player of the system.
                div()
                    .relative()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(u(160.0))
                    .h(u(100.0))
                    .rounded(u(theme.metrics.radius_small))
                    .bg(Hsla::from(colors.basalt))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(u(44.0))
                            .rounded_full()
                            .border_1()
                            .border_color(Hsla::from(colors.basalt_rule))
                            .bg(Hsla::from(colors.basalt_deep))
                            .child(icon(Icon::Play, 20.0, colors.bone)),
                    )
                    .when_some(length, |thumb, length| {
                        thumb.child(
                            div()
                                .absolute()
                                .right(u(5.0))
                                .bottom(u(5.0))
                                .flex()
                                .items_center()
                                .h(u(21.0))
                                .px(u(6.0))
                                .rounded(u(2.0))
                                .bg(Hsla::from(colors.basalt_deep))
                                .font_family(theme.fonts.mono.clone())
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(u(12.5))
                                .ink(colors.bone)
                                .child(SharedString::from(length)),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pt(u(1.0))
                    .child(
                        div()
                            .text_size(u(15.0))
                            .line_height(u(21.0))
                            .font_weight(FontWeight::BOLD)
                            .ink(colors.ink)
                            .child(SharedString::from(text::recording_title(
                                &detail.channel,
                                &detail.day,
                            ))),
                    )
                    .child(line(text::recording_span(detail)))
                    .child(line(text::recording_files(detail)))
                    .children(job)
                    .children(detail.files.iter().filter_map(|file| {
                        let problem = file.problem.as_ref()?;
                        Some(line(format!("{}: {problem}", file_title(file))))
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(6.0))
                            .mt(u(10.0))
                            .children(actions),
                    ),
            );
        let mut body = vec![top.into_any_element()];
        if !downloads.is_empty() {
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .px(u(10.0))
                    .pb(u(8.0))
                    .gap(u(2.0))
                    .children(downloads)
                    .into_any_element(),
            );
        }
        body.push(
            crate::ui::notice_slot(("recording-refusal-slot", index))
                .children(card.refused.clone().map(|reason| {
                    div()
                        .px(u(8.0))
                        .pb(u(8.0))
                        .child(
                            Notice::error(reason).selector("recording-refused").action(
                                Button::new(("dismiss-recording-refusal", index))
                                    .label(text::DISMISS)
                                    .selector("dismiss-recording-refusal")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.dismiss_recording_refusal(
                                                server_id,
                                                recording_id,
                                                cx,
                                            );
                                        });
                                    })),
                            ),
                        )
                        .into_any_element()
                }))
                .into_any_element(),
        );
        body.extend(self.card_transcript(card, &detail.transcript, theme, cx));
        body
    }

    pub(super) fn download_row(
        title: String,
        control: AnyElement,
        position: usize,
        theme: &Theme,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(u(8.0))
            .debug_selector(move || format!("recorded-download-{position}"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(u(13.0))
                    .ink(theme.colors.ink)
                    .child(SharedString::from(title)),
            )
            .child(control)
            .into_any_element()
    }

    pub(crate) fn render_confirm_remove_recording(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        Dialog::new(text::REMOVE_RECORDING_TITLE, self.layer_focus())
            .selector("confirm-remove-recording")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            .child(
                div()
                    .typeface(theme.types.notice, &theme)
                    .ink(theme.colors.ink_2)
                    .child(text::REMOVE_RECORDING_CONFIRM),
            )
            .child(dialog_footer([
                Button::new("cancel")
                    .label(text::CANCEL)
                    .selector("cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new("confirm")
                    .kind(ButtonKind::Primary)
                    .label(text::REMOVE)
                    .selector("confirm")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.update(cx, |state, cx| {
                            state.remove_recording(server_id, recording_id, cx);
                        });
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
            ]))
            .into_any_element()
    }

    /// One line of the transcript in the side panel, with the control that corrects it
    /// and its original text, or with the form that corrects it.
    #[allow(clippy::too_many_arguments)]
    fn panel_line(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        row: &LineRow,
        transcript: &TranscriptShown,
        marked: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let corrected = row
            .corrected
            .then(|| self.corrected_mark(recording_id, row.line, theme, cx));
        let (body, control) = self.line_body(server_id, recording_id, row, transcript, theme, cx);
        let key = (recording_id, TranscriptChange::Line(row.line));
        let open = self.transcript_edit.open.map(|(_, open)| open);
        let picture = self.faces(server_id).image(row.account_id);
        let line = Self::line_frame(row, picture, false, marked, theme, corrected, body, control);
        self.transcript_edit
            .space
            .row(open, &[key], line)
            .into_any_element()
    }

    /// The head of the transcript list: the count of the lines, the language, and the
    /// control that shows the details, with the details under it.
    fn transcript_head(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        transcript: &TranscriptShown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let details_open = self.transcript_edit.details == Some(recording_id);
        // The count and the language wrap in the width of the panel, and the control that
        // shows the details stands under them: no part of the head is wider than the panel.
        let head = div()
            .flex()
            .flex_col()
            .gap(u(6.0))
            .px(u(14.0))
            .py(u(8.0))
            .text_size(u(13.0))
            .ink(colors.ink_2)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(u(8.0))
                    .child(SharedString::from(text::lines(transcript.total)))
                    .when(!transcript.language.is_empty(), |line| {
                        line.child(Chip::new(text::transcript_language(&transcript.language)))
                    }),
            )
            .child(
                div().flex().child(
                    Button::new("transcript-details")
                        .icon(if details_open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        })
                        .label(text::transcripts::DETAILS)
                        .tooltip(text::transcripts::DETAILS_HELP)
                        .selector("transcript-details")
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_transcript_details(recording_id, cx);
                        })),
                ),
            );
        let details = details_open
            .then(|| self.transcript_details(server_id, recording_id, transcript, theme, cx));
        div()
            .flex()
            .flex_col()
            .child(head)
            .children(details)
            .into_any_element()
    }

    /// The lines of a transcript in the side panel, as a list that draws the rows in
    /// view: a transcript has up to 20,000 read lines.
    #[allow(clippy::too_many_arguments)]
    fn transcript_lines(
        &mut self,
        server_id: ServerId,
        panel: &TranscriptPanel,
        transcript: &TranscriptShown,
        open: &Arc<OpenRecording>,
        names: &Arc<HashMap<AccountId, String>>,
        when: When,
        look: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = look.colors;
        let recording_id = panel.recording_id;
        // The rows: the count of the lines, the lines, the failure of a read, and the
        // control for the next page.
        let lines = open.lines.lines.len();
        let (failed, more) = (transcript.failed, transcript.more || transcript.failed);
        let refresh_failed = transcript.refresh_failed;
        let tail = 1 + usize::from(more);
        self.transcript_list.take(recording_id, open, tail);
        match panel.line {
            Some(line) => self.transcript_list.go_to(line, panel.jump),
            None => self.transcript_list.went = None,
        }
        let state = self.transcript_list.list.clone();
        let (reading, earlier) = (transcript.reading, transcript.more_before);
        let (open, names, marked) = (open.clone(), names.clone(), panel.line);
        let shown = Arc::new(transcript.clone());
        let render =
            cx.processor(move |this: &mut Self, index: usize, _, cx| {
                let theme = theme(cx);
                if index == 0 {
                    return this.transcript_head(server_id, recording_id, &shown, &theme, cx);
                }
                if let Some(line) = open.lines.lines.get(index - 1) {
                    let row = line_row(&open, line, &names, when);
                    return this.panel_line(
                        server_id,
                        recording_id,
                        &row,
                        &shown,
                        marked == Some(row.line),
                        &theme,
                        cx,
                    );
                }
                if index == lines + 1 {
                    return div()
                        .px(u(14.0))
                        .pt(u(8.0))
                        .child(crate::ui::notice_slot("transcript-error-slot").children(
                            failed.then(|| {
                                Notice::error(if refresh_failed {
                                    text::TRANSCRIPT_REFRESH_FAILED
                                } else {
                                    text::TRANSCRIPT_LINES_FAILED
                                })
                                .selector("transcript-lines-failed")
                            }),
                        ))
                        .into_any_element();
                }
                div()
                    .flex()
                    .w_full()
                    .px(u(14.0))
                    .pt(u(8.0))
                    .pb(u(14.0))
                    .child(
                        Button::new("more-lines")
                            .w_full()
                            .label(if reading {
                                text::READING_MORE_RESULTS
                            } else if failed {
                                text::TRY_AGAIN
                            } else {
                                text::SHOW_MORE_LINES
                            })
                            .disabled(reading)
                            .selector("more-lines")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    if refresh_failed {
                                        state.reload_recording(server_id, recording_id, cx);
                                    } else {
                                        state.read_transcript_lines(server_id, recording_id);
                                    }
                                });
                            })),
                    )
                    .into_any_element()
            });
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            // The panel was opened at a line: the lines before the read ones are read on
            // demand, from a control above the list that stays in view.
            .when(earlier, |column| {
                column.child(
                    div().flex().flex_none().px(u(14.0)).pt(u(8.0)).child(
                        Button::new("earlier-lines")
                            .kind(ButtonKind::Ghost)
                            .label(text::SHOW_EARLIER_LINES)
                            .disabled(reading)
                            .selector("earlier-lines")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, _| {
                                    state.read_earlier_transcript_lines(server_id, recording_id);
                                });
                            })),
                    ),
                )
            })
            .child(
                div()
                    .id("transcript-lines")
                    .role(Role::List)
                    .aria_label(text::TRANSCRIPT)
                    .debug_selector(|| "transcript-lines".to_owned())
                    .size_full()
                    .child(list(state.clone(), render).size_full()),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&state)
                        .viewport_from_layout()
                        .mode(ScrollbarMode::Always)
                        .styles(move |styles| {
                            let thumb = Hsla::from(colors.rule);
                            let held = Hsla::from(colors.ink_3);
                            styles
                                .track(|track| track.bg(gpui::transparent_black()).width(px(10.0)))
                                .track_hover(|track| track.bg(gpui::transparent_black()))
                                .track_active(|track| track.bg(gpui::transparent_black()))
                                .thumb(move |bar| {
                                    bar.bg(thumb).width(px(4.0)).radius(px(2.0)).inset(px(3.0))
                                })
                                .thumb_hover(move |bar| bar.bg(held))
                                .thumb_active(move |bar| bar.bg(held))
                        }),
                ),
            )
            .into_any_element()
    }

    /// The side panel of the transcript of one recording, with every line that was read.
    pub(crate) fn render_transcript_panel(
        &mut self,
        server_id: ServerId,
        panel: &TranscriptPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let recording_id = panel.recording_id;
        let header = SideHeader::new(text::TRANSCRIPT)
            .title_bar()
            .icon(Icon::Document)
            .note(panel.channel.clone())
            .trailing(
                Button::new("close-transcript")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE_TRANSCRIPT)
                    .selector("close-transcript")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.close_transcript(cx));
                    })),
            );
        let note = |sentence: &'static str| {
            div()
                .px(u(14.0))
                .pt(u(12.0))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .child(sentence)
                .into_any_element()
        };
        let body: AnyElement = match &panel.state {
            TranscriptPanelState::Reading => note(text::TRANSCRIPT_READING),
            TranscriptPanelState::Gone => {
                StatePanel::new(text::RECORDING_GONE_TITLE, text::RECORDING_GONE)
                    .selector("transcript-gone")
                    .action(
                        Button::new("reload-gone-transcript")
                            .icon(Icon::Retry)
                            .label(text::TRY_AGAIN)
                            .selector("reload-gone-transcript")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.reload_recording(server_id, recording_id, cx);
                                });
                            })),
                    )
                    .into_any_element()
            }
            TranscriptPanelState::Failed => {
                StatePanel::new(text::TRANSCRIPT, text::RECORDING_NOT_READ)
                    .icon(Icon::Warning)
                    .selector("transcript-failed")
                    .action(
                        Button::new("reload-transcript")
                            .icon(Icon::Retry)
                            .label(text::TRY_AGAIN)
                            .selector("reload-transcript")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.reload_recording(server_id, recording_id, cx);
                                });
                            })),
                    )
                    .into_any_element()
            }
            TranscriptPanelState::Shown { transcript, .. }
                if !matches!(
                    transcript.state,
                    TranscriptState::Ready | TranscriptState::Partial
                ) =>
            {
                StatePanel::new(
                    text::TRANSCRIPT,
                    text::transcript_state(transcript.state, transcript.failure),
                )
                .selector("transcript-state")
                .into_any_element()
            }
            TranscriptPanelState::Shown {
                transcript,
                open,
                names,
                when,
            } => {
                self.settle_transcript_form(server_id, recording_id, transcript, open, window, cx);
                self.transcript_lines(server_id, panel, transcript, open, names, *when, &theme, cx)
            }
        };
        side_panel(&theme, text::TRANSCRIPT, "transcript-panel")
            .child(header)
            .child(body)
            .into_any_element()
    }
}
