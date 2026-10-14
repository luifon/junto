//! The search of the window (ADR 0743, view H of the design): what the person asked for,
//! the pages that the server answered, and the panel that is derived from them.
//!
//! A search holds no state in the client core (ADR 0722). Each page is decided by what
//! the account can read at the time of its request, and a result of a place that the
//! state of the server does not hold any more is not listed.

use std::collections::HashMap;
use std::ops::Range;

use junto_client_core::model::{ServerModel, WorkspaceModel};
use junto_client_core::protocol::discovery::SearchHit;
use junto_client_core::protocol::ids::{
    AccountId, ChannelId, MessageId, RecordingId, ServerId, WorkspaceId,
};
use junto_client_core::protocol::structure::ChannelKind;
use time::OffsetDateTime;
use uuid::Uuid;

use super::feed::Zone;
use super::notices::{Whereabouts, account_names};
use super::view::Known;
use crate::text;

/// The time span of a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DateFilter {
    #[default]
    AnyTime,
    /// Since the day began in the time zone of the person.
    Today,
    Last7Days,
    Last30Days,
}

impl DateFilter {
    pub const ALL: [Self; 4] = [
        Self::AnyTime,
        Self::Today,
        Self::Last7Days,
        Self::Last30Days,
    ];

    /// The first moment that the search covers. Absent: every time.
    pub fn from(self, now: OffsetDateTime, zone: Zone) -> Option<OffsetDateTime> {
        match self {
            Self::AnyTime => None,
            Self::Today => Some(crate::moment::in_utc(
                zone.local(now).replace_time(time::Time::MIDNIGHT),
            )),
            Self::Last7Days => Some(now - time::Duration::days(7)),
            Self::Last30Days => Some(now - time::Duration::days(30)),
        }
    }
}

/// The filters of a search (ADR 0405).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Filters {
    pub channel: Option<ChannelId>,
    /// The author of a message, or the speaker of a transcript line.
    pub author: Option<AccountId>,
    pub date: DateFilter,
}

/// Where the last request of a search stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Searching,
    Done,
    /// The server did not take the search, for this reason.
    Refused(String),
    /// The server gave no answer.
    Failed(String),
}

/// One result as the server gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub message_id: Option<MessageId>,
    pub recording_id: Option<RecordingId>,
    pub line: Option<u32>,
    /// The channel or the direct conversation of the result.
    pub place_id: Uuid,
    pub author_id: AccountId,
    pub created_at: OffsetDateTime,
    pub text: String,
}

impl From<SearchHit> for Hit {
    fn from(hit: SearchHit) -> Self {
        Self {
            message_id: hit.message_id,
            recording_id: hit.recording_id,
            line: hit.line,
            place_id: hit.place_id,
            author_id: hit.author_id,
            created_at: hit.created_at,
            text: bounded(hit.text),
        }
    }
}

/// The text of a result as the window holds it: its first [`MOST_HIT_CHARACTERS`].
fn bounded(text: String) -> String {
    let Some((end, _)) = text.char_indices().nth(MOST_HIT_CHARACTERS - 1) else {
        return text;
    };
    if text[end..].chars().nth(1).is_none() {
        return text;
    }
    let mut cut = text[..end].to_owned();
    cut.push('…');
    cut
}

/// How many results the window holds of one search. A further page is not asked for
/// then.
pub const MOST_HITS: usize = 1_000;
/// How many characters of the text of one result the window holds.
pub const MOST_HIT_CHARACTERS: usize = 600;

/// The search of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Search {
    pub server_id: ServerId,
    pub text: String,
    pub filters: Filters,
    pub progress: Progress,
    /// The results of the pages that were read, the best match first.
    pub hits: Vec<Hit>,
    /// The server has a page after the read ones.
    pub more: bool,
    pub reading_more: bool,
    /// The side panel shows the search.
    pub open: bool,
    /// The message of a result that is being read for the jump to it.
    pub going: Option<MessageId>,
    /// Why the last jump to a result did not happen.
    pub go_failed: Option<String>,
}

/// What the panel says in place of results, or beside them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchShown {
    Searching,
    Results,
    /// The search found nothing that the account can read.
    Nothing,
    Refused(String),
    Failed(String),
}

/// Where a result leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultTarget {
    /// A message. Its position and its thread are read when the person goes to it.
    Message(MessageId),
    /// A line of the transcript of a recording of a voice channel.
    Transcript {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        recording_id: RecordingId,
        line: u32,
    },
}

/// One result as the panel shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRow {
    pub place: Whereabouts,
    pub day: String,
    /// The time of day of a message, or the line of a transcript in words.
    pub time: String,
    pub author_id: AccountId,
    pub author: String,
    pub text: String,
    /// The parts of the text that match a word of the search.
    pub marks: Vec<Range<usize>>,
    pub target: ResultTarget,
}

/// The search as the side panel shows it (view H of the design).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPanel {
    pub server_id: ServerId,
    pub text: String,
    pub state: SearchShown,
    pub messages: Vec<ResultRow>,
    pub transcripts: Vec<ResultRow>,
    /// The server has results after the read ones.
    pub more: bool,
    /// The panel offers the page after the read ones.
    pub can_read_more: bool,
    pub reading_more: bool,
    /// The channels that the filter offers: the ones of the shown workspace.
    pub channels: Vec<(ChannelId, String, ChannelKind)>,
    /// The people that the filter offers: the members of the shown workspace.
    pub authors: Vec<(AccountId, String)>,
    pub channel: Option<(ChannelId, String)>,
    pub author: Option<(AccountId, String)>,
    pub date: DateFilter,
    /// The message of a result that is being read for the jump to it.
    pub going: Option<MessageId>,
    pub go_failed: Option<String>,
}

/// The words that a search looks for: the words and the words of the phrases of the
/// web-search syntax of the server (ADR 0409), without a word after a minus and without
/// `or`. Each is in lower case.
fn terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut quoted = false;
    let mut negated = false;
    let mut word = String::new();
    let mut end = |word: &mut String, negated: &mut bool, quoted: bool| {
        let held = std::mem::take(word);
        let is_or = !quoted && held == "or";
        if !held.is_empty() && !*negated && !is_or && !terms.contains(&held) {
            terms.push(held);
        }
        if !quoted {
            *negated = false;
        }
    };
    for character in query.chars() {
        if character == '"' {
            // A minus directly before a phrase excludes every word of the phrase.
            let excluded = !quoted && negated && word.is_empty();
            end(&mut word, &mut negated, quoted);
            quoted = !quoted;
            negated = quoted && excluded;
        } else if character.is_alphanumeric() {
            word.extend(character.to_lowercase());
        } else {
            let starts_negation = character == '-' && word.is_empty() && !quoted;
            end(&mut word, &mut negated, quoted);
            if starts_negation {
                negated = true;
            }
        }
    }
    end(&mut word, &mut negated, quoted);
    terms
}

/// The parts of a text that are a word of a search, in the order of the text. The server
/// compares whole words without their case (ADR 0409), and so does this function.
pub fn marks(text: &str, query: &str) -> Vec<Range<usize>> {
    let terms = terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut marks = Vec::new();
    let mut start: Option<usize> = None;
    let mut close = |start: &mut Option<usize>, end: usize| {
        if let Some(from) = start.take() {
            let word: String = text[from..end]
                .chars()
                .flat_map(char::to_lowercase)
                .collect();
            if terms.contains(&word) {
                marks.push(from..end);
            }
        }
    };
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
        } else {
            close(&mut start, index);
        }
    }
    close(&mut start, text.len());
    marks
}

/// The place of a result by the identifier that the server gives.
fn place_of(
    place_id: Uuid,
    model: &ServerModel,
    accounts: &HashMap<AccountId, String>,
) -> (Whereabouts, Option<(WorkspaceId, ChannelId)>) {
    let channel_id = ChannelId::from_uuid(place_id);
    if let Some(channel) = model.channel(channel_id) {
        return (
            Whereabouts::Channel {
                name: channel.channel.name.clone(),
                kind: channel.channel.kind,
            },
            Some((channel.channel.workspace_id, channel_id)),
        );
    }
    match model
        .conversations
        .iter()
        .find(|held| held.id.as_uuid() == place_id)
    {
        Some(conversation) => {
            let other = if conversation.low_account_id == model.account.id {
                conversation.high_account_id
            } else {
                conversation.low_account_id
            };
            (
                Whereabouts::Direct {
                    name: accounts.get(&other).cloned(),
                },
                None,
            )
        }
        None => (Whereabouts::Unknown, None),
    }
}

/// The search as the side panel shows it. Absent while no search of this server is open.
pub fn search_panel(
    server_id: ServerId,
    workspace: &WorkspaceModel,
    model: &ServerModel,
    known: &Known,
) -> Option<SearchPanel> {
    let search = known
        .search
        .as_ref()
        .filter(|search| search.server_id == server_id && search.open)?;
    let accounts = account_names(server_id, model, known);
    let name_of = |account_id: AccountId| {
        accounts
            .get(&account_id)
            .cloned()
            .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
    };
    let (mut messages, mut transcripts) = (Vec::new(), Vec::new());
    for hit in &search.hits {
        let (place, channel) = place_of(hit.place_id, model, &accounts);
        if place == Whereabouts::Unknown {
            continue;
        }
        let local = known.zone.local(hit.created_at);
        let day = text::day_or_today(local.date(), known.today);
        let row = |time: String, target: ResultTarget| ResultRow {
            place: place.clone(),
            day: day.clone(),
            time,
            author_id: hit.author_id,
            author: name_of(hit.author_id),
            text: hit.text.clone(),
            marks: marks(&hit.text, &search.text),
            target,
        };
        match (hit.message_id, hit.recording_id, hit.line, channel) {
            (Some(message_id), _, _, _) => messages.push(row(
                format!("{:02}:{:02}", local.hour(), local.minute()),
                ResultTarget::Message(message_id),
            )),
            (None, Some(recording_id), Some(line), Some((workspace_id, channel_id))) => {
                transcripts.push(row(
                    text::transcript_line(line),
                    ResultTarget::Transcript {
                        workspace_id,
                        channel_id,
                        recording_id,
                        line,
                    },
                ));
            }
            _ => {}
        }
    }
    let found = !messages.is_empty() || !transcripts.is_empty();
    let state = match &search.progress {
        Progress::Searching => SearchShown::Searching,
        Progress::Refused(reason) => SearchShown::Refused(reason.clone()),
        Progress::Failed(reason) => SearchShown::Failed(reason.clone()),
        Progress::Done if found => SearchShown::Results,
        Progress::Done => SearchShown::Nothing,
    };
    let channels: Vec<(ChannelId, String, ChannelKind)> = workspace
        .channels
        .iter()
        .map(|channel| {
            (
                channel.channel.id,
                channel.channel.name.clone(),
                channel.channel.kind,
            )
        })
        .collect();
    let mut authors: Vec<(AccountId, String)> = workspace
        .members
        .loaded()
        .map(|members| {
            members
                .iter()
                .map(|member| (member.account_id, member.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    authors.sort_by(|one, other| {
        one.1
            .to_lowercase()
            .cmp(&other.1.to_lowercase())
            .then(one.0.cmp(&other.0))
    });
    Some(SearchPanel {
        server_id,
        text: search.text.clone(),
        state,
        messages,
        transcripts,
        more: search.more,
        can_read_more: search.more && search.hits.len() < MOST_HITS,
        reading_more: search.reading_more,
        channel: search.filters.channel.map(|channel_id| {
            let name = model
                .channel(channel_id)
                .map(|channel| channel.channel.name.clone())
                .unwrap_or_default();
            (channel_id, name)
        }),
        author: search
            .filters
            .author
            .map(|account_id| (account_id, name_of(account_id))),
        date: search.filters.date,
        channels,
        authors,
        going: search.going,
        go_failed: search.go_failed.clone(),
    })
}

#[cfg(test)]
mod tests;
