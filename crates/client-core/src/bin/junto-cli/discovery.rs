//! `threads`, `pins`, `notices` and `search`: the routes of ADR 0409 from a terminal.

use std::path::PathBuf;

use junto_client_core::ServerHandle;
use junto_client_core::api::{PageQuery, SearchQuery, SettingsScope};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::discovery::{
    DailyQuiet, Notification, NotificationLevel, NotificationSettings, SearchHit,
};
use junto_client_core::protocol::ids::{ChannelId, MessageId, WorkspaceId};
use junto_client_core::protocol::message::{MessagePlace, SendMessage, ThreadState};
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::transfers::UploadSource;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::messages::{count, message_lines, plain};
use crate::{Arguments, Client, Failure, end_session, failed, logged_in, shown, usage};

pub(crate) fn word<'a>(
    words: &'a [String],
    position: usize,
    what: &str,
) -> Result<&'a str, Failure> {
    words
        .get(position)
        .map(String::as_str)
        .ok_or_else(|| usage(format!("{what} is missing")))
}

pub(crate) fn identifier<T: std::str::FromStr>(
    words: &[String],
    position: usize,
    what: &str,
) -> Result<T, Failure> {
    word(words, position, what)?
        .parse()
        .map_err(|_| usage(format!("{what} is not an identifier")))
}

fn whole(arguments: &Arguments, name: &str) -> Result<Option<u64>, Failure> {
    arguments
        .value(name)
        .map(|text| {
            text.parse()
                .map_err(|_| usage(format!("--{name} needs a whole number")))
        })
        .transpose()
}

/// `yes`, `no`, or with `inherit` allowed the absence of a value.
fn switch(arguments: &Arguments, name: &str) -> Result<Option<Option<bool>>, Failure> {
    arguments
        .value(name)
        .map(|text| match text {
            "yes" => Ok(Some(true)),
            "no" => Ok(Some(false)),
            "inherit" => Ok(None),
            _ => Err(usage(format!("--{name} takes yes, no or inherit"))),
        })
        .transpose()
}

fn level(text: &str) -> Result<NotificationLevel, Failure> {
    match text {
        "all" => Ok(NotificationLevel::All),
        "mentions" => Ok(NotificationLevel::Mentions),
        "nothing" => Ok(NotificationLevel::Nothing),
        _ => Err(usage("a level is all, mentions or nothing")),
    }
}

/// The level of a workspace for its members, which can be absent (ADR 0412).
fn default_word(level: Option<NotificationLevel>) -> &'static str {
    level.map_or("not set", level_word)
}

fn level_word(level: NotificationLevel) -> &'static str {
    match level {
        NotificationLevel::All => "all",
        NotificationLevel::Mentions => "mentions",
        NotificationLevel::Nothing => "nothing",
    }
}

/// A scope of notification settings as the tool takes it: `account`, `workspace:<id>` or
/// `channel:<id>`.
fn scope(text: &str) -> Result<SettingsScope, Failure> {
    let wrong = || usage("a scope is account, workspace:<id> or channel:<id>");
    match text.split_once(':') {
        None if text == "account" => Ok(SettingsScope::Account),
        Some(("workspace", id)) => id
            .parse::<WorkspaceId>()
            .map(SettingsScope::Workspace)
            .map_err(|_| wrong()),
        Some(("channel", id)) => id
            .parse::<ChannelId>()
            .map(SettingsScope::Channel)
            .map_err(|_| wrong()),
        _ => Err(wrong()),
    }
}

/// A time of day `HH:MM` as the minute of its day.
fn minute_of_day(text: &str) -> Option<u16> {
    let (hours, minutes) = text.split_once(':')?;
    let (hours, minutes): (u16, u16) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// A daily quiet time `HH:MM-HH:MM` on the clock of a named time zone (ADR 0412).
fn quiet(text: &str, time_zone: &str) -> Result<DailyQuiet, Failure> {
    let wrong = || usage("--quiet takes HH:MM-HH:MM with two different times");
    let (start, end) = text.split_once('-').ok_or_else(wrong)?;
    let (start_minute, end_minute) = minute_of_day(start)
        .zip(minute_of_day(end))
        .ok_or_else(wrong)?;
    if start_minute == end_minute {
        return Err(wrong());
    }
    Ok(DailyQuiet {
        start_minute,
        end_minute,
        time_zone: time_zone.to_owned(),
    })
}

pub(crate) fn time_of(
    arguments: &Arguments,
    name: &str,
) -> Result<Option<OffsetDateTime>, Failure> {
    arguments
        .value(name)
        .map(|text| {
            OffsetDateTime::parse(text, &Rfc3339).map_err(|_| {
                usage(format!(
                    "--{name} takes a time such as 2026-01-31T00:00:00Z"
                ))
            })
        })
        .transpose()
}

/// Applies the options of `notices set-settings` to the stored settings of a scope: the
/// server replaces the whole settings with what it is sent (ADR 0409).
fn changed_settings(
    mut settings: NotificationSettings,
    arguments: &Arguments,
    now: OffsetDateTime,
) -> Result<NotificationSettings, Failure> {
    if let Some(text) = arguments.value("level") {
        settings.level = match text {
            "inherit" => None,
            other => Some(level(other)?),
        };
    }
    if let Some(mute) = switch(arguments, "mute")? {
        settings.mute = mute.ok_or_else(|| usage("--mute takes yes or no"))?;
        settings.muted_until = None;
    }
    if let Some(hours) = arguments.number("mute-hours")? {
        settings.mute = true;
        settings.muted_until = Some(now + time::Duration::hours(hours.into()));
    }
    if let Some(role_mentions) = switch(arguments, "role-mentions")? {
        settings.role_mentions = role_mentions;
    }
    if let Some(everyone_mentions) = switch(arguments, "everyone-mentions")? {
        settings.everyone_mentions = everyone_mentions;
    }
    if let Some(dnd) = switch(arguments, "dnd")? {
        settings.do_not_disturb = dnd.ok_or_else(|| usage("--dnd takes yes or no"))?;
    }
    if arguments.flag("no-quiet") {
        settings.daily_quiet = None;
    }
    if let Some(text) = arguments.value("quiet") {
        // The server knows the names of the time zones and refuses another text.
        let zone = arguments.value("time-zone").unwrap_or("UTC");
        settings.daily_quiet = Some(quiet(text, zone)?);
    }
    // A mute that has ended is stored as a mute with a time in the past, which the server
    // refuses in a new request.
    if settings.muted_until.is_some_and(|until| until <= now) {
        settings.mute = false;
        settings.muted_until = None;
    }
    Ok(settings)
}

fn settings_lines(scope: SettingsScope, settings: &NotificationSettings) -> Vec<String> {
    let name = match scope {
        SettingsScope::Account => "account".to_owned(),
        SettingsScope::Workspace(workspace_id) => format!("workspace:{workspace_id}"),
        SettingsScope::Channel(channel_id) => format!("channel:{channel_id}"),
    };
    scoped_settings_lines(&name, settings)
}

pub(crate) fn scoped_settings_lines(scope: &str, settings: &NotificationSettings) -> Vec<String> {
    let inherited = |held: Option<bool>| match held {
        Some(true) => "yes",
        Some(false) => "no",
        None => "inherit",
    };
    let mute = match (settings.mute, settings.muted_until) {
        (false, _) => "no".to_owned(),
        (true, Some(until)) => format!("until {until}"),
        (true, None) => "until it is turned off".to_owned(),
    };
    let quiet = match &settings.daily_quiet {
        Some(quiet) => format!(
            "{:02}:{:02}-{:02}:{:02} in {}",
            quiet.start_minute / 60,
            quiet.start_minute % 60,
            quiet.end_minute / 60,
            quiet.end_minute % 60,
            shown(&quiet.time_zone)
        ),
        None => "none".to_owned(),
    };
    vec![
        format!(
            "notification settings: {scope}  level {}  mute {mute}",
            settings.level.map_or("inherit", level_word)
        ),
        format!(
            "  role mentions: {}  everyone mentions: {}",
            inherited(settings.role_mentions),
            inherited(settings.everyone_mentions)
        ),
        format!(
            "  do not disturb: {}  daily quiet time: {quiet}",
            settings.do_not_disturb
        ),
    ]
}

pub(crate) fn thread_line(state: &ThreadState) -> String {
    // The choice of the account for the thread, when it made one (ADR 0412).
    let choice = match state.follow_setting {
        Some(true) => " (chosen)",
        Some(false) => " (chosen not to)",
        None => "",
    };
    format!(
        "thread: {}  latest {}  read {}  unread {}  mentions {}  following {}{choice}",
        state.root_id,
        state.latest,
        state.last_read,
        count(state.unread),
        count(state.mentions),
        if state.following { "yes" } else { "no" }
    )
}

pub(crate) fn notice_line(notice: &Notification) -> String {
    let kind = match serde_json::to_value(notice.kind) {
        Ok(serde_json::Value::String(kind)) => kind,
        _ => "unknown".to_owned(),
    };
    let read = match notice.read_at {
        Some(at) => format!("read {at}"),
        None => "unread".to_owned(),
    };
    format!(
        "notice: {}  {kind}  message {}  {}  {read}",
        notice.id, notice.message_id, notice.created_at
    )
}

/// A time from the start of a recording, as hours, minutes and seconds.
fn line_time(start_ms: u64) -> String {
    let seconds = start_ms / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A result of a search: what it is, where it stands, and its text. A message states its
/// position in its place and the root of its thread, so that a reader opens the place at
/// the message; a transcript line states its time in the recording (ADR 0413).
fn hit_lines(hit: &SearchHit) -> Vec<String> {
    let mut what = match (hit.message_id, hit.recording_id, hit.line) {
        (Some(message_id), _, _) => format!("message {message_id}"),
        (None, Some(recording_id), Some(line)) => {
            format!("transcript of recording {recording_id} line {line}")
        }
        _ => "unknown".to_owned(),
    };
    if let Some(position) = hit.position {
        what.push_str(&format!("  position {position}"));
    }
    if let Some(root_id) = hit.thread_root {
        what.push_str(&format!("  thread {root_id}"));
    }
    if let Some(start_ms) = hit.start_ms {
        what.push_str(&format!("  at {}", line_time(start_ms)));
    }
    vec![
        format!(
            "result: {what}  place {}  author {}  {}  rank {}",
            hit.place_id, hit.author_id, hit.created_at, hit.rank
        ),
        format!("  {}", shown(&hit.text)),
    ]
}

async fn threads(
    server: &ServerHandle,
    words: &[String],
    arguments: &Arguments,
) -> Result<Vec<String>, Failure> {
    let api = server.api();
    Ok(match word(words, 0, "the action of threads")? {
        "list" => {
            let root_id: MessageId = identifier(words, 1, "the root message")?;
            let query = PageQuery {
                before: whole(arguments, "before")?,
                after: whole(arguments, "after")?,
                limit: arguments.number("limit")?,
                ..PageQuery::default()
            };
            let page = api.thread(root_id, &query).await.map_err(failed)?;
            let mut lines = vec!["root:".to_owned()];
            lines.extend(message_lines(&page.root));
            lines.push(format!("replies: {}", page.root.reply_count));
            lines.extend(page.replies.messages.iter().flat_map(message_lines));
            lines.push(format!("more: {}", page.replies.has_more));
            lines.extend(
                page.forks
                    .iter()
                    .map(|fork| format!("fork: channel {fork}")),
            );
            lines
        }
        "reply" => {
            let root_id: MessageId = identifier(words, 1, "the root message")?;
            let files: Vec<PathBuf> = arguments.values("file").iter().map(PathBuf::from).collect();
            let mut attachments = Vec::new();
            if !files.is_empty() {
                // A file of a reply is declared for the channel of the thread.
                let root = api.message(root_id).await.map_err(failed)?;
                let MessagePlace::Channel { channel_id, .. } = root.place else {
                    return Err(Failure::Failed("a thread belongs to a channel".to_owned()));
                };
                let place = PlaceId::Thread {
                    channel_id,
                    root_id,
                };
                let transfers = server.transfers();
                for file in files {
                    let upload = transfers.upload(place, UploadSource::File(file.clone()));
                    attachments.push(upload.finished().await.map_err(|failure| {
                        Failure::Failed(format!("{}: {failure:?}", file.display()))
                    })?);
                }
            }
            let message = SendMessage {
                document: plain(arguments.value("text").unwrap_or_default()),
                reply_to: arguments
                    .value("reply")
                    .map(|id| id.parse().map_err(|_| usage("--reply needs a message")))
                    .transpose()?,
                attachments,
            };
            message_lines(
                &api.reply_in_thread(root_id, &message)
                    .await
                    .map_err(failed)?,
            )
        }
        "state" => {
            let root_id = identifier(words, 1, "the root message")?;
            vec![thread_line(
                &api.thread_state(root_id).await.map_err(failed)?,
            )]
        }
        "read" => {
            let root_id = identifier(words, 1, "the root message")?;
            let position = word(words, 2, "the position")?
                .parse()
                .map_err(|_| usage("the position is a whole number"))?;
            vec![thread_line(
                &api.mark_thread_read(root_id, position)
                    .await
                    .map_err(failed)?,
            )]
        }
        "follow" | "unfollow" => {
            let root_id = identifier(words, 1, "the root message")?;
            let following = word(words, 0, "the action of threads")? == "follow";
            vec![thread_line(
                &api.set_thread_follow(root_id, following)
                    .await
                    .map_err(failed)?
                    .value,
            )]
        }
        "clear-follow" => {
            let root_id = identifier(words, 1, "the root message")?;
            api.clear_thread_follow(root_id).await.map_err(failed)?;
            vec!["the rule decides again whether the account follows the thread".to_owned()]
        }
        "followed" => {
            let channel_id: ChannelId = identifier(words, 1, "the channel")?;
            let page = api
                .followed_threads(
                    channel_id,
                    arguments.value("before"),
                    arguments.number("limit")?,
                )
                .await
                .map_err(failed)?;
            let mut lines: Vec<String> = page.threads.iter().map(thread_line).collect();
            lines.push(format!("more: {}", page.more));
            if let Some(next) = &page.next {
                lines.push(format!("continue with: --before {}", shown(next)));
            }
            lines
        }
        "unread" => {
            let page = api
                .unread_threads(arguments.value("before"), arguments.number("limit")?)
                .await
                .map_err(failed)?;
            let mut lines: Vec<String> = page.threads.iter().map(thread_line).collect();
            lines.push(format!("more: {}", page.more));
            if let Some(next) = &page.next {
                lines.push(format!("continue with: --before {}", shown(next)));
            }
            lines
        }
        "fork" => {
            let root_id = identifier(words, 1, "the root message")?;
            let name = arguments
                .value("name")
                .ok_or_else(|| usage("--name <name of the new channel> is missing"))?;
            let kind = match arguments.value("kind").unwrap_or("text") {
                "text" => ChannelKind::Text,
                "voice" => ChannelKind::Voice,
                _ => return Err(usage("--kind takes text or voice")),
            };
            let channel = api.fork_thread(root_id, name, kind).await.map_err(failed)?;
            vec![format!(
                "channel: {}  {}  workspace {}",
                channel.id,
                shown(&channel.name),
                channel.workspace_id
            )]
        }
        other => return Err(usage(format!("unknown action of threads: {other}"))),
    })
}

async fn pins(server: &ServerHandle, words: &[String]) -> Result<Vec<String>, Failure> {
    let api = server.api();
    let done = |()| vec!["done".to_owned()];
    Ok(match word(words, 0, "the action of pins")? {
        "list" => {
            let channel_id = identifier(words, 1, "the channel")?;
            let pins = api.pins(channel_id).await.map_err(failed)?;
            let mut lines: Vec<String> = pins.iter().flat_map(message_lines).collect();
            lines.push(format!("pins: {}", pins.len()));
            lines
        }
        "add" => {
            let message_id = identifier(words, 1, "the message")?;
            done(api.pin_message(message_id).await.map_err(failed)?)
        }
        "remove" => {
            let message_id = identifier(words, 1, "the message")?;
            done(api.unpin_message(message_id).await.map_err(failed)?)
        }
        other => return Err(usage(format!("unknown action of pins: {other}"))),
    })
}

async fn notices(
    server: &ServerHandle,
    words: &[String],
    arguments: &Arguments,
) -> Result<Vec<String>, Failure> {
    let api = server.api();
    Ok(match word(words, 0, "the action of notices")? {
        "list" => {
            let before: Option<Uuid> = arguments
                .value("before")
                .map(|text| text.parse().map_err(|_| usage("--before needs a notice")))
                .transpose()?;
            let page = api
                .notifications(before, arguments.number("limit")?)
                .await
                .map_err(failed)?;
            let mut lines: Vec<String> = page.notifications.iter().map(notice_line).collect();
            lines.push(format!("more: {}", page.has_more));
            lines
        }
        "read" => {
            let id: Uuid = identifier(words, 1, "the notice")?;
            vec![notice_line(
                &api.mark_notification_read(id).await.map_err(failed)?,
            )]
        }
        "settings" => {
            let scope = scope(word(words, 1, "the scope")?)?;
            let settings = api.notification_settings(scope).await.map_err(failed)?;
            settings_lines(scope, &settings)
        }
        "set-settings" => {
            let scope = scope(word(words, 1, "the scope")?)?;
            let stored = api.notification_settings(scope).await.map_err(failed)?;
            let wanted = changed_settings(stored, arguments, OffsetDateTime::now_utc())?;
            let settings = api
                .set_notification_settings(scope, &wanted)
                .await
                .map_err(failed)?
                .value;
            settings_lines(scope, &settings)
        }
        "default" => {
            let workspace_id = identifier(words, 1, "the workspace")?;
            let held = api
                .notification_default(workspace_id)
                .await
                .map_err(failed)?;
            vec![format!("default level: {}", default_word(held))]
        }
        "remove-default" => {
            let workspace_id = identifier(words, 1, "the workspace")?;
            api.remove_notification_default(workspace_id)
                .await
                .map_err(failed)?;
            vec!["default level: not set".to_owned()]
        }
        "read-all" => {
            let until = time_of(arguments, "until")?;
            let read = api
                .mark_notifications_read(until)
                .await
                .map_err(failed)?
                .value;
            vec![format!(
                "notices read: {}  until {}",
                read.marked, read.until
            )]
        }
        "set-default" => {
            let workspace_id = identifier(words, 1, "the workspace")?;
            let wanted = level(word(words, 2, "the level")?)?;
            let held = api
                .set_notification_default(workspace_id, wanted)
                .await
                .map_err(failed)?
                .value;
            vec![format!("default level: {}", default_word(held))]
        }
        other => return Err(usage(format!("unknown action of notices: {other}"))),
    })
}

async fn search(
    server: &ServerHandle,
    words: &[String],
    arguments: &Arguments,
) -> Result<Vec<String>, Failure> {
    let parse = |name: &str, what: &str| {
        arguments
            .value(name)
            .map(|text| {
                text.parse()
                    .map_err(|_| usage(format!("--{name} needs {what}")))
            })
            .transpose()
    };
    let query = SearchQuery {
        text: word(words, 0, "the text of the search")?.to_owned(),
        channel_id: parse("channel", "a channel")?,
        author_id: arguments
            .value("author")
            .map(|text| text.parse().map_err(|_| usage("--author needs an account")))
            .transpose()?,
        from: time_of(arguments, "from")?,
        until: time_of(arguments, "until")?,
        limit: arguments.number("limit")?,
        cursor: arguments.value("cursor").map(str::to_owned),
    };
    let page = server.api().search(&query).await.map_err(failed)?;
    let mut lines: Vec<String> = page.results.iter().flat_map(hit_lines).collect();
    lines.push(format!("results: {}", page.results.len()));
    if let Some(next) = &page.next {
        lines.push(format!("continue with: --cursor {}", shown(next)));
    }
    Ok(lines)
}

/// `threads`, `pins`, `notices` or `search`: one action in one process.
pub(crate) async fn run(
    client: &Client,
    command: &str,
    arguments: &Arguments,
) -> Result<(), Failure> {
    let words = arguments.positional.get(1..).unwrap_or_default();
    // An action that the tool does not know is told before the server is contacted.
    let known: &[&str] = match command {
        "threads" => &["list", "reply", "state", "read", "unread", "fork"],
        "pins" => &["list", "add", "remove"],
        "notices" => &[
            "list",
            "read",
            "settings",
            "set-settings",
            "default",
            "set-default",
        ],
        _ => &[],
    };
    if command != "search" {
        let action = word(words, 0, "the action")?;
        if !known.contains(&action) {
            return Err(usage(format!("unknown action of {command}: {action}")));
        }
    } else {
        word(words, 0, "the text of the search")?;
    }
    let server = logged_in(client, arguments).await?;
    let lines = match command {
        "threads" => threads(&server, words, arguments).await,
        "pins" => pins(&server, words).await,
        "notices" => notices(&server, words, arguments).await,
        _ => search(&server, words, arguments).await,
    };
    end_session(&server).await;
    for line in lines? {
        crate::live::say(&line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(words: &[&str]) -> Arguments {
        match Arguments::parse(words.iter().map(|word| (*word).to_owned())) {
            Ok(arguments) => arguments,
            Err(_) => panic!("the arguments are not read"),
        }
    }

    #[test]
    fn a_result_states_the_position_of_a_message_and_the_time_of_a_transcript_line() {
        use junto_client_core::protocol::ids::{AccountId, MessageId, RecordingId};
        let at = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let (message_id, root_id) = (MessageId::generate(), MessageId::generate());
        let message = SearchHit {
            message_id: Some(message_id),
            recording_id: None,
            line: None,
            place_id: uuid::Uuid::nil(),
            author_id: AccountId::generate(),
            created_at: at,
            text: "the obsidian plan".to_owned(),
            rank: 0.5,
            position: Some(41),
            thread_root: Some(root_id),
            start_ms: None,
        };
        let lines = hit_lines(&message);
        assert!(lines[0].starts_with(&format!(
            "result: message {message_id}  position 41  thread {root_id}  place "
        )));
        // A message of an earlier server states no position.
        let plain = SearchHit {
            position: None,
            thread_root: None,
            ..message.clone()
        };
        assert!(hit_lines(&plain)[0].starts_with(&format!("result: message {message_id}  place ")));

        let recording_id = RecordingId::generate();
        let spoken = SearchHit {
            message_id: None,
            recording_id: Some(recording_id),
            line: Some(7),
            position: None,
            thread_root: None,
            start_ms: Some(3_725_900),
            ..message
        };
        assert!(hit_lines(&spoken)[0].starts_with(&format!(
            "result: transcript of recording {recording_id} line 7  at 1:02:05  place "
        )));
        assert_eq!(line_time(0), "0:00");
        assert_eq!(line_time(59_999), "0:59");
        assert_eq!(line_time(61_000), "1:01");
        assert_eq!(line_time(3_600_000), "1:00:00");
    }

    #[test]
    fn a_scope_is_the_account_a_workspace_or_a_channel() {
        let (workspace_id, channel_id) = (WorkspaceId::generate(), ChannelId::generate());
        assert!(matches!(scope("account"), Ok(SettingsScope::Account)));
        assert!(matches!(
            scope(&format!("workspace:{workspace_id}")),
            Ok(SettingsScope::Workspace(id)) if id == workspace_id
        ));
        assert!(matches!(
            scope(&format!("channel:{channel_id}")),
            Ok(SettingsScope::Channel(id)) if id == channel_id
        ));
        assert!(scope("server").is_err());
        assert!(scope("channel:").is_err());
    }

    #[test]
    fn a_quiet_time_is_two_different_times_of_day() {
        let held = match quiet("22:30-07:00", "America/Sao_Paulo") {
            Ok(held) => held,
            Err(_) => panic!("the quiet time is not read"),
        };
        assert_eq!(
            (held.start_minute, held.end_minute, held.time_zone.as_str()),
            (22 * 60 + 30, 7 * 60, "America/Sao_Paulo")
        );
        assert!(quiet("07:00-07:00", "UTC").is_err());
        assert!(quiet("24:00-07:00", "UTC").is_err());
        assert!(quiet("7-8", "UTC").is_err());
    }

    #[test]
    fn the_options_change_the_stored_settings_and_keep_the_rest() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let stored = NotificationSettings {
            level: Some(NotificationLevel::All),
            role_mentions: Some(false),
            ..NotificationSettings::default()
        };
        let changed = changed_settings(
            stored.clone(),
            &arguments(&["--mute-hours", "8", "--everyone-mentions", "no"]),
            now,
        );
        let Ok(changed) = changed else {
            panic!("the options are not read");
        };
        assert_eq!(changed.level, Some(NotificationLevel::All));
        assert_eq!(changed.role_mentions, Some(false));
        assert_eq!(changed.everyone_mentions, Some(false));
        assert!(changed.mute);
        assert_eq!(changed.muted_until, Some(now + time::Duration::hours(8)));

        let inherited = changed_settings(changed, &arguments(&["--level", "inherit"]), now);
        assert!(matches!(inherited, Ok(held) if held.level.is_none() && held.mute));

        // A mute whose time has passed is not sent again: the server refuses it.
        let ended = NotificationSettings {
            mute: true,
            muted_until: Some(now - time::Duration::hours(1)),
            ..stored
        };
        let sent = changed_settings(ended, &arguments(&["--dnd", "yes"]), now);
        assert!(matches!(
            sent,
            Ok(held) if !held.mute && held.muted_until.is_none() && held.do_not_disturb
        ));
    }
}
