//! The composer of a message (ADR 0740): a text area with the marks of
//! [`crate::compose`], the toolbar of the design, mentions as tokens with a picker, the
//! emoji picker, the files of the next message, and the states of a reply and of an edit.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use gpui::{
    AnyElement, Context, Entity, ExternalPaths, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, PathPromptOptions, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_base::input::{
    self, InlineToken, InputContent, InputEditorStyle, InputEvent, InputState, Textarea,
    TextareaState,
};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{AccountId, MessageId, ServerId};
use junto_client_core::protocol::message::{Document, Message};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::transfers::{UploadSource, UploadStage};

use super::{EmojiTarget, Layer, Pane, Shell};
use crate::assets::Icon;
use crate::compose::{self, Mention, MentionTarget};
use crate::state::feed::{Feed, FeedRow, plain_text};
use crate::state::view::{DirectRow, DraftFile, Members, ShellView, WorkspaceView};
use crate::text;
use crate::theme::{theme, u};
use crate::ui::{
    Button, ButtonKind, Dialog, Field, FieldKind, ScrollArea, StyleExt, float_shadow,
    focus_ring_inside, icon, ledge, tooltip,
};

/// The rows of the text area: it grows with the text up to this number.
const MOST_ROWS: usize = 10;
/// The members and roles that the mention picker lists at one time.
const MOST_CANDIDATES: usize = 8;
/// The emoji that the picker draws at one time.
const MOST_EMOJI: usize = 240;

/// What stays with a place while the window shows another one.
struct Draft {
    content: InputContent,
    reply_to: Option<MessageId>,
    editing: Option<Editing>,
}

/// The list of members and roles that a typed `@` opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MentionPicker {
    /// The byte of the text at which the `@` stands.
    start: usize,
    query: String,
    /// The candidate that Enter takes.
    chosen: usize,
}

/// A key of the text area that the composer can take before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Up,
    Down,
    Tab,
    Escape,
}

/// One entry of the mention picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub target: MentionTarget,
    pub name: String,
    /// What the entry is, under its name.
    pub detail: &'static str,
}

/// A place of a server and the account that writes there. The text that one account
/// left in a place is not shown to another account of the same server.
type Desk = (ServerId, AccountId, PlaceId);

/// The message that the composer changes.
#[derive(Clone)]
pub(crate) struct Editing {
    pub(crate) message_id: MessageId,
    /// The document at the start of the edit, and the text that the composer got for it.
    /// What the person does not change is taken from the document (ADR 0740).
    original: Arc<(Document, compose::Written)>,
}

/// Files attached to the next message of a place whose sizes are being read.
struct Attaching {
    number: u64,
    /// The place and the account that the composer wrote to when the files were attached,
    /// and the owner that the server had then (ADR 0746).
    asked: Desk,
    owner: junto_client_core::Owner,
    /// The sizes, when they have been read.
    measured: Option<Vec<crate::state::Measured>>,
}

pub(crate) struct Composer {
    pub(crate) input: Entity<TextareaState>,
    /// The place whose text the input holds, and the account that writes there.
    place: Option<Desk>,
    /// The composer has held the text of a place before.
    placed: bool,
    pub(crate) reply_to: Option<MessageId>,
    pub(crate) editing: Option<Editing>,
    drafts: HashMap<Desk, Draft>,
    /// Why the message was not sent as it is.
    pub(crate) problem: Option<String>,
    /// Files whose sizes are read on a background thread, in the order in which they were
    /// attached. They go into the draft in that order, and a new message of their place
    /// is not sent before they are in it.
    attaching: VecDeque<Attaching>,
    next_attaching: u64,
    pub(crate) mention: Option<MentionPicker>,
    pub(crate) emoji_find: Entity<InputState>,
    emoji_group: usize,
    emoji_scroll: ScrollHandle,
}

fn token_id(target: MentionTarget) -> String {
    match target {
        MentionTarget::Account(account_id) => format!("account:{account_id}"),
        MentionTarget::Role(role_id) => format!("role:{role_id}"),
        MentionTarget::Everyone => "everyone".to_owned(),
    }
}

fn token_target(id: &str) -> Option<MentionTarget> {
    match id.split_once(':') {
        Some(("account", id)) => id.parse().ok().map(MentionTarget::Account),
        Some(("role", id)) => id.parse().ok().map(MentionTarget::Role),
        None if id == "everyone" => Some(MentionTarget::Everyone),
        _ => None,
    }
}

/// The text of the input with its tokens as the mentions of [`compose`].
fn mentions_of(content: &InputContent) -> Vec<Mention> {
    content
        .tokens()
        .iter()
        .filter_map(|span| {
            Some(Mention {
                range: span.range(),
                target: token_target(span.token().id())?,
            })
        })
        .collect()
}

/// The text after the last `@` before the cursor, when a mention can start there: the
/// `@` stands at the start of the text or after white space, and no line ends after it.
fn mention_query(text: &str, cursor: usize) -> Option<(usize, String)> {
    let before = text.get(..cursor)?;
    let start = before.rfind('@')?;
    let query = &before[start + 1..];
    let opens = before[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    (opens && query.chars().count() <= 32 && !query.contains('\n'))
        .then(|| (start, query.to_owned()))
}

/// The other participant of a direct conversation, when the query finds the name. A
/// participant whose name the client has not read yet is not offered: the mention would
/// have no name to show.
fn direct_candidates(row: &DirectRow, query: &str) -> Vec<Candidate> {
    row.name
        .as_ref()
        .map(|name| Candidate {
            target: MentionTarget::Account(row.account_id),
            name: name.clone(),
            detail: text::IN_THIS_CONVERSATION,
        })
        .filter(|candidate| rank(candidate, &query.to_lowercase()).is_some())
        .into_iter()
        .collect()
}

/// Where a candidate stands for a query in lower case: 0 when its name starts with the
/// query, 1 when the name holds it, and absent otherwise.
fn rank(candidate: &Candidate, query: &str) -> Option<u8> {
    let name = candidate.name.to_lowercase();
    if name.starts_with(query) {
        Some(0)
    } else if name.contains(query) {
        Some(1)
    } else {
        None
    }
}

/// The members and roles whose names start with the query, and then those that hold it.
fn candidates(
    workspace: &WorkspaceView,
    own: Option<junto_client_core::protocol::ids::AccountId>,
    query: &str,
) -> Vec<Candidate> {
    let query = query.to_lowercase();
    let mut all: Vec<Candidate> = Vec::new();
    if let Members::Loaded(members) = &workspace.members {
        all.extend(
            members
                .iter()
                .filter(|member| Some(member.account_id) != own)
                .map(|member| Candidate {
                    target: MentionTarget::Account(member.account_id),
                    name: member.name.clone(),
                    detail: text::MEMBER,
                }),
        );
    }
    all.extend(workspace.roles.iter().map(|(role_id, name)| Candidate {
        target: MentionTarget::Role(*role_id),
        name: name.clone(),
        detail: text::ROLE,
    }));
    if workspace.allowed.mention_everyone {
        all.push(Candidate {
            target: MentionTarget::Everyone,
            name: text::EVERYONE.to_owned(),
            detail: text::EVERYONE_HELP,
        });
    }
    let mut found: Vec<(u8, Candidate)> = all
        .into_iter()
        .filter_map(|candidate| rank(&candidate, &query).map(|rank| (rank, candidate)))
        .collect();
    found.sort_by(|one, other| {
        one.0
            .cmp(&other.0)
            .then_with(|| one.1.name.to_lowercase().cmp(&other.1.name.to_lowercase()))
    });
    found
        .into_iter()
        .map(|(_, candidate)| candidate)
        .take(MOST_CANDIDATES)
        .collect()
}

/// The emoji that the picker offers: one group, or what a search finds in all groups. An
/// emoji with a skin tone is not listed by itself.
fn emoji_shown(group: usize, find: &str) -> Vec<&'static emojis::Emoji> {
    let find = find.trim().to_lowercase();
    if find.is_empty() {
        return emojis::Group::iter()
            .nth(group)
            .map(|group| group.emojis().take(MOST_EMOJI).collect())
            .unwrap_or_default();
    }
    emojis::iter()
        .filter(|emoji| {
            emoji.name().contains(&find)
                || emoji
                    .shortcodes()
                    .any(|shortcode| shortcode.contains(&find))
        })
        .take(MOST_EMOJI)
        .collect()
}

fn group_name(group: emojis::Group) -> &'static str {
    match group {
        emojis::Group::SmileysAndEmotion => "Smileys",
        emojis::Group::PeopleAndBody => "People",
        emojis::Group::AnimalsAndNature => "Nature",
        emojis::Group::FoodAndDrink => "Food",
        emojis::Group::TravelAndPlaces => "Travel",
        emojis::Group::Activities => "Activities",
        emojis::Group::Objects => "Objects",
        emojis::Group::Symbols => "Symbols",
        emojis::Group::Flags => "Flags",
    }
}

impl Composer {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        let input = gpui::AppContext::new(cx, |cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MOST_ROWS)
                .submit_on_enter(true)
        });
        Self {
            input,
            place: None,
            placed: false,
            reply_to: None,
            editing: None,
            drafts: HashMap::new(),
            problem: None,
            attaching: VecDeque::new(),
            next_attaching: 0,
            mention: None,
            emoji_find: Field::state(text::FIND_EMOJI, window, cx),
            emoji_group: 0,
            emoji_scroll: ScrollHandle::new(),
        }
    }
}

impl Shell {
    /// The place that the composer writes to.
    fn composer_place(&self, pane: Pane) -> Option<(ServerId, PlaceId)> {
        self.pen(pane)
            .place
            .map(|(server_id, _, place)| (server_id, place))
    }

    fn composer_feed(&self, pane: Pane) -> Option<&Feed> {
        self.pane_feed(pane)
    }

    /// Each composer holds the text of the place that its pane shows: the place of the
    /// main area, and the thread of the side panel. The text of the place before stays
    /// with that place.
    pub(crate) fn composer_follows(
        &mut self,
        view: &ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let desk = |place: Option<PlaceId>| -> Option<Desk> {
            Some((view.selected.as_ref()?.id, view.person.account_id?, place?))
        };
        let workspace = view.workspace();
        let shown = view.shown();
        let main: SharedString = workspace
            .map(
                |workspace| match (&workspace.conversation, &workspace.channel) {
                    (Some(conversation), _) => text::direct_placeholder(
                        conversation.name.as_deref().unwrap_or(text::UNKNOWN_PERSON),
                    ),
                    (None, Some(channel)) => {
                        text::message_placeholder(&channel.name, channel.kind == ChannelKind::Voice)
                    }
                    (None, None) => String::new(),
                },
            )
            .unwrap_or_default()
            .into();
        self.pen_follows(Pane::Main, desk(shown.place()), main, window, cx);
        let thread = desk(shown.thread_place());
        self.pen_follows(
            Pane::Thread,
            thread,
            text::REPLY_IN_THREAD.into(),
            window,
            cx,
        );
    }

    fn pen_follows(
        &mut self,
        pane: Pane,
        shown: Option<Desk>,
        placeholder: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pen = self.pen_mut(pane);
        if shown == pen.place {
            return;
        }
        if let Some((server_id, account_id, _)) = shown {
            // The server has another account now: the texts of the one before are gone.
            pen.drafts
                .retain(|(server, account, _), _| *server != server_id || *account == account_id);
        }
        let content = pen.input.read(cx).content();
        if let Some(before) = pen.place.take() {
            let draft = Draft {
                content,
                reply_to: pen.reply_to.take(),
                editing: pen.editing.take(),
            };
            let empty = draft.content.text().trim().is_empty()
                && draft.reply_to.is_none()
                && draft.editing.is_none();
            if empty {
                pen.drafts.remove(&before);
            } else {
                pen.drafts.insert(before, draft);
            }
        }
        let first = shown.is_some() && !std::mem::replace(&mut pen.placed, true);
        let held = shown.and_then(|place| pen.drafts.remove(&place));
        let (content, reply_to, editing) = match held {
            Some(draft) => (draft.content, draft.reply_to, draft.editing),
            // What was put into the composer before the window had a place stays.
            None if first => (
                pen.input.read(cx).content(),
                pen.reply_to,
                pen.editing.clone(),
            ),
            None => (InputContent::new(""), None, None),
        };
        pen.input.update(cx, |input, cx| {
            input.set_value(content, window, cx);
            input.set_placeholder(placeholder, window, cx);
        });
        pen.place = shown;
        pen.reply_to = reply_to;
        pen.editing = editing;
        if !first {
            pen.problem = None;
            pen.mention = None;
        }
    }

    /// The text of the composer changed: the mention picker follows the text before the
    /// cursor, and a statement about the text before is put away.
    pub(crate) fn composer_changed(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let pen = self.pen_mut(pane);
        pen.problem = None;
        let (value, cursor) = {
            let input = pen.input.read(cx);
            (input.value(), input.cursor())
        };
        let query = mention_query(&value, cursor);
        let open = pen.mention.take();
        pen.mention = query.map(|(start, query)| MentionPicker {
            chosen: match &open {
                Some(open) if open.start == start && open.query == query => open.chosen,
                _ => 0,
            },
            start,
            query,
        });
        cx.notify();
    }

    /// The entries of the mention picker of a pane. A message of a channel mentions the
    /// members and the roles of its workspace; a message of a direct conversation mentions
    /// only its two participants, so the picker offers the other one (ADR 0408).
    fn mention_candidates(&self, pane: Pane) -> Vec<Candidate> {
        let (Some(view), Some(open)) = (&self.view, &self.pen(pane).mention) else {
            return Vec::new();
        };
        let Some(workspace) = view.workspace() else {
            return Vec::new();
        };
        match self.composer_place(pane).map(|(_, place)| place) {
            Some(PlaceId::Direct(conversation_id)) => workspace
                .directs
                .iter()
                .chain(&workspace.conversation)
                .find(|row| row.id == conversation_id)
                .map(|row| direct_candidates(row, &open.query))
                .unwrap_or_default(),
            _ => candidates(workspace, view.person.account_id, &open.query),
        }
    }

    /// Puts a mention into the text in place of the typed `@` and its query.
    fn take_candidate(
        &mut self,
        pane: Pane,
        candidate: &Candidate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.pen_mut(pane).mention.take() else {
            return;
        };
        let token = InlineToken::new(token_id(candidate.target), format!("@{}", candidate.name));
        self.pen_mut(pane).input.update(cx, |input, cx| {
            let end = input.cursor();
            if input
                .replace_range_with_token(open.start..end, token, window, cx)
                .is_ok()
            {
                input.insert(" ", window, cx);
            }
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// A key of the text area before the text area takes it. The mention picker takes the
    /// arrow keys, Tab and Escape; an empty composer edits the last own message with the
    /// arrow up; Escape ends a reply or an edit. True when the key was taken.
    fn composer_step(
        &mut self,
        pane: Pane,
        step: Step,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pen_mut(pane).mention.is_some() {
            let found = self.mention_candidates(pane);
            if !found.is_empty() {
                let Some(open) = &mut self.pen_mut(pane).mention else {
                    return false;
                };
                match step {
                    Step::Down => open.chosen = (open.chosen + 1) % found.len(),
                    Step::Up => open.chosen = (open.chosen + found.len() - 1) % found.len(),
                    Step::Tab => {
                        let chosen = found[open.chosen.min(found.len() - 1)].clone();
                        self.take_candidate(pane, &chosen, window, cx);
                    }
                    Step::Escape => self.pen_mut(pane).mention = None,
                }
                cx.notify();
                return true;
            }
        }
        let empty = self.pen_mut(pane).input.read(cx).value().is_empty();
        match step {
            Step::Escape
                if self.pen_mut(pane).editing.is_some()
                    || self.pen_mut(pane).reply_to.is_some() =>
            {
                self.cancel_compose_state(pane, window, cx);
                true
            }
            Step::Up if empty && self.pen_mut(pane).editing.is_none() => {
                let last = self.composer_feed(pane).and_then(|feed| {
                    feed.rows.iter().rev().find_map(|row| match row {
                        FeedRow::Message(row) if row.can_edit => Some(row.message.id),
                        _ => None,
                    })
                });
                match last {
                    Some(message_id) => {
                        self.edit_message(pane, message_id, window, cx);
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
    }

    /// A key that the text area has no action for: the keys of bold and italic.
    fn composer_key(
        &mut self,
        pane: Pane,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let modifiers = event.keystroke.modifiers;
        if !modifiers.secondary() || modifiers.shift {
            return false;
        }
        match event.keystroke.key.as_str() {
            "b" => self.wrap_selection(pane, "**", "**", window, cx),
            "i" => self.wrap_selection(pane, "*", "*", window, cx),
            _ => return false,
        }
        true
    }

    /// A picture in the clipboard becomes a file of the message. False when the clipboard
    /// holds no picture: the text area pastes its text.
    fn paste_picture(&mut self, pane: Pane, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let pictures: Vec<UploadSource> = item
            .into_entries()
            .filter_map(|entry| match entry {
                gpui::ClipboardEntry::Image(image) if image.format == gpui::ImageFormat::Png => {
                    Some(UploadSource::Bytes {
                        name: text::PASTED_PICTURE.to_owned(),
                        bytes: Arc::new(image.bytes),
                    })
                }
                _ => None,
            })
            .collect();
        if pictures.is_empty() {
            return false;
        }
        self.attach(pane, pictures, cx);
        true
    }

    /// Attaches files to the next message of the shown place. The sizes of the files are
    /// read on a background thread (ADR 0730); the files are attached when that is done,
    /// after the files that were attached before them, while the composer still writes to
    /// the place and the server still has the owner that asked (ADR 0746).
    pub(crate) fn attach(
        &mut self,
        pane: Pane,
        sources: Vec<UploadSource>,
        cx: &mut Context<Self>,
    ) {
        let Some(asked) = self.pen(pane).place else {
            return;
        };
        let Some(owner) = self.state.read(cx).owner(asked.0) else {
            return;
        };
        let pen = self.pen_mut(pane);
        let number = pen.next_attaching;
        pen.next_attaching += 1;
        pen.attaching.push_back(Attaching {
            number,
            asked,
            owner,
            measured: None,
        });
        cx.notify();
        let measuring = cx
            .background_executor()
            .spawn(async move { crate::state::measure(sources) });
        cx.spawn(async move |this, cx| {
            let measured = measuring.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(entry) = this
                    .pen_mut(pane)
                    .attaching
                    .iter_mut()
                    .find(|entry| entry.number == number)
                {
                    entry.measured = Some(measured);
                }
                this.attach_measured(pane, cx);
            });
        })
        .detach();
    }

    /// Puts the measured files into the draft of their place, in the order in which they
    /// were attached: it stops at the first files whose sizes are still being read.
    fn attach_measured(&mut self, pane: Pane, cx: &mut Context<Self>) {
        while self
            .pen(pane)
            .attaching
            .front()
            .is_some_and(|entry| entry.measured.is_some())
        {
            let Some(Attaching {
                asked,
                owner,
                measured: Some(measured),
                ..
            }) = self.pen_mut(pane).attaching.pop_front()
            else {
                return;
            };
            let same_owner = self.state.read(cx).owner(asked.0) == Some(owner);
            if self.pen(pane).place != Some(asked) || !same_owner {
                self.pen_mut(pane).problem = Some(text::FILES_OF_ANOTHER_PLACE.to_owned());
                continue;
            }
            let (server_id, _, place) = asked;
            let refused = self
                .state
                .update(cx, |state, cx| state.attach(server_id, place, measured, cx));
            self.pen_mut(pane).problem = refused.first().map(text::attach_refusal);
        }
        cx.notify();
    }

    /// Files are being attached to the next message of the place that the composer shows.
    fn attaching_here(&self, pane: Pane) -> bool {
        let pen = self.pen(pane);
        pen.attaching
            .iter()
            .any(|entry| Some(entry.asked) == pen.place)
    }

    /// Attaches the files at these paths, as the file dialog and a drop do.
    pub fn attach_paths(
        &mut self,
        pane: Pane,
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.attach(
            pane,
            paths.into_iter().map(UploadSource::File).collect(),
            cx,
        );
    }

    /// Opens the file dialog for the next message of the place that the composer writes
    /// to. The dialog does not stop the window: the person, and an event of a server, can
    /// change what the composer shows before the dialog is answered.
    fn choose_files(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let Some(asked) = self.pen(pane).place else {
            return;
        };
        let Some(owner) = self.state.read(cx).owner(asked.0) else {
            return;
        };
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await {
                let _ = this.update(cx, |this, cx| {
                    this.attach_chosen(pane, (asked, owner), paths, cx);
                });
            }
        })
        .detach();
    }

    /// Takes the files of a file dialog that was opened for `asked`: a place of a server
    /// and the owner that writes there. They are attached only while the composer still
    /// writes to that place and the server still has that owner (ADR 0746). Otherwise
    /// nothing is uploaded, and the composer says so, because the person chose the files
    /// for another place.
    fn attach_chosen(
        &mut self,
        pane: Pane,
        asked: (Desk, junto_client_core::Owner),
        paths: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let (asked, owner) = asked;
        let same_owner = self.state.read(cx).owner(asked.0) == Some(owner);
        if self.pen(pane).place != Some(asked) || !same_owner {
            self.pen_mut(pane).problem = Some(text::FILES_OF_ANOTHER_PLACE.to_owned());
            cx.notify();
            return;
        }
        self.attach_paths(pane, paths, cx);
    }

    /// Puts marks around the selected text, or around the place of the cursor.
    fn wrap_selection(
        &mut self,
        pane: Pane,
        before: &str,
        after: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (before, after) = (before.to_owned(), after.to_owned());
        self.pen_mut(pane).input.update(cx, |input, cx| {
            let range = input.selected_range();
            let selected = input.value()[range.clone()].to_owned();
            input.replace(format!("{before}{selected}{after}"), window, cx);
            // With nothing selected the cursor stands between the marks.
            let start = range.start + before.len();
            input.set_selected_range(start..start + selected.len(), cx);
            input.focus(window, cx);
        });
        self.composer_changed(pane, cx);
    }

    /// Puts a mark at the start of the line of the cursor, or of each selected line.
    fn prefix_lines(
        &mut self,
        pane: Pane,
        prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prefix = prefix.to_owned();
        self.pen_mut(pane).input.update(cx, |input, cx| {
            let value = input.value().to_string();
            let range = input.selected_range();
            let start = value[..range.start].rfind('\n').map_or(0, |at| at + 1);
            let end = range.end.max(start);
            let lines: Vec<String> = value[start..end]
                .split('\n')
                .map(|line| format!("{prefix}{line}"))
                .collect();
            let made = lines.join("\n");
            input.set_selected_range(start..end, cx);
            input.replace(made.clone(), window, cx);
            let cursor = start + made.len();
            input.set_selected_range(cursor..cursor, cx);
            input.focus(window, cx);
        });
        self.composer_changed(pane, cx);
    }

    /// Starts a reply to a message of the shown place.
    pub fn reply_to_message(
        &mut self,
        pane: Pane,
        message_id: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pen_mut(pane).reply_to = Some(message_id);
        self.pen_mut(pane).editing = None;
        self.pen_mut(pane)
            .input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn held_message(&self, pane: Pane, message_id: MessageId) -> Option<Arc<Message>> {
        self.composer_feed(pane)?
            .rows
            .iter()
            .find_map(|row| match row {
                FeedRow::Message(row) if row.message.id == message_id => Some(row.message.clone()),
                _ => None,
            })
    }

    /// Puts the text of an own message into the composer, to change it.
    pub fn edit_message(
        &mut self,
        pane: Pane,
        message_id: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(message) = self.held_message(pane, message_id) else {
            return;
        };
        let Some(document) = &message.document else {
            return;
        };
        let names = self
            .composer_feed(pane)
            .map(|feed| feed.names.clone())
            .unwrap_or_default();
        let name = |target: MentionTarget| match target {
            MentionTarget::Account(account_id) => names
                .accounts
                .get(&account_id)
                .cloned()
                .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned()),
            MentionTarget::Role(role_id) => names
                .roles
                .get(&role_id)
                .cloned()
                .unwrap_or_else(|| text::UNKNOWN_ROLE.to_owned()),
            MentionTarget::Everyone => text::EVERYONE.to_owned(),
        };
        let shown = compose::written(document, &name);
        let mut content = InputContent::new(shown.text.clone());
        for mention in &shown.mentions {
            let token = InlineToken::new(
                token_id(mention.target),
                shown.text[mention.range.clone()].to_owned(),
            );
            if let Ok(with) = content.clone().with_token(mention.range.clone(), token) {
                content = with;
            }
        }
        self.pen_mut(pane).editing = Some(Editing {
            message_id,
            original: Arc::new((document.clone(), shown)),
        });
        self.pen_mut(pane).reply_to = None;
        self.pen_mut(pane).problem = None;
        self.pen_mut(pane).input.update(cx, |input, cx| {
            input.set_value(content, window, cx);
            let end = input.value().len();
            input.set_selected_range(end..end, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Ends a reply or an edit without sending.
    fn cancel_compose_state(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if self.pen_mut(pane).editing.take().is_some() {
            self.pen_mut(pane)
                .input
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.pen_mut(pane).reply_to = None;
        self.pen_mut(pane).problem = None;
        cx.notify();
    }

    /// Sends what the composer holds: a new message, or the new text of the message that
    /// is edited. A text that the server would refuse is not sent, and the reason is told.
    pub fn submit_message(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if self.pen_mut(pane).mention.is_some() {
            let found = self.mention_candidates(pane);
            if let Some(open) = &self.pen_mut(pane).mention
                && let Some(chosen) = found.get(open.chosen.min(found.len().saturating_sub(1)))
            {
                let chosen = chosen.clone();
                self.take_candidate(pane, &chosen, window, cx);
                return;
            }
        }
        let Some((server_id, place)) = self.composer_place(pane) else {
            return;
        };
        let content = self.pen_mut(pane).input.read(cx).content();
        let editing = self.pen_mut(pane).editing.clone();
        let document = match &editing {
            Some(editing) => {
                let (original, shown) = editing.original.as_ref();
                compose::parse_edit(original, shown, content.text(), &mentions_of(&content))
            }
            None => compose::parse(content.text(), &mentions_of(&content)),
        };
        let editing = editing.map(|editing| editing.message_id);
        if editing.is_none() && self.attaching_here(pane) {
            self.pen_mut(pane).problem = Some(text::FILES_STILL_ATTACHING.to_owned());
            cx.notify();
            return;
        }
        let draft = self
            .view
            .as_ref()
            .and_then(|view| view.workspace())
            .map(|workspace| match (pane, &workspace.thread) {
                (Pane::Thread, Some(thread)) => thread.draft.clone(),
                (Pane::Thread, None) => Vec::new(),
                (Pane::Main, _) => workspace.draft.clone(),
            })
            .unwrap_or_default();
        if draft
            .iter()
            .any(|file| matches!(file.progress.stage, UploadStage::Failed(_)))
        {
            self.pen_mut(pane).problem = Some(text::REMOVE_FAILED_FILE.to_owned());
            cx.notify();
            return;
        }
        let has_files = match editing {
            Some(message_id) => self
                .held_message(pane, message_id)
                .is_some_and(|message| !message.attachments.is_empty()),
            None => !draft.is_empty(),
        };
        if let Err(problem) = compose::check(&document, has_files) {
            self.pen_mut(pane).problem = Some(text::compose_problem(problem));
            cx.notify();
            return;
        }
        let reply_to = self.pen_mut(pane).reply_to;
        let sent = self.state.update(cx, |state, cx| match editing {
            Some(message_id) => {
                state.edit_message(server_id, message_id, document, cx);
                true
            }
            None => state.send_message(server_id, place, document, reply_to, cx),
        });
        if !sent {
            self.pen_mut(pane).problem = Some(text::NOT_SENT.to_owned());
            cx.notify();
            return;
        }
        self.pen_mut(pane).editing = None;
        self.pen_mut(pane).reply_to = None;
        self.pen_mut(pane).problem = None;
        self.pen_mut(pane)
            .input
            .update(cx, |input, cx| input.set_value("", window, cx));
        // The person sent a message: the list shows its end.
        if editing.is_none() {
            self.list(pane).list.set_follow_mode(gpui::FollowMode::Tail);
        }
        cx.notify();
    }

    /// Opens the emoji picker: for a reaction to a message, or for the text of a composer.
    pub fn open_emoji_picker(
        &mut self,
        target: EmojiTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .emoji_find
            .update(cx, |find, cx| find.set_value("", window, cx));
        self.open_layer(Layer::Emoji(target), window, cx);
        self.focus_field(&self.composer.emoji_find.clone(), window, cx);
    }

    fn take_emoji(&mut self, emoji: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let Layer::Emoji(target) = self.layer else {
            return;
        };
        self.close_layer(window, cx);
        let server_id = self
            .view
            .as_ref()
            .and_then(|view| view.selected.as_ref())
            .map(|server| server.id);
        match (target, server_id) {
            (EmojiTarget::React(message_id), Some(server_id)) => {
                self.state.update(cx, |state, cx| {
                    state.react(server_id, message_id, emoji.to_owned(), true, cx);
                });
            }
            (EmojiTarget::React(_), None) => {}
            (EmojiTarget::Compose(pane), _) => {
                self.pen_mut(pane).input.update(cx, |input, cx| {
                    input.insert(emoji, window, cx);
                    input.focus(window, cx);
                });
                self.composer_changed(pane, cx);
            }
        }
    }

    pub(crate) fn render_emoji_picker(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let find = self.composer.emoji_find.read(cx).value().to_string();
        let searching = !find.trim().is_empty();
        let shown = emoji_shown(self.composer.emoji_group, &find);
        let ring_width = theme.metrics.focus_ring;
        let radius = theme.metrics.radius;
        let groups: Vec<_> = emojis::Group::iter()
            .enumerate()
            .map(|(index, group)| {
                let on = index == self.composer.emoji_group && !searching;
                Button::new(("emoji-group", index))
                    .kind(ButtonKind::Ghost)
                    .label(group_name(group))
                    .when(on, |button| button.bg(Hsla::from(colors.limestone_lo)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.composer.emoji_group = index;
                        this.composer.emoji_scroll = ScrollHandle::new();
                        cx.notify();
                    }))
            })
            .collect();
        let cells: Vec<_> = shown
            .iter()
            .enumerate()
            .map(|(index, emoji)| {
                let sign = emoji.as_str();
                div()
                    .id(("emoji", index))
                    .role(Role::Button)
                    .aria_label(SharedString::from(emoji.name()))
                    .debug_selector(move || format!("emoji:{sign}"))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(u(34.0))
                    .rounded(u(radius))
                    .text_size(u(21.0))
                    .line_height(u(26.0))
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .hover(move |style| style.bg(Hsla::from(colors.plaster)))
                    .focus_visible(move |style| {
                        style.shadow(focus_ring_inside(colors.terracotta_deep, ring_width))
                    })
                    .tooltip(tooltip(emoji.name()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.take_emoji(sign, window, cx);
                    }))
                    .child(sign)
            })
            .collect();
        Dialog::new(text::EMOJI, self.layer_focus())
            .selector("emoji-picker")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            .child(
                Field::new(&self.composer.emoji_find, text::FIND_EMOJI)
                    .kind(FieldKind::Find)
                    .selector("find-emoji"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(u(2.0))
                    .mt(u(8.0))
                    .children(groups),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .mt(u(8.0))
                    .h(u(260.0))
                    .child(if shown.is_empty() {
                        div()
                            .typeface(theme.types.small, &theme)
                            .ink(colors.ink_2)
                            .child(text::NO_EMOJI_FOUND)
                            .into_any_element()
                    } else {
                        ScrollArea::new("emoji-list", &self.composer.emoji_scroll, text::EMOJI)
                            .child(div().flex().flex_wrap().gap(u(2.0)).children(cells))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    /// The composer of a pane (view A of the design): of the place of the main area, or
    /// of the thread of the side panel. `draft` holds the files of the next message, and
    /// `permanent` states that a sent message of the place cannot be edited.
    pub(crate) fn render_composer(
        &mut self,
        pane: Pane,
        draft: &[DraftFile],
        compact: bool,
        permanent: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let faces = self.shown_faces();
        let colors = theme.colors;
        let side = if compact { 12.0 } else { 16.0 };
        let ring_width = theme.metrics.focus_ring;
        let radius_small = theme.metrics.radius_small;
        self.pen_mut(pane).input.update(cx, |input, _| {
            input.set_editor_style(InputEditorStyle {
                foreground: colors.ink.into(),
                muted_foreground: colors.ink_3.into(),
                background: colors.travertine_hi.into(),
                border: colors.travertine_hi.into(),
                selection: colors.terracotta_wash.into(),
                caret: colors.ink.into(),
                ..InputEditorStyle::default()
            });
        });
        let tool = |id: &'static str, label: &'static str, mark: AnyElement| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .debug_selector(move || id.to_owned())
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(u(26.0))
                .h(u(22.0))
                .rounded(u(radius_small))
                .text_size(u(14.0))
                .line_height(u(14.0))
                .ink(colors.ink_2)
                .cursor_pointer()
                .focusable()
                .tab_stop(true)
                .hover(move |style| style.bg(Hsla::from(colors.plaster)))
                .focus_visible(move |style| {
                    style.shadow(focus_ring_inside(colors.terracotta_deep, ring_width))
                })
                .tooltip(tooltip(label))
                .child(mark)
        };
        let letter = |letter: &'static str, weight: FontWeight| {
            div().font_weight(weight).child(letter).into_any_element()
        };
        let shown = |shown: Icon| icon(shown, 15.0, colors.ink_2).into_any_element();
        let divider = || {
            div()
                .flex_none()
                .w(px(1.0))
                .h(u(14.0))
                .mx(u(3.0))
                .bg(Hsla::from(colors.rule))
        };
        let tools = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(1.0))
            .h(u(28.0))
            .px(u(4.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule_soft))
            .child(
                tool(
                    "format-bold",
                    text::BOLD,
                    letter("B", FontWeight::EXTRA_BOLD),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.wrap_selection(pane, "**", "**", window, cx)
                })),
            )
            .child(
                tool(
                    "format-italic",
                    text::ITALIC,
                    div()
                        .italic()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("I")
                        .into_any_element(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.wrap_selection(pane, "*", "*", window, cx)
                })),
            )
            .child(
                tool(
                    "format-strike",
                    text::STRIKETHROUGH,
                    div().line_through().child("S").into_any_element(),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.wrap_selection(pane, "~~", "~~", window, cx)
                })),
            )
            .child(divider())
            .child(
                tool("format-code", text::INLINE_CODE, shown(Icon::Code)).on_click(cx.listener(
                    move |this, _, window, cx| this.wrap_selection(pane, "`", "`", window, cx),
                )),
            )
            .child(
                tool(
                    "format-code-block",
                    text::CODE_BLOCK,
                    shown(Icon::CodeBlock),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.wrap_selection(pane, "```\n", "\n```", window, cx);
                })),
            )
            .child(divider())
            .child(
                tool("format-link", text::LINK, shown(Icon::Link)).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.wrap_selection(pane, "[", "](https://)", window, cx);
                    },
                )),
            )
            .child(tool("format-list", text::LIST, shown(Icon::List)).on_click(
                cx.listener(move |this, _, window, cx| this.prefix_lines(pane, "- ", window, cx)),
            ))
            .child(
                tool("format-quote", text::QUOTE, shown(Icon::Quote)).on_click(cx.listener(
                    move |this, _, window, cx| this.prefix_lines(pane, "> ", window, cx),
                )),
            )
            .child(divider())
            .child(
                tool("attach-file", text::ATTACH_FILE, shown(Icon::Clip))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_files(pane, cx))),
            )
            .child(
                tool("insert-emoji", text::EMOJI, shown(Icon::Smile)).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_emoji_picker(EmojiTarget::Compose(pane), window, cx)
                    },
                )),
            )
            .when(!compact, |tools| {
                tools.child(div().flex_1()).child(
                    div()
                        .id("composer-hint")
                        .debug_selector(|| "composer-hint".to_owned())
                        .flex_none()
                        .pr(u(5.0))
                        .text_size(u(12.5))
                        .ink(colors.ink_3)
                        .child(if permanent {
                            text::PERMANENT_COMPOSER
                        } else {
                            text::SEND_HINT
                        }),
                )
            });

        // The state above the box: a reply or an edit, with the control that ends it.
        let banner = match (self.pen(pane).editing.is_some(), self.pen(pane).reply_to) {
            (true, _) => Some((Icon::Edit, SharedString::from(text::EDITING_MESSAGE), None)),
            (false, Some(message_id)) => {
                let quoted = self.held_message(pane, message_id);
                let names = self
                    .pane_feed(pane)
                    .map(|feed| feed.names.clone())
                    .unwrap_or_default();
                let author = quoted
                    .as_ref()
                    .and_then(|message| message.author_name.clone())
                    .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned());
                let excerpt = quoted
                    .as_ref()
                    .and_then(|message| message.document.as_ref())
                    .map(|document| plain_text(document, &names, 80));
                Some((Icon::Reply, text::replying_to(&author).into(), excerpt))
            }
            (false, None) => None,
        }
        .map(|(shown, title, excerpt)| {
            div()
                .id("compose-state")
                .debug_selector(|| "compose-state".to_owned())
                .flex()
                .items_center()
                .gap(u(7.0))
                .mb(u(4.0))
                .text_size(u(12.5))
                .line_height(u(18.0))
                .ink(colors.ink_2)
                .child(icon(shown, 13.0, colors.terracotta_deep))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .ink(colors.ink)
                        .child(title),
                )
                .children(excerpt.map(|excerpt| {
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(excerpt))
                }))
                .child(div().flex_1())
                .child(
                    Button::new("cancel-compose-state")
                        .kind(ButtonKind::Ghost)
                        .icon(Icon::X)
                        .tooltip(text::CANCEL)
                        .selector("cancel-compose-state")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.cancel_compose_state(pane, window, cx);
                        })),
                )
        });

        // The files of the next message, each with the state of its upload.
        let place = self.composer_place(pane);
        let attaching = self.attaching_here(pane);
        let files = (!draft.is_empty() || attaching).then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap(u(6.0))
                .mb(u(5.0))
                .children(draft.iter().enumerate().map(|(index, file)| {
                    let upload = file.upload;
                    let progress = file.progress;
                    let (state, failed): (SharedString, bool) = match progress.stage {
                        UploadStage::Reading => (text::UPLOAD_READING.into(), false),
                        UploadStage::Sending if progress.interrupted => {
                            (text::UPLOAD_INTERRUPTED.into(), false)
                        }
                        UploadStage::Sending => (
                            text::transfer_progress(progress.sent, progress.bytes).into(),
                            false,
                        ),
                        UploadStage::Checking => (text::UPLOAD_CHECKING.into(), false),
                        UploadStage::Complete(_) => (text::file_size(progress.bytes).into(), false),
                        UploadStage::Failed(failure) => {
                            (text::transfer_failure(failure).into(), true)
                        }
                        UploadStage::Cancelled => (text::DISCARD.into(), true),
                    };
                    div()
                        .debug_selector(move || format!("draft-file:{index}"))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(7.0))
                        .max_w(u(340.0))
                        .h(u(30.0))
                        .pl(u(8.0))
                        .pr(u(2.0))
                        .rounded(u(theme.metrics.radius))
                        .border_1()
                        .border_color(Hsla::from(if failed {
                            colors.cinnabar_wash_rule
                        } else {
                            colors.rule
                        }))
                        .bg(Hsla::from(if failed {
                            colors.cinnabar_wash
                        } else {
                            colors.travertine_hi
                        }))
                        .text_size(u(13.0))
                        .child(icon(Icon::Clip, 14.0, colors.ink_2))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::BOLD)
                                .ink(colors.ink)
                                .child(SharedString::from(file.name.clone())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .max_w(u(170.0))
                                .truncate()
                                .text_size(u(12.5))
                                .ink(if failed {
                                    colors.cinnabar_ink
                                } else {
                                    colors.ink_3
                                })
                                .child(state),
                        )
                        .child(
                            Button::new(("remove-file", index))
                                .kind(ButtonKind::Ghost)
                                .icon(Icon::X)
                                .tooltip(text::REMOVE_FILE)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some((server_id, place)) = place {
                                        this.state.update(cx, |state, cx| {
                                            state.remove_attachment(server_id, place, upload, cx);
                                        });
                                    }
                                })),
                        )
                }))
                .children(attaching.then(|| {
                    div()
                        .debug_selector(|| "draft-attaching".to_owned())
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(7.0))
                        .h(u(30.0))
                        .px(u(8.0))
                        .rounded(u(theme.metrics.radius))
                        .border_1()
                        .border_color(Hsla::from(colors.rule))
                        .bg(Hsla::from(colors.travertine_hi))
                        .text_size(u(12.5))
                        .ink(colors.ink_3)
                        .child(icon(Icon::Clip, 14.0, colors.ink_2))
                        .child(text::FILES_BEING_READ)
                }))
        });

        let problem = self
            .pen(pane)
            .problem
            .clone()
            .or_else(|| self.state.read(cx).message_refusal().map(str::to_owned))
            .map(|sentence| {
                div()
                    .id("compose-problem")
                    .debug_selector(|| "compose-problem".to_owned())
                    .mb(u(4.0))
                    .text_size(u(12.5))
                    .line_height(u(18.0))
                    .ink(colors.cinnabar_ink)
                    .child(SharedString::from(sentence))
            });

        // The mention picker, above the box.
        let found = self.mention_candidates(pane);
        let chosen = self
            .pen(pane)
            .mention
            .as_ref()
            .map_or(0, |open| open.chosen.min(found.len().saturating_sub(1)));
        let picker = (self.pen(pane).mention.is_some() && !found.is_empty()).then(|| {
            div()
                .id("mention-picker")
                .role(Role::ListBox)
                .aria_label(text::MENTION_SOMEONE)
                .debug_selector(|| "mention-picker".to_owned())
                .absolute()
                .bottom_full()
                .left_0()
                .mb(u(4.0))
                .w(u(280.0))
                .py(u(4.0))
                .rounded(u(theme.metrics.radius_menu))
                .border_1()
                .border_color(Hsla::from(colors.rule))
                .bg(Hsla::from(colors.travertine_hi))
                .shadow(float_shadow(colors.shadow_deep, 8.0, 22.0))
                .occlude()
                .children(found.iter().enumerate().map(|(index, candidate)| {
                    let taken = candidate.clone();
                    let person = match candidate.target {
                        MentionTarget::Account(account_id) => Some(account_id),
                        _ => None,
                    };
                    div()
                        .id(("candidate", index))
                        .role(Role::ListBoxOption)
                        .aria_label(SharedString::from(candidate.name.clone()))
                        .aria_selected(index == chosen)
                        .debug_selector(move || format!("candidate:{index}"))
                        .flex()
                        .items_center()
                        .gap(u(8.0))
                        .h(u(30.0))
                        .px(u(10.0))
                        .cursor_pointer()
                        .when(index == chosen, |row| {
                            row.bg(Hsla::from(colors.limestone_lo))
                        })
                        .hover(move |style| style.bg(Hsla::from(colors.plaster)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.take_candidate(pane, &taken, window, cx);
                        }))
                        .child(match person {
                            Some(account_id) => super::people::avatar(&faces, account_id, 18.0)
                                .radius(2.0)
                                .into_any_element(),
                            None => icon(Icon::People, 15.0, colors.ink_2).into_any_element(),
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(u(14.0))
                                .font_weight(FontWeight::BOLD)
                                .ink(colors.ink)
                                .child(SharedString::from(candidate.name.clone())),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex_none()
                                .text_size(u(12.5))
                                .ink(colors.ink_3)
                                .child(candidate.detail),
                        )
                }))
        });

        // A side panel has no room for the statement in its toolbar.
        let permanence = (permanent && compact).then(|| {
            div()
                .id("composer-permanent")
                .debug_selector(|| "composer-permanent".to_owned())
                .mb(u(4.0))
                .text_size(u(12.5))
                .line_height(u(17.0))
                .ink(colors.ink_2)
                .child(text::PERMANENT_COMPOSER)
        });
        let editing = self.pen(pane).editing.is_some();
        let token_wash = colors.terracotta_wash;
        let token_ink = colors.terracotta_deep;
        let input = Textarea::new(&self.pen(pane).input).token(move |context, _, _| {
            div()
                .px(u(3.0))
                .rounded(u(2.0))
                .bg(Hsla::from(token_wash))
                .text_color(Hsla::from(token_ink))
                .font_weight(FontWeight::BOLD)
                .child(context.token().label().clone())
        });
        let frame = div()
            .relative()
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
            .shadow(ledge(colors.rule))
            .child(tools)
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(u(8.0))
                    .min_h(u(40.0))
                    .pl(u(11.0))
                    .pr(u(6.0))
                    .py(u(6.0))
                    .child(
                        div()
                            .id(pane.id("composer-input"))
                            .role(Role::TextInput)
                            .aria_label(match pane {
                                Pane::Main => text::MESSAGES,
                                Pane::Thread => text::REPLY_IN_THREAD,
                            })
                            .debug_selector(move || pane.id("composer-input").to_owned())
                            .flex_1()
                            .min_w_0()
                            .text_size(u(14.5))
                            .line_height(u(21.0))
                            .ink(colors.ink)
                            .child(input),
                    )
                    .child(
                        div()
                            .id(pane.id("send-message"))
                            .role(Role::Button)
                            .aria_label(if editing { text::SAVE_EDIT } else { text::SEND })
                            .debug_selector(move || pane.id("send-message").to_owned())
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(u(36.0))
                            .h(u(28.0))
                            .rounded(u(theme.metrics.radius))
                            .bg(Hsla::from(colors.terracotta))
                            .shadow(ledge(colors.terracotta_deep))
                            .cursor_pointer()
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(move |style| {
                                style.shadow(crate::ui::focus_ring(
                                    colors.terracotta_deep,
                                    ring_width,
                                ))
                            })
                            .tooltip(tooltip(if editing { text::SAVE_EDIT } else { text::SEND }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.submit_message(pane, window, cx);
                            }))
                            .child(icon(
                                if editing { Icon::Check } else { Icon::Send },
                                16.0,
                                colors.on_terracotta,
                            )),
                    ),
            )
            .children(picker);

        div()
            .id(pane.id("composer"))
            .debug_selector(move || pane.id("composer").to_owned())
            .flex_none()
            .pt(u(2.0))
            .px(u(side))
            .pb(u(10.0))
            .capture_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if this.composer_key(pane, event, window, cx) {
                    cx.stop_propagation();
                }
            }))
            // The text area binds these keys to its actions, and an action is dispatched
            // before a key: the composer takes the action first.
            .capture_action(cx.listener(move |this, _: &input::MoveUp, window, cx| {
                if this.composer_step(pane, Step::Up, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(move |this, _: &input::MoveDown, window, cx| {
                if this.composer_step(pane, Step::Down, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(
                cx.listener(move |this, _: &input::IndentInline, window, cx| {
                    if this.composer_step(pane, Step::Tab, window, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_action(cx.listener(move |this, _: &input::Escape, window, cx| {
                if this.composer_step(pane, Step::Escape, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(move |this, _: &input::Paste, _, cx| {
                if this.paste_picture(pane, cx) {
                    cx.stop_propagation();
                }
            }))
            // Enter without Shift sends. The text area passes that key on; it ends here, so
            // that the system does not add a line for a key that nobody took.
            .on_action(|action: &input::Enter, _, cx| {
                if action.shift {
                    cx.propagate();
                }
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                this.attach_paths(pane, paths.paths().to_vec(), cx);
            }))
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.bg(Hsla::from(colors.terracotta_wash))
            })
            .children(permanence)
            .children(banner)
            .children(files)
            .child(
                crate::ui::notice_slot(pane.id("compose-problem-slot"))
                    .h(u(24.0))
                    .children(problem),
            )
            .child(frame)
            .into_any_element()
    }

    /// Follows the text area of the composer: a change of the text, and Enter.
    pub(crate) fn observe_composer(
        pane: Pane,
        input: &Entity<TextareaState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Subscription {
        cx.subscribe_in(
            input,
            window,
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.composer_changed(pane, cx),
                InputEvent::PressEnter { shift: false, .. } => {
                    this.submit_message(pane, window, cx)
                }
                InputEvent::PressEnter { .. } | InputEvent::Focus | InputEvent::Blur => {
                    cx.notify();
                }
            },
        )
    }

    /// Moves the keyboard focus into the composer.
    pub fn focus_composer(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.pen_mut(pane)
            .input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Puts text into the field that searches the emoji picker, as typing does.
    pub fn find_emoji(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.composer
            .emoji_find
            .update(cx, |find, cx| find.set_value(text, window, cx));
        cx.notify();
    }

    /// Puts text into the composer, as typing does.
    pub fn type_message(
        &mut self,
        pane: Pane,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_owned();
        self.pen_mut(pane).input.update(cx, |input, cx| {
            let end = text.len();
            input.set_value(text, window, cx);
            input.set_selected_range(end..end, cx);
        });
        self.composer_changed(pane, cx);
    }

    /// The text that the composer holds.
    pub fn composer_text(&self, pane: Pane, cx: &gpui::App) -> String {
        self.pen(pane).input.read(cx).value().to_string()
    }

    /// Why the composer did not send its text.
    pub fn composer_problem(&self, pane: Pane) -> Option<&str> {
        self.pen(pane).problem.as_deref()
    }

    /// The message that the composer answers, and the message that it changes.
    pub fn compose_state(&self, pane: Pane) -> (Option<MessageId>, Option<MessageId>) {
        (
            self.pen(pane).reply_to,
            self.pen(pane)
                .editing
                .as_ref()
                .map(|editing| editing.message_id),
        )
    }

    /// The names of the entries that the mention picker of a pane shows, in their order.
    pub fn mention_names(&self, pane: Pane) -> Vec<String> {
        self.mention_candidates(pane)
            .into_iter()
            .map(|candidate| candidate.name)
            .collect()
    }

    /// Takes the entry of the mention picker with this name.
    pub fn pick_mention(
        &mut self,
        pane: Pane,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let found = self.mention_candidates(pane);
        match found.iter().find(|candidate| candidate.name == name) {
            Some(candidate) => {
                let candidate = candidate.clone();
                self.take_candidate(pane, &candidate, window, cx);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mention_starts_at_an_at_sign_after_white_space() {
        assert_eq!(mention_query("@", 1), Some((0, String::new())));
        assert_eq!(mention_query("hello @br", 9), Some((6, "br".to_owned())));
        assert_eq!(
            mention_query("hello @bruno costa", 18),
            Some((6, "bruno costa".to_owned()))
        );
        // An address is no mention.
        assert_eq!(mention_query("mail ana@example", 16), None);
        assert_eq!(mention_query("no sign", 7), None);
        assert_eq!(mention_query("@one\n two", 9), None);
        // The cursor stands before the sign.
        assert_eq!(mention_query("ab @c", 2), None);
    }

    #[test]
    fn a_token_names_its_target() {
        let account_id = junto_client_core::protocol::ids::AccountId::generate();
        let role_id = junto_client_core::protocol::ids::WorkspaceRoleId::generate();
        for target in [
            MentionTarget::Account(account_id),
            MentionTarget::Role(role_id),
            MentionTarget::Everyone,
        ] {
            assert_eq!(token_target(&token_id(target)), Some(target));
        }
        assert_eq!(token_target("account:nothing"), None);
        assert_eq!(token_target("other"), None);
    }

    #[test]
    fn the_emoji_of_the_picker_are_ones_that_the_server_takes() {
        for (index, _) in emojis::Group::iter().enumerate() {
            let shown = emoji_shown(index, "");
            assert!(!shown.is_empty());
            for emoji in shown {
                assert_eq!(
                    emojis::get(emoji.as_str()).map(|held| held.as_str()),
                    Some(emoji.as_str())
                );
            }
        }
        let found = emoji_shown(0, "thumbs up");
        assert!(found.iter().any(|emoji| emoji.as_str() == "\u{1f44d}"));
        assert!(emoji_shown(0, "no such emoji name").is_empty());
    }
}
