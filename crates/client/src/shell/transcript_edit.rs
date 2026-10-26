//! The changes of a transcript in its side panel (ADR 0616): the correction of a line, the
//! original text of a corrected line with who corrected it and when, the name of a voice of
//! shared sound, and the details of the transcript: the glossary terms that it used and the
//! result of each audio file.

use std::collections::HashSet;

use gpui::{
    AnyElement, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder,
};
use gpui_base::input::InputState;
use junto_client_core::model::OpenRecording;
use junto_client_core::protocol::ids::{RecordingId, ServerId};

use super::Shell;
use crate::assets::Icon;
use crate::state::recorded::{LineRow, TranscriptShown, VoiceRow};
use crate::state::transcript_work::{ChangeState, TranscriptChange};
use crate::text;
use crate::text::transcripts as txt;
use crate::theme::{Theme, u};
use crate::ui::{Button, ButtonKind, Field, FieldEvent, FormSpace, Notice, StyleExt, icon};

/// The form of a change: the recording and what it changes.
pub(crate) type FormKey = (RecordingId, TranscriptChange);

/// What the side panel of a transcript holds of the window itself.
pub(crate) struct TranscriptEdit {
    /// The text of the open form: a correction or a name.
    pub(crate) field: Entity<InputState>,
    /// The server and the change whose form is open.
    pub(crate) open: Option<(ServerId, FormKey)>,
    /// Why the text of the field was not sent.
    pub(crate) invalid: Option<&'static str>,
    /// The form was sent and its answer has not arrived.
    pub(crate) sent: bool,
    /// The space of the rows with a form (ADR 0730).
    pub(crate) space: FormSpace<FormKey>,
    /// The corrected lines whose original text the panel shows.
    pub(crate) originals: HashSet<(RecordingId, u32)>,
    /// The recording whose details the panel shows.
    pub(crate) details: Option<RecordingId>,
    /// The value that the open form was made from: the text of the line, or the name of
    /// the voice (`None`: no name). It is sent with the change, and the server refuses a
    /// change when the line or the voice has another value (ADR 0607).
    pub(crate) read: Option<String>,
    /// The line or the name changed on the server while the form was open.
    pub(crate) incoming: Option<Incoming>,
    /// The owner of the server whose transcript the panel showed (ADR 0745). Nothing of the
    /// panel stays for another owner.
    pub(crate) owner: Option<junto_client_core::book::Owner>,
}

/// What the form tells about a change that arrived while it was open: of another person, or
/// of the same account on another device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    /// The person had not changed the text: the form holds the new value.
    Taken,
    /// The person had changed the text: it stays, and the new value is told.
    Kept(String),
}

impl TranscriptEdit {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Subscription) {
        let field = Field::state("", window, cx);
        let subscription = Field::observe(
            &field,
            window,
            cx,
            |this: &mut Shell, event, window, cx| match event {
                FieldEvent::Changed => {
                    this.transcript_edit.invalid = None;
                    cx.notify();
                }
                FieldEvent::Submitted => this.submit_transcript_form(window, cx),
            },
        );
        let edit = Self {
            field,
            open: None,
            invalid: None,
            sent: false,
            space: FormSpace::default(),
            originals: HashSet::new(),
            details: None,
            read: None,
            incoming: None,
            owner: None,
        };
        (edit, subscription)
    }
}

impl Shell {
    /// Opens the form of a change with the value that the panel shows now: the text of the
    /// line, or the name of the voice (`None`: no name).
    pub fn open_transcript_form(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        what: TranscriptChange,
        shown: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = self.state.read(cx).owner_of(server_id);
        let edit = &mut self.transcript_edit;
        edit.open = Some((server_id, (recording_id, what)));
        edit.invalid = None;
        edit.sent = false;
        edit.incoming = None;
        edit.read = shown.map(str::to_owned);
        edit.owner = owner;
        edit.space.release();
        let shown = shown.unwrap_or_default().to_owned();
        let field = edit.field.clone();
        field.update(cx, |field, cx| field.set_value(shown, window, cx));
        self.focus_field(&field, window, cx);
        self.state.update(cx, |state, cx| {
            state.dismiss_transcript_ask(server_id, recording_id, cx);
        });
        cx.notify();
    }

    /// Puts the open form away. A request that is on its way still changes the transcript.
    pub(crate) fn close_transcript_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.transcript_edit.open.take();
        self.transcript_edit.invalid = None;
        self.transcript_edit.sent = false;
        self.transcript_edit.incoming = None;
        self.transcript_edit.read = None;
        self.transcript_edit.space.release();
        let field = self.transcript_edit.field.clone();
        field.update(cx, |field, cx| field.set_value("", window, cx));
        if let Some((server_id, (recording_id, _))) = open {
            self.state.update(cx, |state, cx| {
                state.dismiss_transcript_ask(server_id, recording_id, cx);
            });
        }
        cx.notify();
    }

    /// Sends the open form. A text that the server would refuse is told in the form; a
    /// text that changes nothing closes it.
    pub(crate) fn submit_transcript_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((server_id, (recording_id, what))) = self.transcript_edit.open else {
            return;
        };
        if self.transcript_edit.sent {
            return;
        }
        let typed = self.transcript_edit.field.read(cx).value().to_string();
        let read = self.transcript_edit.read.clone();
        let sent = self.state.update(cx, |state, cx| {
            state.change_transcript(server_id, recording_id, what, &typed, read, cx)
        });
        match sent {
            Ok(true) => {
                self.transcript_edit.sent = true;
                self.transcript_edit.incoming = None;
            }
            Ok(false) => self.close_transcript_form(window, cx),
            Err(problem) => self.transcript_edit.invalid = Some(problem),
        }
        cx.notify();
    }

    /// Follows the state for the open form. Nothing of the panel stays for another owner
    /// of the server. A change of another person while the form is open is shown in it,
    /// and the form is made from that value from then on. The answer of the sent form: the
    /// server did it, and the form closes with its row keeping the height that it had; or
    /// the server refused it, and the form stays with the reason.
    pub(crate) fn settle_transcript_form(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        transcript: &TranscriptShown,
        open: &OpenRecording,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let owner = self.state.read(cx).owner_of(server_id);
        let edit = &mut self.transcript_edit;
        // A draft, a shown original and open details belong to the owner that made them
        // (ADR 0745): another account of the server, or none, gets none of them.
        if edit.owner != owner {
            edit.owner = owner;
            edit.open = None;
            edit.sent = false;
            edit.invalid = None;
            edit.incoming = None;
            edit.read = None;
            edit.originals.clear();
            edit.details = None;
            edit.space.release();
            let field = edit.field.clone();
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        let edit = &mut self.transcript_edit;
        // The panel shows another recording: what was open belongs to the one before.
        if edit
            .open
            .is_some_and(|(of_server, (of, _))| of != recording_id || of_server != server_id)
        {
            edit.open = None;
            edit.sent = false;
            edit.invalid = None;
            edit.incoming = None;
            edit.read = None;
            edit.space.release();
        }
        // A change of another person while the form is open and not sent.
        if let Some((_, (_, what))) = edit.open.filter(|_| !edit.sent) {
            let now = match what {
                TranscriptChange::Line(position) => {
                    open.line(position).map(|line| Some(line.text.clone()))
                }
                TranscriptChange::Speaker(number) => {
                    open.speaker(number).map(|speaker| speaker.name.clone())
                }
            };
            if let Some(now) = now.filter(|now| *now != edit.read) {
                let draft = edit.field.read(cx).value().trim().to_owned();
                let untouched = draft == edit.read.clone().unwrap_or_default();
                edit.incoming = Some(if untouched {
                    let field = edit.field.clone();
                    let value = now.clone().unwrap_or_default();
                    field.update(cx, |field, cx| field.set_value(value, window, cx));
                    Incoming::Taken
                } else {
                    Incoming::Kept(now.clone().unwrap_or_default())
                });
                edit.read = now;
            }
        }
        if edit.details.is_some_and(|of| of != recording_id) {
            edit.details = None;
        }
        edit.originals.retain(|(of, _)| *of == recording_id);
        let Some((_, key)) = edit.open.filter(|_| edit.sent) else {
            return;
        };
        match transcript.ask.as_ref().filter(|ask| ask.what == key.1) {
            Some(ask) if ask.state == ChangeState::Sending => {}
            Some(_) => edit.sent = false,
            None => {
                edit.open = None;
                edit.sent = false;
                edit.invalid = None;
                edit.incoming = None;
                edit.read = None;
                edit.space.keep(key, txt::CHANGE_SAVED);
                let field = edit.field.clone();
                field.update(cx, |field, cx| field.set_value("", window, cx));
            }
        }
    }

    /// Shows or hides the original text of a corrected line.
    pub fn toggle_original(
        &mut self,
        recording_id: RecordingId,
        line: u32,
        cx: &mut Context<Self>,
    ) {
        let key = (recording_id, line);
        if !self.transcript_edit.originals.remove(&key) {
            self.transcript_edit.originals.insert(key);
        }
        cx.notify();
    }

    /// Shows or hides the details of the transcript of the side panel.
    pub fn toggle_transcript_details(&mut self, recording_id: RecordingId, cx: &mut Context<Self>) {
        let edit = &mut self.transcript_edit;
        edit.details = match edit.details {
            Some(open) if open == recording_id => None,
            _ => Some(recording_id),
        };
        edit.space.release();
        cx.notify();
    }

    /// The form of a change: its field, the two buttons, and the slot for what the window
    /// or the server tells about it.
    fn transcript_form(
        &mut self,
        label: &'static str,
        sending: bool,
        refused: Option<String>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let field = self.transcript_edit.field.clone();
        // A change that arrived after a refusal is newer than the refusal: a refusal because
        // the value changed meanwhile is answered by the new value, which a second Save
        // replaces (ADR 0616). A send puts away the change that the form named.
        let told = self
            .transcript_edit
            .invalid
            .map(|invalid| {
                Notice::error(invalid)
                    .selector("transcript-form-invalid")
                    .into_any_element()
            })
            .or_else(|| {
                self.transcript_edit.incoming.as_ref().map(|incoming| {
                    let sentence = match incoming {
                        Incoming::Taken => txt::CHANGED_TAKEN.to_owned(),
                        Incoming::Kept(now) => txt::changed_kept(now),
                    };
                    Notice::new(sentence)
                        .selector("transcript-form-changed")
                        .into_any_element()
                })
            })
            .or_else(|| {
                refused.map(|reason| {
                    Notice::error(reason)
                        .selector("transcript-form-refused")
                        .into_any_element()
                })
            });
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(u(6.0))
            .pt(u(4.0))
            .child(Field::new(&field, label).selector("transcript-form-field"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(u(6.0))
                    .child(
                        Button::new("transcript-form-cancel")
                            .label(text::CANCEL)
                            .selector("transcript-form-cancel")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_transcript_form(window, cx);
                            })),
                    )
                    .child(
                        Button::new("transcript-form-send")
                            .kind(ButtonKind::Primary)
                            .label(if sending {
                                text::admin::SENDING
                            } else {
                                text::SAVE
                            })
                            .selector("transcript-form-send")
                            .disabled(sending)
                            .h(u(28.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_transcript_form(window, cx);
                            })),
                    ),
            )
            .child(crate::ui::notice_slot("transcript-form-slot").children(told))
            .child(div().h(u(2.0)))
            .typeface(theme.types.small, theme)
            .into_any_element()
    }

    /// The state of the request of the open form, when it is the form `key`.
    fn form_state(transcript: &TranscriptShown, key: FormKey) -> (bool, Option<String>) {
        match transcript.ask.as_ref().filter(|ask| ask.what == key.1) {
            Some(ask) => match &ask.state {
                ChangeState::Sending => (true, None),
                ChangeState::Refused(reason) => (false, Some(reason.clone())),
            },
            None => (false, None),
        }
    }

    /// The part of a line of the side panel under its speaker: the text, or the form that
    /// corrects it, and the original text when the person asked for it. Returns the part
    /// and the control at the right of the line.
    pub(crate) fn line_body(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        row: &LineRow,
        transcript: &TranscriptShown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> (AnyElement, Option<AnyElement>) {
        let colors = theme.colors;
        let line = row.line;
        let key = (recording_id, TranscriptChange::Line(line));
        let editing = self
            .transcript_edit
            .open
            .is_some_and(|(_, open)| open == key);
        if editing {
            let (sending, refused) = Self::form_state(transcript, key);
            let form = self.transcript_form(txt::CORRECTED_TEXT, sending, refused, theme, cx);
            return (form, None);
        }
        let spoken = div()
            .text_size(u(14.5))
            .line_height(u(22.0))
            .ink(colors.ink)
            .child(SharedString::from(row.text.clone()));
        let shows_original = self
            .transcript_edit
            .originals
            .contains(&(recording_id, line));
        let original = row
            .original
            .clone()
            .filter(|_| shows_original)
            .map(|original| {
                div()
                    .id(("line-original-text", line as usize))
                    .debug_selector(move || format!("line-original-text:{line}"))
                    .mt(u(3.0))
                    .pl(u(8.0))
                    .border_l_2()
                    .border_color(Hsla::from(colors.rule))
                    .text_size(u(13.0))
                    .line_height(u(19.0))
                    .ink(colors.ink_2)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(txt::ORIGINAL))
                    .child(SharedString::from(original))
                    .when_some(row.changed.clone(), |block, changed| {
                        block.child(
                            div()
                                .debug_selector(move || format!("line-changed:{line}"))
                                .ink(colors.ink_3)
                                .child(SharedString::from(txt::changed_by(
                                    &changed.by,
                                    &changed.at,
                                ))),
                        )
                    })
            });
        let body = div().child(spoken).children(original).into_any_element();
        let correct = Button::new(("correct-line", line as usize))
            .kind(ButtonKind::Ghost)
            .icon(Icon::Edit)
            .tooltip(if transcript.may_change {
                txt::CORRECT_LINE
            } else {
                txt::ARCHIVED
            })
            .accessible_label(txt::CORRECT_LINE)
            .disabled(!transcript.may_change)
            .on_click({
                let shown = row.text.clone();
                cx.listener(move |this, _, window, cx| {
                    this.open_transcript_form(
                        server_id,
                        recording_id,
                        TranscriptChange::Line(line),
                        Some(&shown),
                        window,
                        cx,
                    );
                })
            });
        let control = div()
            .flex_none()
            .debug_selector(move || format!("correct-line:{line}"))
            .child(correct)
            .into_any_element();
        (body, Some(control))
    }

    /// The mark of a corrected line beside its speaker. A press shows or hides the text
    /// that the speech model wrote.
    pub(crate) fn corrected_mark(
        &mut self,
        recording_id: RecordingId,
        line: u32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let ring = crate::ui::focus_ring(colors.terracotta_deep, theme.metrics.focus_ring);
        div()
            .id(("line-original", line as usize))
            .role(gpui::Role::Button)
            .aria_label(txt::SHOW_ORIGINAL)
            .debug_selector(move || format!("line-original:{line}"))
            .flex_none()
            .text_size(u(12.5))
            .font_weight(FontWeight::NORMAL)
            .ink(colors.cypress_ink)
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .focus_visible(move |style| style.shadow(ring))
            .tooltip(crate::ui::tooltip(txt::SHOW_ORIGINAL))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_original(recording_id, line, cx);
            }))
            .child(text::LINE_CORRECTED)
            .into_any_element()
    }

    /// One voice of shared sound in the details, with its name and the control that names
    /// it, or the form that names it.
    fn voice_row(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        voice: &VoiceRow,
        transcript: &TranscriptShown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let number = voice.number;
        let key = (recording_id, TranscriptChange::Speaker(number));
        let editing = self
            .transcript_edit
            .open
            .is_some_and(|(_, open)| open == key);
        let content: AnyElement = if editing {
            let (sending, refused) = Self::form_state(transcript, key);
            self.transcript_form(txt::VOICE_NAME, sending, refused, theme, cx)
        } else {
            let shown = voice.name.clone();
            div()
                .flex()
                .items_start()
                .gap(u(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(u(14.0))
                                .font_weight(FontWeight::BOLD)
                                .ink(colors.ink)
                                .truncate()
                                .child(SharedString::from(
                                    voice.name.clone().unwrap_or_else(|| voice.label.clone()),
                                )),
                        )
                        .child(
                            div()
                                .text_size(u(12.5))
                                .ink(colors.ink_2)
                                .child(SharedString::from(txt::voice_of(&voice.label, &voice.of))),
                        )
                        .when_some(voice.changed.clone(), |column, changed| {
                            column.child(
                                div()
                                    .debug_selector(move || format!("voice-changed:{number}"))
                                    .text_size(u(12.5))
                                    .ink(colors.ink_3)
                                    .child(SharedString::from(txt::named_by(
                                        &changed.by,
                                        &changed.at,
                                    ))),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .debug_selector(move || format!("name-voice:{number}"))
                        .child(
                            Button::new(("name-voice", number as usize))
                                .label(if voice.name.is_some() {
                                    txt::RENAME_VOICE
                                } else {
                                    txt::NAME_VOICE
                                })
                                .tooltip(if transcript.may_change {
                                    txt::NAME_VOICE_HELP
                                } else {
                                    txt::ARCHIVED
                                })
                                .disabled(!transcript.may_change)
                                .h(u(28.0))
                                .px(u(10.0))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_transcript_form(
                                        server_id,
                                        recording_id,
                                        TranscriptChange::Speaker(number),
                                        shown.as_deref(),
                                        window,
                                        cx,
                                    );
                                })),
                        ),
                )
                .into_any_element()
        };
        let open = self.transcript_edit.open.map(|(_, open)| open);
        self.transcript_edit
            .space
            .row(open, &[key], div().w_full().py(u(4.0)).child(content))
            .debug_selector(move || format!("voice:{number}"))
            .into_any_element()
    }

    /// The details of a transcript at the head of its list: the glossary terms that it
    /// used and cut, the voices of shared sound, and the result of each audio file.
    pub(crate) fn transcript_details(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        transcript: &TranscriptShown,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let heading = |label: &'static str| {
            div()
                .mt(u(8.0))
                .mb(u(2.0))
                .typeface(theme.types.label, theme)
                .ink(colors.bronze)
                .child(label)
        };
        let small = |sentence: String| {
            div()
                .text_size(u(13.0))
                .line_height(u(19.0))
                .ink(colors.ink_2)
                .child(SharedString::from(sentence))
        };
        let mut column = div()
            .id("transcript-details-panel")
            .debug_selector(|| "transcript-details-panel".to_owned())
            .flex()
            .flex_col()
            .px(u(14.0))
            .pb(u(8.0))
            .child(heading(txt::GLOSSARY));
        if transcript.used.is_empty() && transcript.cut.is_empty() {
            column = column.child(small(txt::GLOSSARY_EMPTY.to_owned()));
        }
        if !transcript.used.is_empty() {
            column = column.child(
                small(txt::glossary_used(&transcript.used))
                    .debug_selector(|| "glossary-used".to_owned()),
            );
        }
        if !transcript.cut.is_empty() {
            column = column.child(
                small(txt::glossary_cut(&transcript.cut))
                    .debug_selector(|| "glossary-cut".to_owned()),
            );
        }
        if !transcript.voices.is_empty() {
            column = column.child(heading(txt::VOICES));
            for voice in &transcript.voices {
                let row = self.voice_row(server_id, recording_id, voice, transcript, theme, cx);
                column = column.child(row);
            }
        }
        if !transcript.segments.is_empty() {
            column = column.child(heading(txt::AUDIO_FILES));
            for (index, result) in transcript.segments.iter().enumerate() {
                column = column.child(
                    div()
                        .debug_selector(move || format!("transcript-segment:{index}"))
                        .flex()
                        .items_start()
                        .gap(u(6.0))
                        .py(u(2.0))
                        .when(result.failed, |row| {
                            row.child(div().mt(u(3.0)).child(icon(
                                Icon::Warning,
                                13.0,
                                colors.cinnabar,
                            )))
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(u(13.0))
                                .line_height(u(19.0))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .ink(colors.ink)
                                        .child(SharedString::from(result.title.clone())),
                                )
                                .child(
                                    div()
                                        .ink(if result.failed {
                                            colors.cinnabar_ink
                                        } else {
                                            colors.ink_2
                                        })
                                        .child(SharedString::from(result.sentence.clone())),
                                ),
                        ),
                );
            }
        }
        column.into_any_element()
    }
}

impl Shell {
    /// What the open form of the transcript panel tells about a change that arrived while
    /// it was open.
    pub fn transcript_incoming(&self) -> Option<Incoming> {
        self.transcript_edit.incoming.clone()
    }

    /// The text of the field of the open form of the transcript panel.
    pub fn transcript_text(&self, cx: &gpui::App) -> String {
        self.transcript_edit.field.read(cx).value().to_string()
    }

    /// Puts text into the field of the open form of the transcript panel, as typing does.
    pub fn type_transcript_text(
        &mut self,
        typed: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typed = typed.to_owned();
        self.transcript_edit
            .field
            .update(cx, |field, cx| field.set_value(typed, window, cx));
    }
}
