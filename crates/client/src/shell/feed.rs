//! The message list of a place (ADR 0740): the rows that [`crate::state::feed`] derives,
//! drawn in a list that measures a row when it is shown and keeps its position while
//! rows are added before and after it.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Context, FollowMode, FontStyle, FontWeight, HighlightStyle, Hsla,
    InteractiveElement, InteractiveText, IntoElement, ListAlignment, ListOffset, ListScrollEvent,
    ListState, MouseButton, ObjectFit, ParentElement, RenderImage, Role, SharedString,
    StatefulInteractiveElement, StrikethroughStyle, Styled, StyledImage, StyledText,
    UnderlineStyle, Window, div, img, list, prelude::FluentBuilder, px,
};
use gpui_base::{Scrollbar, ScrollbarMode};
use junto_client_core::model::{PlaceId, ReadFailure, SendState};
#[cfg(feature = "test-support")]
use junto_client_core::protocol::ids::AttachmentId;
use junto_client_core::protocol::ids::{MessageId, ServerId};
use junto_client_core::protocol::message::{
    Attachment, Block, Document, Inline, Mark, Message, Reaction,
};
use junto_client_core::transfers::{DownloadStage, UploadStage};

use super::pictures::{MOST_PICTURES, Outcome, Picture, PictureKey, Pictures};
use super::{EmojiTarget, Layer, Pane, Shell};
use crate::assets::Icon;
use crate::compose;
use crate::highlight::{self, Token};
use crate::state::feed::{
    Feed, FeedRow, MessageRow, Names, Quote, QuoteText, SystemKind, SystemRow, WaitingRow,
    can_send_again,
};
use crate::state::people::ServerFaces;
use crate::state::view::{DraftFile, ShellView, WorkspaceView};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Avatar, Button, ButtonKind, Menu, MenuItem, Notice, StatePanel, StyleExt, focus_ring_inside,
    icon, ledge, tooltip,
};

/// The widest picture of a message, and the tallest, in design pixels.
const PICTURE_WIDTH: f32 = 292.0;
const PICTURE_HEIGHT: f32 = 220.0;
/// The rows from an end of the list at which the next page is read.
const PAGE_MARGIN: usize = 4;

/// The list of the place that the window shows.
pub(crate) struct FeedList {
    key: Option<(ServerId, PlaceId)>,
    pub(crate) list: ListState,
    rows: Arc<Vec<FeedRow>>,
    /// The last frame of the window drew the list. The list of a call is not drawn while
    /// its panel shows the members or the settings.
    pub(crate) drawn: bool,
    /// The message that the list went to last, from a notice or a search result.
    target: Option<(MessageId, u64)>,
}

impl FeedList {
    /// The list of a pane. The list of the main area stands at its end; the replies of
    /// a thread start under the root of the thread.
    pub(crate) fn new(pane: Pane) -> Self {
        let alignment = match pane {
            Pane::Main => ListAlignment::Bottom,
            Pane::Thread => ListAlignment::Top,
        };
        Self {
            key: None,
            list: ListState::new(0, alignment, px(800.0)),
            rows: Arc::default(),
            drawn: false,
            target: None,
        }
    }

    /// Goes to the message that the person asked for, once the rows hold it. The list
    /// then stays there and does not follow new messages.
    fn show_target(&mut self, jump: Option<(MessageId, u64)>) {
        let Some((message_id, _)) = jump else {
            self.target = None;
            return;
        };
        // Each jump moves the list once: the person can scroll away afterwards.
        if self.target == jump {
            return;
        }
        let index = self
            .rows
            .iter()
            .position(|row| matches!(row, FeedRow::Message(row) if row.message.id == message_id));
        let Some(index) = index else {
            return;
        };
        self.target = jump;
        self.list.set_follow_mode(FollowMode::Normal);
        self.list.scroll_to(ListOffset {
            item_ix: index.saturating_sub(1),
            offset_in_item: px(0.0),
        });
    }

    /// Takes the rows of the feed that is shown now. The rows that did not change keep
    /// their measured heights, and the list keeps its position.
    fn take(&mut self, feed: Option<&Feed>) {
        let key = feed.map(|feed| (feed.server_id, feed.place));
        let rows = feed.map(|feed| feed.rows.clone()).unwrap_or_default();
        let jump = feed.and_then(|feed| feed.target);
        let first = key != self.key || (self.rows.is_empty() && !rows.is_empty());
        if first {
            self.key = key;
            self.list.reset(rows.len());
            self.list.set_follow_mode(FollowMode::Tail);
            // A place with unread messages opens at the line before the first of them.
            if let Some(line) = rows.iter().position(|row| matches!(row, FeedRow::Unread)) {
                self.list.scroll_to(ListOffset {
                    item_ix: line.saturating_sub(1),
                    offset_in_item: px(0.0),
                });
            }
            self.rows = rows;
            self.show_target(jump);
            return;
        }
        if Arc::ptr_eq(&self.rows, &rows) {
            self.show_target(jump);
            return;
        }
        let (old, new) = (&self.rows, &rows);
        let prefix = old
            .iter()
            .zip(new.iter())
            .take_while(|(one, other)| one == other)
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(one, other)| one == other)
            .count();
        let (old_end, new_end) = (old.len() - suffix, new.len() - suffix);
        if prefix != old_end || prefix != new_end {
            self.list.splice(prefix..old_end, new_end - prefix);
        }
        self.rows = rows;
        self.show_target(jump);
    }

    /// True while the list shows the end of its rows and follows new ones.
    pub(crate) fn at_end(&self) -> bool {
        self.list.is_following_tail()
    }
}

/// The pictures of messages that the window holds decoded.
#[derive(Default)]
pub(crate) struct MessagePictures {
    held: Pictures<Arc<RenderImage>>,
    /// The pictures that the drawing which runs asked for and that are not held.
    waiting: Vec<(PictureKey, Attachment)>,
    /// The step that starts the reads of the waiting pictures is on its way.
    starting: bool,
    /// The pictures that the screen tool draws for files of its own, which no server has.
    #[cfg(feature = "test-support")]
    shown: HashMap<AttachmentId, Picture<Arc<RenderImage>>>,
}

impl MessagePictures {
    /// A drawing of the window starts (see [`Pictures::drawing_starts`]).
    pub(crate) fn drawing_starts(&mut self) {
        self.held.drawing_starts();
    }
}

/// What a pane draws: its feed, the files of its next message, and what it says of a
/// place that the server does not give and of a place without a message.
pub(crate) struct PaneShown<'a> {
    pub feed: Option<&'a Feed>,
    pub draft: &'a [DraftFile],
    /// The pane stands in a side panel.
    pub compact: bool,
    /// A sent message of the place cannot be edited (ADR 0402).
    pub permanent: bool,
    pub gone: &'static str,
    pub empty: &'static str,
}

/// How a message is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageShape {
    /// In the list of the main area.
    Wide,
    /// In a list of a side panel.
    Compact,
    /// In a side panel, without its actions, its menu, its pin mark and the mark of its
    /// thread: the root of the thread of the side panel, and a message of the pin list.
    Bare,
}

/// The message whose menu is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MessageMenuOpen {
    pub message_id: MessageId,
    /// The list that the message was drawn in.
    pub pane: Pane,
}

/// True when the server marked the file as a picture that a client can show (ADR 0408).
fn is_picture(attachment: &Attachment) -> bool {
    matches!(
        attachment.media_type.as_str(),
        "image/png" | "image/jpeg" | "image/webp"
    ) && attachment
        .width
        .is_some_and(|width| (1..=1024).contains(&width))
        && attachment
            .height
            .is_some_and(|height| (1..=1024).contains(&height))
}

/// Decodes a picture that another account sent. The decoder refuses a picture that is
/// larger than the server marks, whatever its header states.
fn decode(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(1024);
    limits.max_image_height = Some(1024);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let mut picture = reader.decode().ok()?.into_rgba8();
    // The window takes the colours in the order blue, green, red.
    for pixel in picture.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(picture)])))
}

/// The size of a picture in the list: its own size, reduced to the bounds of the list
/// with its proportions kept.
fn picture_size(attachment: &Attachment) -> (f32, f32) {
    let width = attachment.width.unwrap_or(1).max(1) as f32;
    let height = attachment.height.unwrap_or(1).max(1) as f32;
    let scale = (PICTURE_WIDTH / width)
        .min(PICTURE_HEIGHT / height)
        .min(1.0);
    ((width * scale).max(24.0), (height * scale).max(24.0))
}

/// The letters on the mark of a file: its extension, when it has a short one.
fn extension(name: &str) -> SharedString {
    name.rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|extension| {
            (1..=4).contains(&extension.len()) && extension.chars().all(char::is_alphanumeric)
        })
        .map_or_else(
            || "FILE".into(),
            |extension| extension.to_uppercase().into(),
        )
}

/// The text of inline content with the styles of its parts.
struct Rich {
    text: String,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    families: Vec<(Range<usize>, SharedString)>,
    /// The parts that open an address, with the address.
    links: Vec<(Range<usize>, String)>,
}

impl Rich {
    fn of(content: &[Inline], names: &Names, theme: &Theme) -> Self {
        let colors = theme.colors;
        let mut rich = Self {
            text: String::new(),
            highlights: Vec::new(),
            families: Vec::new(),
            links: Vec::new(),
        };
        for node in content {
            let start = rich.text.len();
            match node {
                Inline::Text { text, marks } => {
                    rich.text.push_str(text);
                    if !marks.is_empty() {
                        let mut style = HighlightStyle::default();
                        for mark in marks {
                            match mark {
                                Mark::Bold => style.font_weight = Some(FontWeight::BOLD),
                                Mark::Italic => style.font_style = Some(FontStyle::Italic),
                                Mark::Strikethrough => {
                                    style.strikethrough = Some(StrikethroughStyle {
                                        thickness: px(1.0),
                                        color: None,
                                    });
                                }
                            }
                        }
                        rich.highlights.push((start..rich.text.len(), style));
                    }
                }
                Inline::Code { text } => {
                    // A thin space at each end stands for the padding of the design.
                    rich.text.push('\u{2009}');
                    rich.text.push_str(text);
                    rich.text.push('\u{2009}');
                    let range = start..rich.text.len();
                    rich.highlights.push((
                        range.clone(),
                        HighlightStyle {
                            color: Some(colors.terracotta_deep.into()),
                            background_color: Some(colors.code_wash.into()),
                            font_weight: Some(FontWeight::MEDIUM),
                            ..HighlightStyle::default()
                        },
                    ));
                    rich.families.push((range, theme.fonts.mono.clone()));
                }
                // The document came from a server, and a server is not trusted with what
                // this device opens (ADR 0714): an address that the composer would not
                // take as a link is drawn as text.
                Inline::Link { text, url } if !compose::link_is_accepted(url) => {
                    rich.text.push_str(text);
                }
                Inline::Link { text, url } => {
                    rich.text.push_str(text);
                    let range = start..rich.text.len();
                    rich.highlights.push((
                        range.clone(),
                        HighlightStyle {
                            color: Some(colors.cypress.into()),
                            underline: Some(UnderlineStyle {
                                thickness: px(1.0),
                                color: None,
                                wavy: false,
                            }),
                            ..HighlightStyle::default()
                        },
                    ));
                    rich.links.push((range, url.clone()));
                }
                Inline::Account { .. } | Inline::Role { .. } | Inline::Everyone => {
                    rich.text.push('\u{2009}');
                    match node {
                        Inline::Account { account_id } => {
                            rich.text.push('@');
                            rich.text.push_str(
                                names
                                    .accounts
                                    .get(account_id)
                                    .map_or(text::UNKNOWN_PERSON, String::as_str),
                            );
                        }
                        Inline::Role { role_id } => {
                            rich.text.push('@');
                            rich.text.push_str(
                                names
                                    .roles
                                    .get(role_id)
                                    .map_or(text::UNKNOWN_ROLE, String::as_str),
                            );
                        }
                        _ => rich.text.push_str(text::MENTION_EVERYONE),
                    }
                    rich.text.push('\u{2009}');
                    rich.highlights.push((
                        start..rich.text.len(),
                        HighlightStyle {
                            color: Some(colors.terracotta_deep.into()),
                            background_color: Some(colors.terracotta_wash.into()),
                            font_weight: Some(FontWeight::BOLD),
                            ..HighlightStyle::default()
                        },
                    ));
                }
            }
        }
        rich
    }

    /// The element of the text. `id` is the name of the element among the texts of its
    /// row.
    fn element(self, id: usize) -> AnyElement {
        let links = self.links;
        let styled = StyledText::new(self.text)
            .with_highlights(self.highlights)
            .with_font_family_overrides(self.families);
        if links.is_empty() {
            return styled.into_any_element();
        }
        let ranges: Vec<Range<usize>> = links.iter().map(|(range, _)| range.clone()).collect();
        let hover = links.clone();
        InteractiveText::new(("rich", id), styled)
            .on_click(ranges, move |index, _, cx| {
                // The address is HTTP, HTTPS or mailto: the application of the system
                // for it opens it.
                if let Some((_, address)) = links.get(index) {
                    cx.open_url(address);
                }
            })
            .tooltip(move |index, window, cx| {
                hover
                    .iter()
                    .find(|(range, _)| range.contains(&index))
                    .map(|(_, address)| tooltip(address.clone())(window, cx))
            })
            .into_any_element()
    }
}

/// One block of a document in the list.
fn paragraph(content: &[Inline], names: &Names, theme: &Theme, id: usize) -> AnyElement {
    div()
        .min_w_0()
        .child(Rich::of(content, names, theme).element(id))
        .into_any_element()
}

/// How many code blocks the window holds with their tokens.
const MOST_CODE_BLOCKS: usize = 256;

/// The tokens of one code block.
type Tokens = Arc<Vec<(Range<usize>, Token)>>;

/// The tokens of the code blocks that the window drew, by the text and the language of
/// each block. A list draws its rows many times, and the tokens of a text do not change.
#[derive(Default)]
pub(crate) struct CodeTokens {
    /// By the rules of the language, then by the text.
    held: HashMap<&'static str, HashMap<String, Tokens>>,
    blocks: usize,
}

impl CodeTokens {
    fn of(&mut self, code: &str, language: Option<&str>) -> Tokens {
        // A block without a language that has rules, and a block above the bound of
        // the highlighter, has no tokens and is not held.
        let Some(rules) = language.and_then(highlight::rules_of) else {
            return Tokens::default();
        };
        if code.len() > highlight::MOST_BYTES {
            return Tokens::default();
        }
        if let Some(held) = self.held.get(rules).and_then(|held| held.get(code)) {
            return held.clone();
        }
        if self.blocks >= MOST_CODE_BLOCKS {
            self.held.clear();
            self.blocks = 0;
        }
        let tokens = Arc::new(highlight::tokens(code, language));
        self.held
            .entry(rules)
            .or_default()
            .insert(code.to_owned(), tokens.clone());
        self.blocks += 1;
        tokens
    }
}

fn code_block(
    code: &str,
    language: Option<&str>,
    tokens: &[(Range<usize>, Token)],
    theme: &Theme,
) -> AnyElement {
    let colors = theme.colors;
    let colour = |token: Token| match token {
        Token::Keyword => colors.code_keyword,
        Token::Type => colors.code_type,
        Token::Function => colors.code_function,
        Token::Value => colors.code_value,
        Token::Comment => colors.code_comment,
    };
    let highlights: Vec<(Range<usize>, HighlightStyle)> = tokens
        .iter()
        .map(|(range, token)| {
            (
                range.clone(),
                HighlightStyle {
                    color: Some(colour(*token).into()),
                    ..HighlightStyle::default()
                },
            )
        })
        .collect();
    div()
        .relative()
        .flex_none()
        .mt(u(4.0))
        .mb(u(3.0))
        .max_w(u(540.0))
        .pt(u(7.0))
        .pb(u(8.0))
        .px(u(12.0))
        .rounded(u(theme.metrics.radius))
        .bg(Hsla::from(colors.basalt))
        .border_l_3()
        .border_color(Hsla::from(colors.ochre))
        .overflow_hidden()
        .font_family(theme.fonts.mono.clone())
        .font_weight(FontWeight::MEDIUM)
        .text_size(u(13.0))
        .line_height(u(19.0))
        .ink(colors.code_text)
        .child(StyledText::new(code.to_owned()).with_highlights(highlights))
        .when_some(language, |block, language| {
            block.child(
                div()
                    .absolute()
                    .top(u(6.0))
                    .right(u(9.0))
                    .text_size(u(12.0))
                    .ink(colors.bone_3)
                    .child(SharedString::from(language.to_owned())),
            )
        })
        .into_any_element()
}

impl Shell {
    /// The feed that a pane shows: the one of the main area, or the replies of the thread
    /// of the side panel.
    pub(crate) fn pane_feed(&self, pane: Pane) -> Option<&Feed> {
        let workspace = self.view.as_ref()?.workspace()?;
        match pane {
            Pane::Main => workspace.feed.as_ref(),
            Pane::Thread => workspace.thread.as_ref()?.feed.as_ref(),
        }
    }

    /// Takes a new view: each list gets the rows of its feed, and the read position
    /// follows what is shown.
    pub(crate) fn feed_changed(
        &mut self,
        view: &ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = view.workspace();
        let main = workspace.and_then(|workspace| workspace.feed.as_ref());
        let thread = workspace
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.feed.as_ref());
        self.feed.take(main);
        self.thread_feed.take(thread);
        if thread.is_none() {
            self.thread_feed.drawn = false;
        }
        self.composer_follows(view, window, cx);
        self.keep_pictures_of_present_owners(cx);
        self.feed_seen(Pane::Main, main, window, cx);
        self.feed_seen(Pane::Thread, thread, window, cx);
    }

    /// The person sees the end of a list in an active window: the read position moves
    /// there (ADR 0740).
    fn feed_seen(
        &mut self,
        pane: Pane,
        feed: Option<&Feed>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(feed) = feed else {
            return;
        };
        let list = self.list(pane);
        if !window.is_window_active() || !list.drawn || !list.at_end() {
            return;
        }
        if let Some(position) = feed.end_position.filter(|_| feed.unread > 0) {
            let (server_id, place) = (feed.server_id, feed.place);
            self.state
                .update(cx, |state, _| state.mark_read(server_id, place, position));
        }
    }

    /// A list was scrolled: the next page is read near an end, the client core is told
    /// where the list stands, and the read position follows the end.
    pub(crate) fn feed_scrolled(
        &mut self,
        pane: Pane,
        event: &ListScrollEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(feed) = self.pane_feed(pane).cloned() else {
            return;
        };
        let (server_id, place) = (feed.server_id, feed.place);
        let at_end = event.is_following_tail && !feed.more_after;
        let first_position = feed
            .rows
            .get(event.visible_range.start..)
            .and_then(|rows| rows.iter().find_map(FeedRow::position));
        let near_start = event.visible_range.start <= PAGE_MARGIN;
        let near_end = event.visible_range.end + PAGE_MARGIN >= event.count;
        self.state.update(cx, |state, cx| {
            if near_start && feed.more_before && !feed.reading.older {
                state.read_older(server_id, place);
            }
            if near_end && feed.more_after && !feed.reading.newer {
                state.read_newer(server_id, place);
            }
            state.set_anchor(server_id, place, if at_end { None } else { first_position });
            // The cards of the recordings around the new place are read.
            if pane == Pane::Main {
                state.follow_shown_recordings(cx);
            }
        });
        self.feed_seen(pane, Some(&feed), window, cx);
        // The rows moved under a pointer that did not move; gpui tells a row that the
        // pointer is over it at the next move of the pointer.
        if self
            .hovered_message
            .is_some_and(|(hovered, _)| hovered == pane)
        {
            self.hovered_message = None;
        }
        cx.notify();
    }

    /// Moves the list of a pane to one of its rows. The tool that measures the time of a
    /// frame uses it.
    #[cfg(feature = "test-support")]
    pub fn show_feed_row(&mut self, pane: Pane, row: usize, cx: &mut Context<Self>) {
        let list = &self.list(pane).list;
        list.set_follow_mode(FollowMode::Normal);
        list.scroll_to(ListOffset {
            item_ix: row,
            offset_in_item: px(0.0),
        });
        cx.notify();
    }

    /// Shows the end of the history of a pane: the list goes there, and the latest page
    /// replaces what is loaded when the list does not reach the end.
    pub fn go_to_end(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let Some(feed) = self.pane_feed(pane).cloned() else {
            return;
        };
        if feed.more_after {
            let (server_id, place) = (feed.server_id, feed.place);
            self.state
                .update(cx, |state, _| state.read_latest(server_id, place));
        }
        self.list(pane).list.set_follow_mode(FollowMode::Tail);
        self.feed_seen(pane, Some(&feed), window, cx);
        cx.notify();
    }

    /// Moves the read position of the place of the main area to its latest message.
    pub fn mark_shown_as_read(&mut self, cx: &mut Context<Self>) {
        let Some(feed) = self.pane_feed(Pane::Main).cloned() else {
            return;
        };
        let (server_id, place) = (feed.server_id, feed.place);
        self.state.update(cx, |state, _| {
            state.mark_place_as_read(server_id, place);
        });
        cx.notify();
    }

    /// The message area of the place of the main area: its list and its composer.
    /// `compact` is the chat of a call in its panel.
    pub(crate) fn render_feed(
        &mut self,
        workspace: &WorkspaceView,
        compact: bool,
        notices: Vec<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let permanent = workspace
            .channel
            .as_ref()
            .is_some_and(|channel| channel.permanent);
        let gone = match &workspace.conversation {
            Some(_) => text::CONVERSATION_CLOSED,
            None => text::MESSAGES_GONE,
        };
        let empty = match &workspace.conversation {
            Some(_) => text::NO_DIRECT_MESSAGES,
            None => text::NO_MESSAGES,
        };
        let shown = PaneShown {
            feed: workspace.feed.as_ref(),
            draft: &workspace.draft,
            compact,
            permanent,
            gone,
            empty,
        };
        self.render_pane(Pane::Main, &shown, notices, window, cx)
    }

    /// The message area of a pane: its list with the states of a list that has no rows,
    /// and the composer under it.
    pub(crate) fn render_pane(
        &mut self,
        pane: Pane,
        shown: &PaneShown<'_>,
        mut notices: Vec<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let compact = shown.compact;
        self.list_mut(pane).drawn = true;
        // The list can reach its end without a scroll of the person: all of it fits, or a
        // new message arrived at the end. What is shown in an active window is seen.
        let seen = shown.feed.is_some_and(|feed| {
            feed.unread > 0 && feed.end_position.is_some() && self.list(pane).at_end()
        });
        if seen && window.is_window_active() {
            cx.defer_in(window, move |this, window, cx| {
                let feed = this.pane_feed(pane).cloned();
                this.feed_seen(pane, feed.as_ref(), window, cx);
            });
        }
        let body: AnyElement = match shown.feed {
            None => self.feed_state(
                pane,
                text::MESSAGES,
                text::MESSAGES_LOADING,
                None,
                std::mem::take(&mut notices),
                cx,
            ),
            Some(feed) if !feed.loaded => match feed.failed {
                Some(failure) => {
                    let sentence = match failure {
                        ReadFailure::NotFound => shown.gone,
                        ReadFailure::Refused(_) | ReadFailure::NoAnswer => text::MESSAGES_FAILED,
                    };
                    let state = self.feed_state(
                        pane,
                        text::MESSAGES_FAILED_TITLE,
                        sentence,
                        Some((feed.server_id, feed.place)),
                        std::mem::take(&mut notices),
                        cx,
                    );
                    // The messages that wait keep their rows under the statement.
                    if feed.has_waiting() {
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .child(state)
                            .child(self.feed_list(feed, compact, &theme, cx))
                            .into_any_element()
                    } else {
                        state
                    }
                }
                None => self.feed_state(
                    pane,
                    text::MESSAGES,
                    text::MESSAGES_LOADING,
                    None,
                    std::mem::take(&mut notices),
                    cx,
                ),
            },
            Some(feed) => {
                let rows = feed.rows.clone();
                let empty = !rows.iter().any(|row| {
                    matches!(
                        row,
                        FeedRow::Message(_)
                            | FeedRow::System(_)
                            | FeedRow::Recording(_)
                            | FeedRow::Waiting(_)
                    )
                });
                if empty && !feed.more_before && !feed.more_after {
                    match pane {
                        Pane::Main => StatePanel::new(text::NO_MESSAGES_TITLE, shown.empty)
                            .selector(pane.id("no-messages"))
                            .after_actions(std::mem::take(&mut notices))
                            .into_any_element(),
                        // A thread without replies shows its root above this line.
                        Pane::Thread => div()
                            .id(pane.id("no-messages"))
                            .debug_selector(move || pane.id("no-messages").to_owned())
                            .flex_1()
                            .px(u(14.0))
                            .pt(u(10.0))
                            .typeface(theme.types.small, &theme)
                            .ink(colors.ink_2)
                            .child(shown.empty)
                            .into_any_element(),
                    }
                } else {
                    self.feed_list(feed, compact, &theme, cx)
                }
            }
        };
        let stale = shown
            .feed
            .filter(|feed| feed.loaded && (feed.stale || feed.failed.is_some()))
            .map(|feed| {
                let (server_id, place) = (feed.server_id, feed.place);
                div().flex_none().px(u(16.0)).pt(u(8.0)).child(
                    Notice::new(text::MESSAGES_STALE)
                        .icon(Icon::Warning)
                        .selector("messages-stale")
                        .action(
                            Button::new("read-messages-again")
                                .label(text::READ_AGAIN)
                                .selector("read-messages-again")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state.update(cx, |state, _| {
                                        state.read_latest(server_id, place);
                                    });
                                })),
                        ),
                )
            });
        div()
            .id(pane.id("feed"))
            .debug_selector(move || pane.id("feed").to_owned())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(Hsla::from(if compact {
                colors.plaster
            } else {
                colors.travertine
            }))
            .child(
                crate::ui::notice_slot(pane.id("message-notice-area"))
                    .children(notices)
                    .children(stale),
            )
            .child(body)
            .child(match self.shown_archived() {
                // An archived channel takes no message (ADR 0410).
                Some(archived) => self.render_archived_bar(pane, archived, cx),
                None => self.render_composer(pane, shown.draft, compact, shown.permanent, cx),
            })
            .into_any_element()
    }

    fn feed_state(
        &mut self,
        pane: Pane,
        title: &'static str,
        sentence: &'static str,
        retry: Option<(ServerId, PlaceId)>,
        notices: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel = StatePanel::new(title, sentence)
            .selector(pane.id("messages-state"))
            .after_actions(notices);
        match retry {
            Some((server_id, place)) => panel
                .action(
                    Button::new("read-messages")
                        .label(text::TRY_AGAIN)
                        .selector("read-messages")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, _| {
                                state.read_latest(server_id, place);
                            });
                        })),
                )
                .into_any_element(),
            None => panel.into_any_element(),
        }
    }

    pub(crate) fn feed_list(
        &mut self,
        feed: &Feed,
        compact: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let pane = Pane::of(feed.place);
        let state = self.list(pane).list.clone();
        let rows = feed.rows.clone();
        let feed_of_rows = feed.clone();
        let render =
            cx.processor(
                move |this: &mut Self, index: usize, window, cx| match rows.get(index) {
                    Some(row) => {
                        this.render_feed_row(index, row, &feed_of_rows, compact, window, cx)
                    }
                    None => div().into_any_element(),
                },
            );
        let away = !self.list(pane).at_end() || feed.more_after;
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id(pane.id("message-list"))
                    .role(Role::List)
                    .aria_label(match pane {
                        Pane::Main => text::MESSAGES,
                        Pane::Thread => text::THREAD,
                    })
                    .debug_selector(move || pane.id("message-list").to_owned())
                    .size_full()
                    .child(list(state.clone(), render).size_full()),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&state)
                        .viewport_from_layout()
                        .mode(ScrollbarMode::Always)
                        .styles(|styles| {
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
            .when(away, |frame| {
                frame.child(
                    div().absolute().right(u(18.0)).bottom(u(8.0)).child(
                        Button::new("go-to-end")
                            .icon(Icon::ChevronDown)
                            .label(if feed.unread > 0 {
                                SharedString::from(text::unread_tooltip(feed.unread, 0))
                            } else {
                                text::GO_TO_END.into()
                            })
                            .tooltip(text::GO_TO_END)
                            .selector(pane.id("go-to-end"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.go_to_end(pane, window, cx);
                            })),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_feed_row(
        &mut self,
        index: usize,
        row: &FeedRow,
        feed: &Feed,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let side = if compact { 14.0 } else { 16.0 };
        let note = |sentence: &'static str, selector: &'static str| {
            div()
                .id(selector)
                .debug_selector(move || selector.to_owned())
                .flex()
                .w_full()
                .justify_center()
                .py(u(8.0))
                .px(u(side))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_3)
                .child(sentence)
                .into_any_element()
        };
        match row {
            FeedRow::Start => note(text::HISTORY_START, "history-start"),
            FeedRow::Earlier { reading } => note(
                if *reading {
                    text::READING_EARLIER
                } else {
                    text::EARLIER_MESSAGES
                },
                "earlier-messages",
            ),
            FeedRow::Later { reading } => note(
                if *reading {
                    text::READING_LATER
                } else {
                    text::LATER_MESSAGES
                },
                "later-messages",
            ),
            FeedRow::Day(date) => {
                let today = feed.today;
                let line = || div().flex_1().h(px(1.0)).bg(Hsla::from(colors.rule_soft));
                div()
                    .flex()
                    .items_center()
                    .gap(u(10.0))
                    .pt(u(5.0))
                    .pb(u(4.0))
                    .px(u(side))
                    .child(line())
                    .child(day_marks(&theme))
                    .child(
                        div()
                            .flex_none()
                            .text_size(u(12.5))
                            .line_height(u(18.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .ink(colors.ink_2)
                            .child(text::day(*date, today)),
                    )
                    .child(day_marks(&theme))
                    .child(line())
                    .into_any_element()
            }
            FeedRow::Unread => div()
                .id("unread-line")
                .debug_selector(|| "unread-line".to_owned())
                .flex()
                .items_center()
                .gap(u(8.0))
                .py(u(3.0))
                .px(u(side))
                .child(div().flex_1().h(px(1.0)).bg(Hsla::from(colors.terracotta)))
                .child(
                    div()
                        .flex_none()
                        .typeface(theme.types.small_strong, &theme)
                        .ink(colors.terracotta_deep)
                        .child(text::NEW_MESSAGES),
                )
                .into_any_element(),
            FeedRow::System(row) => system_line(row, &theme),
            FeedRow::Recording(row) => self.render_recording_card(index, row, cx),
            FeedRow::Message(row) => {
                let shape = if compact {
                    MessageShape::Compact
                } else {
                    MessageShape::Wide
                };
                self.render_message(index, row, feed, shape, window, cx)
            }
            FeedRow::Waiting(row) => self.render_waiting(index, row, feed, compact, cx),
        }
    }

    /// The line above a reply: the small avatar, the name and the start of the message
    /// that the reply answers.
    fn reply_line(quote: &Quote, faces: &ServerFaces, theme: &Theme) -> AnyElement {
        let colors = theme.colors;
        let (sentence, known): (SharedString, bool) = match &quote.text {
            QuoteText::Text(text) => (text.clone().into(), true),
            QuoteText::File(name) => (name.clone().into(), true),
            QuoteText::Deleted => (text::QUOTE_DELETED.into(), false),
            QuoteText::Unknown => (text::QUOTE_UNKNOWN.into(), false),
        };
        div()
            .relative()
            .flex()
            .items_center()
            .gap(u(6.0))
            .ml(u(44.0))
            .min_w_0()
            .text_size(u(12.5))
            .line_height(u(18.0))
            .ink(colors.ink_2)
            // The line that joins the quoted message to the avatar of the reply.
            .child(
                div()
                    .absolute()
                    .left(u(-28.0))
                    .top(u(9.0))
                    .w(u(22.0))
                    .h(u(9.0))
                    .border_l_2()
                    .border_t_2()
                    .rounded_tl(u(4.0))
                    .border_color(Hsla::from(colors.rule)),
            )
            .when_some(quote.author_id, |line, author_id| {
                line.child(super::people::avatar(faces, author_id, 14.0).radius(2.0))
            })
            .when_some(quote.author.clone(), |line, author| {
                line.child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::BOLD)
                        .ink(colors.ink)
                        .child(author),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .when(!known, |sentence| sentence.ink(colors.ink_3))
                    .child(sentence),
            )
            .into_any_element()
    }

    /// The blocks of a document. A check box of a message of the person takes a click.
    fn document(
        &mut self,
        document: &Document,
        message: Option<(&Feed, &Arc<Message>, bool)>,
        names: &Names,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let mut blocks = Vec::new();
        let mut texts = 0;
        let mut next = || {
            texts += 1;
            texts
        };
        for (block_index, block) in document.blocks.iter().enumerate() {
            match block {
                Block::Paragraph { content } => {
                    blocks.push(paragraph(content, names, theme, next()));
                }
                Block::Quote { content } => blocks.push(
                    div()
                        .my(u(2.0))
                        .pl(u(10.0))
                        .border_l_3()
                        .border_color(Hsla::from(colors.rule))
                        .ink(colors.ink_2)
                        .child(paragraph(content, names, theme, next()))
                        .into_any_element(),
                ),
                Block::Code { text, language } => {
                    let tokens = self.code_tokens.of(text, language.as_deref());
                    blocks.push(code_block(text, language.as_deref(), &tokens, theme));
                }
                Block::List { ordered, items } => {
                    for (number, item) in items.iter().enumerate() {
                        let mark: SharedString = if *ordered {
                            format!("{}.", number + 1).into()
                        } else {
                            "•".into()
                        };
                        blocks.push(
                            div()
                                .flex()
                                .gap(u(8.0))
                                .pl(u(4.0))
                                .child(
                                    div()
                                        .flex_none()
                                        .min_w(u(14.0))
                                        .ink(colors.ink_2)
                                        .child(mark),
                                )
                                .child(paragraph(item, names, theme, next()))
                                .into_any_element(),
                        );
                    }
                }
                Block::Checklist { items } => {
                    for (item_index, item) in items.iter().enumerate() {
                        let checked = item.checked;
                        // Ticking a box is an edit of the message: only its author can
                        // do it (ADR 0401).
                        let own = message
                            .filter(|(_, _, own)| *own)
                            .map(|(feed, held, _)| (feed.server_id, held.clone()));
                        let box_id = block_index * 1000 + item_index;
                        let mut check = div()
                            .id(("check", box_id))
                            .role(Role::CheckBox)
                            .aria_label(SharedString::from(crate::state::feed::plain_text(
                                &Document {
                                    blocks: vec![Block::Paragraph {
                                        content: item.content.clone(),
                                    }],
                                },
                                names,
                                80,
                            )))
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .mt(u(3.0))
                            .size(u(14.0))
                            .rounded(u(theme.metrics.radius_small))
                            .border_1()
                            .border_color(Hsla::from(if checked {
                                colors.terracotta_deep
                            } else {
                                colors.ink_3
                            }))
                            .bg(Hsla::from(if checked {
                                colors.terracotta
                            } else {
                                colors.travertine_hi
                            }))
                            .when(checked, |mark| {
                                mark.child(icon(Icon::Check, 10.0, colors.on_terracotta))
                            });
                        if let Some((server_id, held)) = own {
                            check = check.cursor_pointer().on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.set_check(
                                            server_id,
                                            &held,
                                            block_index,
                                            item_index,
                                            !checked,
                                            cx,
                                        );
                                    });
                                },
                            ));
                        }
                        blocks.push(
                            div()
                                .flex()
                                .gap(u(8.0))
                                .py(u(1.0))
                                .when(checked, |row| row.ink(colors.ink_3))
                                .child(check)
                                .child(paragraph(&item.content, names, theme, next()))
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        blocks
    }

    /// The reactions of a message, and the control that adds one.
    fn reactions(
        &mut self,
        feed: &Feed,
        message: &Message,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if message.reactions.is_empty() {
            return None;
        }
        let colors = theme.colors;
        let (server_id, message_id) = (feed.server_id, message.id);
        let chips = message.reactions.iter().enumerate().map(|(index, held)| {
            let Reaction { emoji, count, own } = held.clone();
            let label: SharedString = text::reaction_label(&emoji, count, own).into();
            let sent = emoji.clone();
            let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
            div()
                .id(("reaction", index))
                .role(Role::Button)
                .aria_label(label.clone())
                .debug_selector(move || format!("reaction:{message_id}:{index}"))
                .flex()
                .flex_none()
                .items_center()
                .gap(u(5.0))
                .h(u(23.0))
                .px(u(7.0))
                .rounded(u(theme.metrics.radius))
                .border_1()
                .border_color(Hsla::from(if own {
                    colors.terracotta_wash_rule
                } else {
                    colors.rule
                }))
                .bg(Hsla::from(if own {
                    colors.terracotta_wash
                } else {
                    colors.travertine_hi
                }))
                .text_size(u(13.0))
                .line_height(u(17.0))
                .cursor_pointer()
                .focusable()
                .tab_stop(true)
                .focus_visible(move |style| style.shadow(ring))
                .tooltip(tooltip(label.clone()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let emoji = sent.clone();
                    this.state.update(cx, |state, cx| {
                        state.react(server_id, message_id, emoji, !own, cx);
                    });
                }))
                .child(SharedString::from(emoji))
                .child(
                    div()
                        .text_size(u(12.5))
                        .font_weight(FontWeight::BOLD)
                        .ink(if own {
                            colors.terracotta_deep
                        } else {
                            colors.ink_2
                        })
                        .child(SharedString::from(count.to_string())),
                )
        });
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap(u(5.0))
                .mt(u(4.0))
                .children(chips)
                .child(
                    div()
                        .id("add-reaction")
                        .role(Role::Button)
                        .aria_label(text::REACT)
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(u(23.0))
                        .px(u(5.0))
                        .rounded(u(theme.metrics.radius))
                        .border_1()
                        .border_color(Hsla::from(colors.rule))
                        .bg(Hsla::from(colors.travertine_hi))
                        .cursor_pointer()
                        .tooltip(tooltip(text::REACT))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_emoji_picker(EmojiTarget::React(message_id), window, cx);
                        }))
                        .child(icon(Icon::Smile, 14.0, colors.ink_2)),
                )
                .into_any_element(),
        )
    }

    /// Draws these bytes as the picture of a file of a message. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_picture(&mut self, attachment_id: AttachmentId, bytes: &[u8]) {
        let picture = decode(bytes).map_or(Picture::Failed, Picture::Ready);
        self.message_pictures.shown.insert(attachment_id, picture);
    }

    /// The owners for which the window holds the read picture of a file. A test uses it.
    #[cfg(feature = "test-support")]
    pub fn owners_of_picture(&self, attachment_id: AttachmentId) -> Vec<junto_client_core::Owner> {
        self.message_pictures.held.ready_of(attachment_id)
    }

    /// The pictures of an account that a server has no more, and of a server that was
    /// removed, are dropped (ADR 0745).
    fn keep_pictures_of_present_owners(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        self.message_pictures
            .held
            .keep(|owner| state.owner(owner.entry.server_id) == Some(*owner));
        self.release_dropped_pictures(cx);
    }

    fn release_dropped_pictures(&mut self, cx: &mut Context<Self>) {
        for image in self.message_pictures.held.take_dropped() {
            crate::images::release(image, cx);
        }
    }

    /// A drawing asks for a picture of a message. Returns what the window holds the
    /// picture under. `None` when the device has no account on the server. A picture that
    /// is not held is read after the drawing (see [`Shell::start_pictures`]).
    fn want_picture(
        &mut self,
        server_id: ServerId,
        attachment: &Attachment,
        cx: &mut Context<Self>,
    ) -> Option<PictureKey> {
        let owner = self.state.read(cx).owner(server_id)?;
        let key = PictureKey::of(owner, attachment);
        let pictures = &mut self.message_pictures;
        if pictures.held.touch(&key, cx.background_executor().now()) {
            return Some(key);
        }
        if pictures.waiting.len() < MOST_PICTURES
            && pictures.waiting.iter().all(|(waiting, _)| *waiting != key)
        {
            pictures.waiting.push((key.clone(), attachment.clone()));
        }
        if !std::mem::replace(&mut pictures.starting, true) {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |this, cx| this.start_pictures(cx));
            })
            .detach();
        }
        Some(key)
    }

    /// After a drawing: reads the pictures that the drawing asked for and that are not
    /// held, or whose read got no answer and is due again, each once for its owner, and
    /// decodes them away from the window. A picture that finds no read and no place stays
    /// unread until a later drawing asks again.
    fn start_pictures(&mut self, cx: &mut Context<Self>) {
        self.message_pictures.starting = false;
        for (key, attachment) in std::mem::take(&mut self.message_pictures.waiting) {
            let Some(number) = self
                .message_pictures
                .held
                .start(&key, cx.background_executor().now())
            else {
                continue;
            };
            let reading = self.state.read(cx).picture(key.owner, &attachment);
            cx.spawn(async move |this, cx| {
                let outcome = match reading.await {
                    Ok(bytes) => cx
                        .background_spawn(async move { decode(&bytes) })
                        .await
                        .map_or(Outcome::Refused, Outcome::Read),
                    Err(failure) if failure.is_transient() => Outcome::Unanswered,
                    Err(_) => Outcome::Refused,
                };
                let again = matches!(outcome, Outcome::Unanswered);
                let _ = this.update(cx, |this, cx| {
                    // An answer for a picture that is not held any more, or that was asked
                    // for again since, changes no picture. The window is drawn in both
                    // cases: a read has ended, and a picture that waited can start its own.
                    let now = cx.background_executor().now();
                    let held = &mut this.message_pictures.held;
                    let settled = held.settle(&key, number, outcome, now);
                    if settled
                        && again
                        && let Some(Picture::Unanswered { again }) = held.get(&key)
                    {
                        // A drawing after the wait reads the picture again.
                        let wait = again.saturating_duration_since(now);
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(wait).await;
                            let _ = this.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                    this.release_dropped_pictures(cx);
                    cx.notify();
                });
            })
            .detach();
        }
        self.release_dropped_pictures(cx);
    }

    /// The files of a message: a picture inline, every other file as a row with its size
    /// and the state of its download.
    fn attachments(
        &mut self,
        feed: &Feed,
        message: &Message,
        workspace: Option<&WorkspaceView>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let server_id = feed.server_id;
        let _ = workspace;
        message
            .attachments
            .iter()
            .enumerate()
            .map(|(index, attachment)| {
                let download = self.download_control(server_id, attachment, index, theme, cx);
                let name: SharedString = text::label(&attachment.name).into();
                let size: SharedString = text::file_size(attachment.bytes).into();
                if is_picture(attachment) {
                    let key = self.want_picture(server_id, attachment, cx);
                    let held = key
                        .as_ref()
                        .and_then(|key| self.message_pictures.held.get(key));
                    #[cfg(feature = "test-support")]
                    let held = held.or_else(|| self.message_pictures.shown.get(&attachment.id));
                    let (width, height) = picture_size(attachment);
                    let picture: AnyElement = match held {
                        Some(Picture::Ready(image)) => img(image.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain)
                            .into_any_element(),
                        Some(Picture::Failed | Picture::Unanswered { .. }) => div()
                            .p(u(8.0))
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_3)
                            .child(text::PICTURE_FAILED)
                            .into_any_element(),
                        _ => div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_3)
                            .child(text::PICTURE_LOADING)
                            .into_any_element(),
                    };
                    return div()
                        .flex_none()
                        .mt(u(5.0))
                        .w(u(width.max(200.0)))
                        .rounded(u(theme.metrics.radius))
                        .border_1()
                        .border_color(Hsla::from(colors.rule))
                        .bg(Hsla::from(colors.travertine_hi))
                        .overflow_hidden()
                        .debug_selector(move || format!("picture:{}", index))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w_full()
                                .h(u(height))
                                .bg(Hsla::from(colors.plaster))
                                .child(picture),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(u(7.0))
                                .min_h(u(28.0))
                                .px(u(8.0))
                                .text_size(u(13.0))
                                .ink(colors.ink_2)
                                .child(icon(Icon::Picture, 14.0, colors.ink_2))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::BOLD)
                                        .ink(colors.ink)
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(u(12.5))
                                        .ink(colors.ink_3)
                                        .child(size),
                                )
                                .child(div().flex_1())
                                .child(download),
                        )
                        .into_any_element();
                }
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(10.0))
                    .mt(u(5.0))
                    .w_full()
                    .max_w(u(PICTURE_WIDTH + 60.0))
                    .min_h(u(42.0))
                    .pl(u(6.0))
                    .pr(u(6.0))
                    .rounded(u(theme.metrics.radius))
                    .border_1()
                    .border_color(Hsla::from(colors.rule))
                    .bg(Hsla::from(colors.travertine_hi))
                    .debug_selector(move || format!("file:{}", index))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(u(30.0))
                            .rounded(u(theme.metrics.radius_small))
                            .bg(Hsla::from(colors.terracotta))
                            .font_family(theme.fonts.mono.clone())
                            .font_weight(FontWeight::BOLD)
                            .text_size(u(12.0))
                            .ink(colors.on_terracotta)
                            .child(extension(&attachment.name)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(u(13.5))
                                    .line_height(u(17.0))
                                    .font_weight(FontWeight::BOLD)
                                    .ink(colors.ink)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(u(12.5))
                                    .line_height(u(17.0))
                                    .ink(colors.ink_3)
                                    .child(size),
                            ),
                    )
                    .child(download)
                    .into_any_element()
            })
            .collect()
    }

    /// The control at the end of a file: it starts the download, shows how far it is, and
    /// then where the file is.
    fn download_control(
        &mut self,
        server_id: ServerId,
        attachment: &Attachment,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let attachment_id = attachment.id;
        let progress = self
            .view
            .as_ref()
            .and_then(|view| view.downloads.get(&(server_id, attachment_id)))
            .cloned();
        let small = |sentence: SharedString, colour| {
            div()
                .flex_none()
                .max_w(u(150.0))
                .truncate()
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
                        state.forget_download(server_id, attachment_id, cx);
                    });
                }))
        };
        let start = |cx: &mut Context<Self>| {
            let attachment = attachment.clone();
            Button::new(("download", index))
                .kind(ButtonKind::Ghost)
                .icon(Icon::Download)
                .icon_colour(colors.cypress)
                .tooltip(text::DOWNLOAD)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.download(server_id, &attachment, cx);
                    });
                }))
        };
        let row = div().flex().flex_none().items_center().gap(u(4.0));
        match progress.map(|progress| (progress.stage.clone(), progress)) {
            None => row.child(start(cx)).into_any_element(),
            Some((DownloadStage::Receiving, progress)) => row
                .child(small(
                    if progress.interrupted {
                        text::DOWNLOAD_INTERRUPTED.into()
                    } else {
                        text::transfer_progress(progress.received, progress.bytes).into()
                    },
                    colors.ink_2,
                ))
                .child(forget("cancel-download", text::CANCEL_DOWNLOAD, cx))
                .into_any_element(),
            Some((DownloadStage::Checking, _)) => row
                .child(small(text::DOWNLOAD_CHECKING.into(), colors.ink_2))
                .into_any_element(),
            Some((DownloadStage::Saved(path), _)) => row
                .child(small(text::DOWNLOAD_SAVED.into(), colors.cypress))
                .child(
                    Button::new(("show-file", index))
                        .kind(ButtonKind::Ghost)
                        .icon(Icon::Open)
                        .tooltip(text::SHOW_IN_FOLDER)
                        .on_click(move |_, _, cx| cx.reveal_path(&path)),
                )
                .into_any_element(),
            Some((DownloadStage::Failed(failure), _)) => row
                .child(
                    div()
                        .id(("download-failed", index))
                        .tooltip(tooltip(text::transfer_failure(failure)))
                        .child(icon(Icon::Warning, 14.0, colors.cinnabar)),
                )
                .child(forget("retry-download", text::TRY_AGAIN, cx))
                .into_any_element(),
            Some((DownloadStage::Cancelled, _)) => row.child(start(cx)).into_any_element(),
        }
    }

    /// The actions of a message at the pointer: react, reply, edit and delete.
    fn action_bar(
        &mut self,
        feed: &Feed,
        row: &MessageRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let message_id = row.message.id;
        let server_id = feed.server_id;
        let pane = Pane::of(feed.place);
        let pinned = row.message.pinned;
        let channel_id = crate::state::feed::channel_of(feed.place);
        let action = |id: &'static str, shown: Icon, label: &'static str| {
            div()
                .id(id)
                .debug_selector(move || format!("{id}:{message_id}"))
                .flex()
                .items_center()
                .justify_center()
                .w(u(26.0))
                .h(u(22.0))
                .rounded(u(theme.metrics.radius_small))
                .cursor_pointer()
                .hover(move |style| style.bg(Hsla::from(colors.plaster)))
                .tooltip(tooltip(label))
                .child(icon(shown, 15.0, colors.ink_2))
        };
        div()
            .debug_selector(move || format!("message-actions:{message_id}"))
            .absolute()
            .top(u(-10.0))
            .right(u(18.0))
            .flex()
            .items_center()
            .gap(u(1.0))
            .px(u(3.0))
            .py(u(2.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
            .shadow(ledge(colors.rule))
            .child(
                action("react", Icon::Smile, text::REACT).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_emoji_picker(EmojiTarget::React(message_id), window, cx);
                    },
                )),
            )
            .child(
                action("reply", Icon::Reply, text::REPLY).on_click(cx.listener(
                    move |this, _, window, cx| this.reply_to_message(pane, message_id, window, cx),
                )),
            )
            .when_some(
                channel_id.filter(|_| row.can_thread && pane == Pane::Main),
                |bar, channel_id| {
                    bar.child(
                        action("thread", Icon::Chat, text::REPLY_IN_THREAD).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_thread(server_id, channel_id, message_id, window, cx);
                            },
                        )),
                    )
                },
            )
            .when(row.can_pin, |bar| {
                bar.child(
                    action(
                        "pin",
                        Icon::Pin,
                        if pinned { text::UNPIN } else { text::PIN },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| {
                            state.set_pinned(server_id, message_id, !pinned, cx);
                        });
                    })),
                )
            })
            .when(row.can_edit, |bar| {
                bar.child(action("edit", Icon::Edit, text::EDIT).on_click(cx.listener(
                    move |this, _, window, cx| this.edit_message(pane, message_id, window, cx),
                )))
            })
            .when(row.can_delete, |bar| {
                bar.child(
                    action("delete", Icon::Trash, text::DELETE).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_layer(Layer::ConfirmDelete(message_id), window, cx);
                        },
                    )),
                )
            })
            // A permanent message has no edit and no delete for its author (ADR 0402).
            .when(row.message.permanent && !row.message.deleted, |bar| {
                bar.child(
                    div()
                        .id("no-edit")
                        .debug_selector(move || format!("no-edit:{message_id}"))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(5.0))
                        .h(u(22.0))
                        .ml(u(3.0))
                        .pl(u(7.0))
                        .pr(u(5.0))
                        .border_l_1()
                        .border_color(Hsla::from(colors.rule_soft))
                        .text_size(u(12.5))
                        .ink(colors.ink_2)
                        .whitespace_nowrap()
                        .child(icon(Icon::EditOff, 14.0, colors.ink_2))
                        .child(text::NO_EDIT_OR_DELETE),
                )
            })
            .into_any_element()
    }

    /// The line above a pinned message.
    fn pin_mark(theme: &Theme) -> AnyElement {
        let colors = theme.colors;
        div()
            .flex()
            .items_center()
            .gap(u(6.0))
            .pl(u(44.0))
            .mb(u(1.0))
            .text_size(u(12.5))
            .line_height(u(17.0))
            .font_weight(FontWeight::SEMIBOLD)
            .ink(colors.ink_2)
            .child(icon(Icon::Pin, 12.0, colors.ink_2))
            .child(text::PINNED)
            .into_any_element()
    }

    /// What a message of a channel shows of its thread: the count of the replies, the
    /// author and the time of the last one, and what the person has not read in it. A
    /// press opens the thread in the side panel.
    fn thread_chip(
        &mut self,
        feed: &Feed,
        row: &MessageRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = theme.colors;
        let faces = self.faces(feed.server_id);
        let mark = row.thread.as_ref()?;
        let channel_id = crate::state::feed::channel_of(feed.place)?;
        let (server_id, message_id) = (feed.server_id, row.message.id);
        let unread = (mark.unread > 0 || mark.mentions > 0)
            .then(|| text::thread_unread(mark.unread, mark.mentions));
        let label: SharedString = match &mark.last {
            Some(last) => format!(
                "{}, {}, {}",
                text::replies(mark.replies),
                last.author,
                text::last_reply(&last.time)
            ),
            None => text::replies(mark.replies),
        }
        .into();
        Some(
            div()
                .flex()
                .mt(u(4.0))
                .child(
                    div()
                        .id(("thread-chip", row.message.position as usize))
                        .role(if row.can_thread {
                            Role::Button
                        } else {
                            Role::Note
                        })
                        .aria_label(label)
                        .debug_selector(move || format!("thread-chip:{message_id}"))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(8.0))
                        .h(u(28.0))
                        .pl(u(7.0))
                        .pr(u(9.0))
                        .rounded(u(theme.metrics.radius))
                        .border_1()
                        .border_color(Hsla::from(colors.rule))
                        .bg(Hsla::from(colors.travertine_hi))
                        .text_size(u(13.0))
                        .when(row.can_thread, |chip| {
                            chip.cursor_pointer()
                                .focusable()
                                .tab_stop(true)
                                .hover(move |style| style.bg(Hsla::from(colors.plaster)))
                                .focus_visible(move |style| {
                                    style.shadow(focus_ring_inside(
                                        colors.terracotta_deep,
                                        theme.metrics.focus_ring,
                                    ))
                                })
                        })
                        .when(row.can_thread, |chip| {
                            chip.tooltip(tooltip(text::OPEN_THREAD))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_thread(server_id, channel_id, message_id, window, cx);
                                }))
                        })
                        // The first authors of the replies, as the design draws them: three
                        // small pictures that overlap. A server that states no author
                        // gives the picture of the author of the last reply.
                        .when(!mark.authors.is_empty(), |chip| {
                            let mut authors = div()
                                .debug_selector(move || format!("thread-authors:{message_id}"))
                                .flex()
                                .flex_none()
                                .items_center();
                            for (place, author) in mark.authors.iter().enumerate() {
                                authors = authors.child(
                                    div()
                                        .flex_none()
                                        .when(place > 0, |picture| picture.ml(u(-4.0)))
                                        .child(
                                            Avatar::new(author.as_uuid().as_u128(), 15.0)
                                                .radius(2.0)
                                                .image(faces.image(*author)),
                                        ),
                                );
                            }
                            chip.child(authors)
                        })
                        .when_some(
                            mark.last.as_ref().filter(|_| mark.authors.is_empty()),
                            |chip, last| {
                                let seed = last
                                    .author_id
                                    .map_or(0, |author| author.as_uuid().as_u128());
                                let picture = last.author_id.and_then(|author| faces.image(author));
                                chip.child(Avatar::new(seed, 18.0).radius(2.0).image(picture))
                            },
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .ink(colors.terracotta_deep)
                                .child(SharedString::from(text::replies(mark.replies))),
                        )
                        .when_some(mark.last.as_ref(), |chip, last| {
                            chip.child(
                                div()
                                    .flex_none()
                                    .text_size(u(12.5))
                                    .ink(colors.ink_2)
                                    .child(SharedString::from(text::last_reply(&last.time))),
                            )
                        })
                        .when_some(unread, |chip, unread| {
                            chip.child(
                                div()
                                    .debug_selector(move || format!("thread-unread:{message_id}"))
                                    .flex_none()
                                    .px(u(5.0))
                                    .rounded(u(theme.metrics.radius_small))
                                    .bg(Hsla::from(colors.terracotta))
                                    .text_size(u(12.0))
                                    .font_weight(FontWeight::BOLD)
                                    .ink(colors.on_terracotta)
                                    .child(SharedString::from(unread)),
                            )
                        })
                        .when(mark.open, |chip| {
                            chip.child(
                                div()
                                    .flex_none()
                                    .text_size(u(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .ink(colors.terracotta_deep)
                                    .child(text::SHOWN_IN_PANEL),
                            )
                        }),
                )
                .into_any_element(),
        )
    }

    /// One message of a person.
    pub(crate) fn render_message(
        &mut self,
        index: usize,
        row: &MessageRow,
        feed: &Feed,
        shape: MessageShape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact = shape != MessageShape::Wide;
        let bare = shape == MessageShape::Bare;
        let theme = theme(cx);
        let colors = theme.colors;
        let message = &row.message;
        let message_id = message.id;
        let side = if compact { 14.0 } else { 16.0 };
        let seed = message
            .author_id
            .map_or(0, |author| author.as_uuid().as_u128());
        let faces = self.faces(feed.server_id);
        let picture = message.author_id.and_then(|author| faces.image(author));
        let body: Vec<AnyElement> = if message.deleted {
            vec![
                div()
                    .ink(colors.ink_3)
                    .child(text::DELETED_MESSAGE)
                    .into_any_element(),
            ]
        } else {
            let mut body = match &message.document {
                Some(document) => self.document(
                    document,
                    Some((feed, message, row.own)),
                    &feed.names,
                    &theme,
                    cx,
                ),
                None => Vec::new(),
            };
            body.extend(self.attachments(feed, message, None, &theme, cx));
            body.extend(self.reactions(feed, message, &theme, cx));
            if !bare {
                body.extend(self.thread_chip(feed, row, &theme, cx));
            }
            body
        };
        let pane = Pane::of(feed.place);
        let header = (!row.grouped).then(|| {
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
                        .child(SharedString::from(row.author.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(u(12.5))
                        .ink(colors.ink_3)
                        .child(SharedString::from(row.time.clone())),
                )
                .when(message.edited_at.is_some() && !message.deleted, |header| {
                    header.child(
                        div()
                            .flex_none()
                            .text_size(u(12.5))
                            .ink(colors.ink_3)
                            .child(text::EDITED),
                    )
                })
        });
        let open_menu = !bare
            && matches!(&self.layer, Layer::MessageMenu(open)
                if open.message_id == message_id && open.pane == pane);
        let menu = open_menu.then(|| self.render_message_menu(row, feed, window, cx));
        let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        let actions =
            (!message.deleted && !bare && self.hovered_message == Some((pane, message_id)))
                .then(|| self.action_bar(feed, row, &theme, cx));
        let label: SharedString = format!(
            "{}, {}: {}",
            row.author,
            row.time,
            message
                .document
                .as_ref()
                .map(|document| crate::state::feed::plain_text(document, &feed.names, 200))
                .unwrap_or_default()
        )
        .into();
        div()
            .id(("message", index))
            .role(Role::ListItem)
            .aria_label(label)
            .debug_selector(move || {
                if bare {
                    format!("shown-message:{message_id}")
                } else {
                    format!("message:{message_id}")
                }
            })
            .relative()
            .pt(u(if row.grouped { 1.0 } else { 4.0 }))
            .pb(u(if row.grouped { 1.0 } else { 4.0 }))
            .pl(u(side))
            .pr(u(18.0))
            .when(row.mentions_own, |frame| {
                frame.bg(Hsla::from(colors.mention_wash))
            })
            // The message that the person went to from a notice or a search result.
            .when(row.target, |frame| frame.bg(Hsla::from(colors.target_wash)))
            .when(!bare, |frame| {
                frame.hover(move |style| style.bg(Hsla::from(colors.row_hover)))
            })
            .when(open_menu, |frame| frame.bg(Hsla::from(colors.row_hover)))
            .when(!message.deleted && !bare, |frame| {
                let (by_click, by_key) = (
                    cx.listener(move |this, _: &gpui::MouseDownEvent, window, cx| {
                        this.open_message_menu(pane, message_id, window, cx);
                    }),
                    cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                        let keys = &event.keystroke;
                        let opens = match keys.key.as_str() {
                            "menu" | "enter" | "space" => true,
                            "f10" => keys.modifiers.shift,
                            _ => false,
                        };
                        if opens {
                            this.open_message_menu(pane, message_id, window, cx);
                            cx.stop_propagation();
                        }
                    }),
                );
                frame
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(move |style| style.shadow(ring))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.message_hovered(pane, message_id, *hovered, cx);
                    }))
                    .on_mouse_down(MouseButton::Right, by_click)
                    .on_key_down(by_key)
            })
            .children((message.pinned && !message.deleted && !bare).then(|| Self::pin_mark(&theme)))
            .children(
                row.reply
                    .as_ref()
                    .map(|quote| Self::reply_line(quote, &faces, &theme)),
            )
            .child(
                div()
                    .flex()
                    .gap(u(10.0))
                    .child(if row.grouped {
                        div().flex_none().w(u(34.0)).into_any_element()
                    } else {
                        div()
                            .flex_none()
                            .mt(u(2.0))
                            .child(Avatar::new(seed, 34.0).image(picture))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(u(14.5))
                            .line_height(u(21.0))
                            .ink(colors.ink)
                            .children(header)
                            .children(body),
                    ),
            )
            .children(actions)
            .children(menu)
            .into_any_element()
    }

    /// A message that the person sent and that the server has not confirmed.
    fn render_waiting(
        &mut self,
        index: usize,
        row: &WaitingRow,
        feed: &Feed,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let pending = &row.pending;
        let (server_id, id) = (feed.server_id, pending.id);
        let side = if compact { 14.0 } else { 16.0 };
        let faces = self.faces(server_id);
        let person = self.view.as_ref().and_then(|view| view.person.account_id);
        let own = person.map_or(0, |account| account.as_uuid().as_u128());
        let picture = person.and_then(|account| faces.image(account));
        let uploads = self
            .view
            .as_ref()
            .and_then(|view| view.workspace())
            .map(|workspace| workspace.uploads.clone())
            .unwrap_or_default();
        let (sentence, failed): (SharedString, bool) = match pending.state {
            SendState::WaitingForFiles => (text::WAITING_FOR_FILES.into(), false),
            SendState::Sending => (text::SENDING.into(), false),
            SendState::Unconfirmed => (text::NOT_CONFIRMED.into(), true),
            SendState::Failed(failure) => (text::send_failure(failure).into(), true),
        };
        let files = pending.files.iter().map(|file| {
            let state: SharedString = match uploads.get(&file.upload) {
                Some(progress) => match progress.stage {
                    UploadStage::Reading => text::UPLOAD_READING.into(),
                    UploadStage::Sending if progress.interrupted => text::UPLOAD_INTERRUPTED.into(),
                    UploadStage::Sending => {
                        text::transfer_progress(progress.sent, progress.bytes).into()
                    }
                    UploadStage::Checking => text::UPLOAD_CHECKING.into(),
                    UploadStage::Complete(_) => text::UPLOAD_COMPLETE.into(),
                    UploadStage::Failed(failure) => text::transfer_failure(failure).into(),
                    UploadStage::Cancelled => text::DISCARD.into(),
                },
                None => text::file_size(file.bytes).into(),
            };
            div()
                .flex()
                .items_center()
                .gap(u(7.0))
                .mt(u(4.0))
                .text_size(u(13.0))
                .ink(colors.ink_2)
                .child(icon(Icon::Clip, 14.0, colors.ink_2))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::BOLD)
                        .child(SharedString::from(text::label(&file.name))),
                )
                .child(div().flex_none().ink(colors.ink_3).child(state))
        });
        let body = self.document(&pending.document, None, &feed.names, &theme, cx);
        div()
            .id(("waiting", index))
            .debug_selector(move || format!("waiting:{id}"))
            .pt(u(if row.grouped { 1.0 } else { 4.0 }))
            .pb(u(4.0))
            .pl(u(side))
            .pr(u(18.0))
            .children(
                row.reply
                    .as_ref()
                    .map(|quote| Self::reply_line(quote, &faces, &theme)),
            )
            .child(
                div()
                    .flex()
                    .gap(u(10.0))
                    .child(if row.grouped {
                        div().flex_none().w(u(34.0)).into_any_element()
                    } else {
                        div()
                            .flex_none()
                            .mt(u(2.0))
                            .child(Avatar::new(own, 34.0).image(picture))
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(u(14.5))
                            .line_height(u(21.0))
                            .ink(colors.ink_2)
                            .when(!row.grouped, |column| {
                                column.child(
                                    div()
                                        .flex()
                                        .items_baseline()
                                        .gap(u(8.0))
                                        .line_height(u(19.0))
                                        .child(
                                            div()
                                                .font_weight(FontWeight::BOLD)
                                                .ink(colors.ink)
                                                .child(SharedString::from(row.author.clone())),
                                        )
                                        .child(
                                            div()
                                                .text_size(u(12.5))
                                                .ink(colors.ink_3)
                                                .child(SharedString::from(row.time.clone())),
                                        ),
                                )
                            })
                            .children(body)
                            .children(files)
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap(u(8.0))
                                    .mt(u(2.0))
                                    .child(
                                        div()
                                            .debug_selector(move || format!("waiting-state:{id}"))
                                            .text_size(u(12.5))
                                            .line_height(u(17.0))
                                            .ink(if failed {
                                                colors.cinnabar_ink
                                            } else {
                                                colors.ink_3
                                            })
                                            .child(sentence),
                                    )
                                    .when(can_send_again(pending.state), |line| {
                                        line.child(
                                            Button::new(("send-again", index))
                                                .label(text::SEND_AGAIN)
                                                .selector("send-again")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.state.update(cx, |state, _| {
                                                        state.send_again(server_id, id);
                                                    });
                                                })),
                                        )
                                    })
                                    .when(pending.state != SendState::Sending, |line| {
                                        line.child(
                                            Button::new(("discard", index))
                                                .label(text::DISCARD)
                                                .selector("discard-message")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.state.update(cx, |state, cx| {
                                                        state.discard_message(server_id, id, cx);
                                                    });
                                                })),
                                        )
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    pub fn open_message_menu(
        &mut self,
        pane: Pane,
        message_id: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(
            Layer::MessageMenu(MessageMenuOpen { message_id, pane }),
            window,
            cx,
        );
    }

    /// Shows the thread under a message of a channel in the side panel, with the keyboard
    /// focus in its composer.
    pub fn open_thread(
        &mut self,
        server_id: ServerId,
        channel_id: junto_client_core::protocol::ids::ChannelId,
        root_id: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.members_open = false;
        self.state.update(cx, |state, cx| {
            state.open_thread(server_id, channel_id, root_id, cx);
        });
        self.focus_composer(Pane::Thread, window, cx);
        cx.notify();
    }

    /// Asks whether a message is to be deleted.
    pub fn ask_to_delete_message(
        &mut self,
        message_id: MessageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(Layer::ConfirmDelete(message_id), window, cx);
    }

    /// The pointer entered or left the row of a message: the bar of actions moves to the
    /// row under the pointer.
    fn message_hovered(
        &mut self,
        pane: Pane,
        message_id: MessageId,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let shown = if hovered {
            Some((pane, message_id))
        } else if self.hovered_message == Some((pane, message_id)) {
            None
        } else {
            return;
        };
        if self.hovered_message != shown {
            self.hovered_message = shown;
            cx.notify();
        }
    }

    /// True while the list shows the end of its rows.
    pub fn feed_is_at_end(&self, pane: Pane) -> bool {
        self.list(pane).at_end()
    }

    /// The bounds in which the last frame drew the list of a pane. A test that measures a
    /// build without debug assertions, where gpui keeps no names of elements, turns the
    /// wheel of the pointer over this place (ADR 0914).
    #[cfg(feature = "test-support")]
    pub fn feed_bounds(&self, pane: Pane) -> Option<gpui::Bounds<gpui::Pixels>> {
        let list = self.list(pane);
        list.drawn.then(|| list.list.viewport_bounds())
    }

    /// The menu of a message: the actions of its bar for the keyboard.
    fn render_message_menu(
        &mut self,
        row: &MessageRow,
        feed: &Feed,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let message_id = row.message.id;
        let server_id = feed.server_id;
        let pane = Pane::of(feed.place);
        let pinned = row.message.pinned;
        let mut items = vec![
            MenuItem::new("message-react", text::REACT)
                .icon(Icon::Smile)
                .selector("message-react")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_layer(window, cx);
                    this.open_emoji_picker(EmojiTarget::React(message_id), window, cx);
                }))
                .into_any_element(),
            MenuItem::new("message-reply", text::REPLY)
                .icon(Icon::Reply)
                .selector("message-reply")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_layer(window, cx);
                    this.reply_to_message(pane, message_id, window, cx);
                }))
                .into_any_element(),
        ];
        if let Some(channel_id) = crate::state::feed::channel_of(feed.place)
            .filter(|_| row.can_thread && pane == Pane::Main)
        {
            items.push(
                MenuItem::new("message-thread", text::REPLY_IN_THREAD)
                    .icon(Icon::Chat)
                    .selector("message-thread")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_layer(window, cx);
                        this.open_thread(server_id, channel_id, message_id, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if row.can_pin {
            items.push(
                MenuItem::new("message-pin", if pinned { text::UNPIN } else { text::PIN })
                    .icon(Icon::Pin)
                    .selector("message-pin")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_layer(window, cx);
                        this.state.update(cx, |state, cx| {
                            state.set_pinned(server_id, message_id, !pinned, cx);
                        });
                    }))
                    .into_any_element(),
            );
        }
        if row.can_edit {
            items.push(
                MenuItem::new("message-edit", text::EDIT)
                    .icon(Icon::Edit)
                    .selector("message-edit")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_layer(window, cx);
                        this.edit_message(pane, message_id, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        if row.can_delete {
            items.push(
                MenuItem::new("message-delete", text::DELETE)
                    .icon(Icon::Trash)
                    .selector("message-delete")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_layer(Layer::ConfirmDelete(message_id), window, cx);
                    }))
                    .into_any_element(),
            );
        }
        gpui::deferred(
            gpui::anchored().child(
                // The menu opens at the start of the text of its message, inside the list.
                div().absolute().top(u(4.0)).left(u(44.0)).child(
                    Menu::new("message-menu", text::MESSAGE_ACTIONS, self.layer_focus())
                        .selector("message-menu")
                        .on_close(Self::handler(cx, |this, window, cx| {
                            this.close_layer(window, cx);
                        }))
                        .children(items),
                ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// The question before a message is deleted: the deletion cannot be undone
    /// (ADR 0401).
    pub(crate) fn render_confirm_delete(
        &mut self,
        message_id: MessageId,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let server_id = self.pane_feed(Pane::Main).map(|feed| feed.server_id);
        crate::ui::Dialog::new(text::DELETE_MESSAGE_TITLE, self.layer_focus())
            .selector("confirm-delete-message")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            .child(
                div()
                    .typeface(theme.types.notice, &theme)
                    .ink(theme.colors.ink_2)
                    .child(text::DELETE_MESSAGE_CONFIRM),
            )
            .child(crate::ui::dialog_footer([
                Button::new("cancel-delete-message")
                    .label(text::CANCEL)
                    .selector("cancel-delete-message")
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new("delete-message")
                    .kind(ButtonKind::Primary)
                    .label(text::DELETE)
                    .selector("delete-message")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(server_id) = server_id {
                            this.state.update(cx, |state, cx| {
                                state.delete_message(server_id, message_id, cx);
                            });
                        }
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
            ]))
            .into_any_element()
    }
}

/// The three small marks beside the date of a day separator.
fn day_marks(theme: &Theme) -> impl IntoElement + use<> {
    let colors = theme.colors;
    let mark = |colour| div().flex_none().size(u(5.0)).bg(Hsla::from(colour));
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(4.0))
        .child(mark(colors.ochre))
        .child(mark(colors.terracotta).rounded_full())
        .child(mark(colors.cypress))
}

/// A line that states what happened in a channel, with its time.
fn system_line(row: &SystemRow, theme: &Theme) -> AnyElement {
    let colors = theme.colors;
    let message_id = row.message.id;
    let mark = match row.kind {
        SystemKind::Start => div().bg(Hsla::from(colors.cypress)),
        SystemKind::Recording => div().bg(Hsla::from(colors.cinnabar)).rounded_full(),
        SystemKind::Plain => div().bg(Hsla::from(colors.bronze)),
    };
    let mut line = String::new();
    let mut strong = Vec::new();
    for part in &row.parts {
        let start = line.len();
        line.push_str(&part.text);
        if part.strong {
            strong.push((
                start..line.len(),
                HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    color: Some(colors.ink.into()),
                    ..HighlightStyle::default()
                },
            ));
        }
    }
    div()
        .debug_selector(move || format!("system:{message_id}"))
        .flex()
        .items_start()
        .gap(u(10.0))
        .py(u(2.0))
        .px(u(14.0))
        .text_size(u(13.0))
        .line_height(u(19.0))
        .ink(colors.ink_2)
        .child(
            div()
                .flex_none()
                .w(u(40.0))
                .whitespace_nowrap()
                .text_right()
                .text_size(u(12.5))
                .ink(colors.ink_3)
                .child(SharedString::from(row.time.clone())),
        )
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .gap(u(7.0))
                .child(mark.flex_none().mt(u(7.0)).size(u(6.0)))
                .child(
                    div()
                        .min_w_0()
                        .child(StyledText::new(line).with_highlights(strong)),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_that_is_no_web_or_mail_address_is_no_link() {
        let theme = Theme::stone();
        let names = Names::default();
        let link = |url: &str| Inline::Link {
            text: "here".to_owned(),
            url: url.to_owned(),
        };
        // A server can send a document that it did not check (ADR 0714).
        let content = [
            link("https://example.com/a"),
            link("file:///etc/passwd"),
            link("javascript:alert(1)"),
            link("smb://host/share"),
            link("mailto:a@example.com"),
            link("https://user:secret@example.com"),
        ];
        let rich = Rich::of(&content, &names, &theme);
        assert_eq!(rich.text, "here".repeat(6));
        let opened: Vec<&str> = rich
            .links
            .iter()
            .map(|(_, address)| address.as_str())
            .collect();
        assert_eq!(opened, ["https://example.com/a", "mailto:a@example.com"]);
    }
}
