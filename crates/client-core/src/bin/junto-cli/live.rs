//! The commands that show the state of a server as the event stream keeps it, and the
//! actions of calls (ADR 0719, ADR 0510). They are the manual test tools of the event
//! stream and of the call routes until a window exists.

use std::io::{BufRead, Write};
use std::str::FromStr;
use std::time::Duration;

use junto_client_core::api::{ServerApi, code_name};
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::{LiveView, Notice, StreamFault, StreamStatus};
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::call::{
    Call, CallInvitation, CallJoin, CallLock, CallMove, CallParticipant, CallPlace, EntryRequest,
    Meeting, ShareAudio, UpdateCallSettings,
};
use junto_client_core::protocol::discovery::NotificationLevel;
use junto_client_core::protocol::events::{Cursor, Event};
use junto_client_core::protocol::ids::{
    AccountId, CallId, CallInvitationId, CallMoveId, ChannelId, EntryRequestId,
};
use junto_client_core::protocol::message::MessagePlace;
use junto_client_core::protocol::recording::Recording;
use junto_client_core::protocol::structure::{Channel, ChannelKind};
use junto_client_core::{Client, ServerHandle};
use time::OffsetDateTime;
use tokio::sync::mpsc;

use super::{Arguments, Failure, end_session, failed, logged_in, shown, usage};

/// How long `state` waits for the event stream to deliver.
const LIVE_WAIT: Duration = Duration::from_secs(30);
/// The longest line of standard input that `watch` takes as an action.
const LONGEST_LINE: usize = 1024;

/// What joins a call: each gives the token for the media server.
enum Entry {
    Channel(ChannelId),
    Invitation(CallInvitationId),
    Move(CallMoveId),
    Direct(AccountId),
}

#[derive(Clone, Copy)]
enum Moderation {
    BlockMicrophone,
    UnblockMicrophone,
    BlockSound,
    UnblockSound,
    Disconnect,
}

/// One action of `call`, read from the words of the command before any request.
enum CallAction {
    Show(Option<ChannelId>),
    Join {
        entry: Entry,
        show_token: bool,
    },
    Leave,
    State {
        muted: bool,
        deafened: bool,
    },
    Share(ShareAudio),
    Unshare,
    Hand(bool),
    Speaking(bool),
    Settings {
        channel_id: ChannelId,
        change: UpdateCallSettings,
    },
    Lock(ChannelId, bool),
    Meeting(ChannelId, bool),
    Ask(ChannelId),
    Answer(EntryRequestId, bool),
    CancelRequest(EntryRequestId),
    Invite(ChannelId, AccountId),
    Refuse(CallInvitationId),
    CancelInvitation(CallInvitationId),
    Move {
        channel_id: ChannelId,
        account_id: AccountId,
        to: ChannelId,
    },
    FailMove(CallMoveId),
    Moderate {
        channel_id: ChannelId,
        account_id: AccountId,
        moderation: Moderation,
    },
    RefuseDirect(AccountId),
    MuteDirect(AccountId, Option<u32>),
    UnmuteDirect(AccountId),
}

fn identifier<T: FromStr>(words: &[String], position: usize, what: &str) -> Result<T, Failure> {
    words
        .get(position)
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| usage(format!("the action needs the identifier of {what}")))
}

fn choice(
    words: &[String],
    position: usize,
    on: &str,
    off: &str,
    what: &str,
) -> Result<bool, Failure> {
    match words.get(position).map(String::as_str) {
        Some(word) if word == on => Ok(true),
        Some(word) if word == off => Ok(false),
        _ => Err(usage(format!("{what} is `{on}` or `{off}`"))),
    }
}

/// Reads the action of `call` from `words`: the name of the action and its arguments.
fn parse_call(words: &[String], arguments: &Arguments) -> Result<CallAction, Failure> {
    let Some(action) = words.first().map(String::as_str) else {
        return Err(usage("call needs an action"));
    };
    let show_token = arguments.flag("show-token");
    let channel = |position| identifier::<ChannelId>(words, position, "a channel");
    let account = |position| identifier::<AccountId>(words, position, "an account");
    let yes_no = |position, what| choice(words, position, "yes", "no", what);
    Ok(match action {
        "show" => match words.get(1) {
            Some(_) => CallAction::Show(Some(channel(1)?)),
            None => CallAction::Show(None),
        },
        "join" => CallAction::Join {
            entry: Entry::Channel(channel(1)?),
            show_token,
        },
        "accept" => CallAction::Join {
            entry: Entry::Invitation(identifier(words, 1, "an invitation")?),
            show_token,
        },
        "join-move" => CallAction::Join {
            entry: Entry::Move(identifier(words, 1, "a move")?),
            show_token,
        },
        "direct" => CallAction::Join {
            entry: Entry::Direct(account(1)?),
            show_token,
        },
        "leave" => CallAction::Leave,
        "state" => CallAction::State {
            muted: yes_no(1, "muted")?,
            deafened: yes_no(2, "deafened")?,
        },
        "share" => CallAction::Share(match words.get(1).map(String::as_str) {
            Some("application") => ShareAudio::Application,
            Some("system") => ShareAudio::System,
            Some("none") => ShareAudio::None,
            _ => {
                return Err(usage(
                    "the sound of a share is `application`, `system` or `none`",
                ));
            }
        }),
        "unshare" => CallAction::Unshare,
        "hand" => CallAction::Hand(choice(words, 1, "up", "down", "the hand")?),
        "speaking" => CallAction::Speaking(yes_no(1, "speaking")?),
        "settings" => {
            if arguments.value("limit").is_some() && arguments.flag("no-limit") {
                return Err(usage("--limit and --no-limit exclude each other"));
            }
            let user_limit = match arguments.number("limit")? {
                Some(limit) => Some(Some(limit)),
                None if arguments.flag("no-limit") => Some(None),
                None => None,
            };
            let hides_occupants = match arguments.value("hide") {
                Some("yes") => Some(true),
                Some("no") => Some(false),
                Some(_) => return Err(usage("--hide is `yes` or `no`")),
                None => None,
            };
            CallAction::Settings {
                channel_id: channel(1)?,
                change: UpdateCallSettings {
                    user_limit,
                    hides_occupants,
                },
            }
        }
        "lock" => CallAction::Lock(channel(1)?, true),
        "unlock" => CallAction::Lock(channel(1)?, false),
        "meeting" => CallAction::Meeting(channel(1)?, choice(words, 2, "on", "off", "meeting")?),
        "ask" => CallAction::Ask(channel(1)?),
        "answer" => CallAction::Answer(
            identifier(words, 1, "a request to enter")?,
            choice(words, 2, "accept", "deny", "the answer")?,
        ),
        "cancel-request" => CallAction::CancelRequest(identifier(words, 1, "a request to enter")?),
        "invite" => CallAction::Invite(channel(1)?, account(2)?),
        "refuse" => CallAction::Refuse(identifier(words, 1, "an invitation")?),
        "cancel-invitation" => CallAction::CancelInvitation(identifier(words, 1, "an invitation")?),
        "move" => CallAction::Move {
            channel_id: channel(1)?,
            account_id: account(2)?,
            to: channel(3)?,
        },
        "fail-move" => CallAction::FailMove(identifier(words, 1, "a move")?),
        "block-microphone" | "unblock-microphone" | "block-sound" | "unblock-sound"
        | "disconnect" => CallAction::Moderate {
            channel_id: channel(1)?,
            account_id: account(2)?,
            moderation: match action {
                "block-microphone" => Moderation::BlockMicrophone,
                "unblock-microphone" => Moderation::UnblockMicrophone,
                "block-sound" => Moderation::BlockSound,
                "unblock-sound" => Moderation::UnblockSound,
                _ => Moderation::Disconnect,
            },
        },
        "refuse-direct" => CallAction::RefuseDirect(account(1)?),
        "mute-direct" => CallAction::MuteDirect(account(1)?, arguments.number("hours")?),
        "unmute-direct" => CallAction::UnmuteDirect(account(1)?),
        other => return Err(usage(format!("unknown call action {other}"))),
    })
}

/// Sends the request of one action and returns the lines that the tool prints.
async fn perform(api: &ServerApi, action: CallAction) -> Result<Vec<String>, Failure> {
    let done = |text: &str| Ok(vec![text.to_owned()]);
    match action {
        CallAction::Show(None) => {
            let view = api.own_call().await.map_err(failed)?;
            let Some(state) = view.call else {
                return done("own call: none");
            };
            let mut lines = vec![
                format!("own call: {}", state.own.call_id),
                format!("place: {}", place(&state.own.place)),
                format!("device: {}", state.own.device_id),
                format!("identity: {}", shown(&state.own.identity)),
            ];
            lines.extend(call_lines(&state.call, ""));
            lines.extend(state.entry_requests.iter().map(entry_request_line));
            Ok(lines)
        }
        CallAction::Show(Some(channel_id)) => {
            let shown = api.channel_call(channel_id).await.map_err(failed)?;
            let mut lines = vec![format!("in use: {}", shown.in_use)];
            if let Some(call) = &shown.call {
                lines.extend(call_lines(call, ""));
            }
            Ok(lines)
        }
        CallAction::Join { entry, show_token } => {
            let join = match entry {
                Entry::Channel(channel_id) => api.join_channel_call(channel_id).await,
                Entry::Invitation(invitation_id) => api.accept_call_invitation(invitation_id).await,
                Entry::Move(move_id) => api.join_move(move_id).await,
                Entry::Direct(account_id) => api.join_direct_call(account_id).await,
            };
            Ok(join_lines(&join.map_err(failed)?, show_token))
        }
        CallAction::Leave => {
            api.leave_call().await.map_err(failed)?;
            done("left the call")
        }
        CallAction::State { muted, deafened } => {
            api.report_call_state(muted, deafened)
                .await
                .map_err(failed)?;
            Ok(vec![format!(
                "reported: muted {muted}, deafened {deafened}"
            )])
        }
        CallAction::Share(audio) => {
            api.report_screen_share(0, None, audio, None)
                .await
                .map_err(failed)?;
            Ok(vec![format!(
                "reported: screen share, sound {}",
                word(&audio)
            )])
        }
        CallAction::Unshare => {
            api.end_screen_share(None).await.map_err(failed)?;
            done("reported: no screen share")
        }
        CallAction::Hand(true) => {
            api.raise_hand().await.map_err(failed)?;
            done("hand: up")
        }
        CallAction::Hand(false) => {
            api.lower_hand().await.map_err(failed)?;
            done("hand: down")
        }
        CallAction::Speaking(speaking) => {
            api.report_speaking(speaking).await.map_err(failed)?;
            Ok(vec![format!("reported: speaking {speaking}")])
        }
        CallAction::Settings { channel_id, change } => {
            let channel = api
                .update_call_settings(channel_id, &change)
                .await
                .map_err(failed)?;
            let mut lines = vec![channel_line(&channel)];
            lines.extend(setting_lines(&channel, ""));
            Ok(lines)
        }
        CallAction::Lock(channel_id, true) => {
            api.lock_call(channel_id).await.map_err(failed)?;
            done("locked: true")
        }
        CallAction::Lock(channel_id, false) => {
            api.unlock_call(channel_id).await.map_err(failed)?;
            done("locked: false")
        }
        CallAction::Meeting(channel_id, true) => {
            api.start_meeting(channel_id).await.map_err(failed)?;
            done("meeting: on")
        }
        CallAction::Meeting(channel_id, false) => {
            api.end_meeting(channel_id).await.map_err(failed)?;
            done("meeting: off")
        }
        CallAction::Ask(channel_id) => {
            let request = api.request_entry(channel_id).await.map_err(failed)?;
            Ok(vec![entry_request_line(&request)])
        }
        CallAction::Answer(request_id, accept) => {
            api.answer_entry_request(request_id, accept)
                .await
                .map_err(failed)?;
            let answer = if accept { "accepted" } else { "denied" };
            Ok(vec![format!("entry request {request_id}: {answer}")])
        }
        CallAction::CancelRequest(request_id) => {
            api.cancel_entry_request(request_id).await.map_err(failed)?;
            Ok(vec![format!("entry request {request_id}: cancelled")])
        }
        CallAction::Invite(channel_id, account_id) => {
            let invitation = api
                .invite_to_call(channel_id, account_id)
                .await
                .map_err(failed)?;
            Ok(vec![invitation_line(&invitation)])
        }
        CallAction::Refuse(invitation_id) => {
            api.refuse_call_invitation(invitation_id)
                .await
                .map_err(failed)?;
            Ok(vec![format!("invitation {invitation_id}: refused")])
        }
        CallAction::CancelInvitation(invitation_id) => {
            api.cancel_call_invitation(invitation_id)
                .await
                .map_err(failed)?;
            Ok(vec![format!("invitation {invitation_id}: cancelled")])
        }
        CallAction::Move {
            channel_id,
            account_id,
            to,
        } => {
            let call_move = api
                .move_participant(channel_id, account_id, to)
                .await
                .map_err(failed)?;
            Ok(vec![move_line(&call_move)])
        }
        CallAction::FailMove(move_id) => {
            api.fail_move(move_id).await.map_err(failed)?;
            Ok(vec![format!("move {move_id}: reported as failed")])
        }
        CallAction::Moderate {
            channel_id,
            account_id,
            moderation,
        } => {
            let (sent, result) = match moderation {
                Moderation::BlockMicrophone => (
                    api.block_microphone(channel_id, account_id).await,
                    "microphone block: set",
                ),
                Moderation::UnblockMicrophone => (
                    api.unblock_microphone(channel_id, account_id).await,
                    "microphone block: removed",
                ),
                Moderation::BlockSound => (
                    api.block_sound(channel_id, account_id).await,
                    "sound block: set",
                ),
                Moderation::UnblockSound => (
                    api.unblock_sound(channel_id, account_id).await,
                    "sound block: removed",
                ),
                Moderation::Disconnect => (
                    api.disconnect_participant(channel_id, account_id).await,
                    "disconnected",
                ),
            };
            sent.map_err(failed)?;
            Ok(vec![format!("account {account_id}: {result}")])
        }
        CallAction::RefuseDirect(account_id) => {
            api.refuse_direct_call(account_id).await.map_err(failed)?;
            Ok(vec![format!("direct call with {account_id}: refused")])
        }
        CallAction::MuteDirect(account_id, hours) => {
            let until =
                hours.map(|hours| OffsetDateTime::now_utc() + time::Duration::hours(hours.into()));
            api.mute_direct_conversation(account_id, until)
                .await
                .map_err(failed)?;
            Ok(vec![format!(
                "muted conversation: {account_id}  {}",
                mute_end(until)
            )])
        }
        CallAction::UnmuteDirect(account_id) => {
            api.unmute_direct_conversation(account_id)
                .await
                .map_err(failed)?;
            Ok(vec![format!("conversation with {account_id}: not muted")])
        }
    }
}

/// A value of the protocol as the server writes it.
fn word<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_owned(),
    }
}

fn place(place: &CallPlace) -> String {
    match place {
        CallPlace::Channel { channel_id, .. } => format!("channel {channel_id}"),
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => format!("direct {low_account_id} {high_account_id}"),
    }
}

fn mute_end(until: Option<OffsetDateTime>) -> String {
    match until {
        Some(until) => format!("until {until}"),
        None => "until it is turned off".to_owned(),
    }
}

fn participant_line(participant: &CallParticipant, speaking: Option<bool>, indent: &str) -> String {
    let microphone_block = match &participant.microphone_block {
        Some(block) => format!("by {} ({})", block.by, word(&block.reason)),
        None => "none".to_owned(),
    };
    let sound_block = match &participant.sound_block {
        Some(block) => format!("by {}", block.by),
        None => "none".to_owned(),
    };
    let screen_share = if participant.shares().is_empty() {
        "none".to_owned()
    } else {
        participant
            .shares()
            .iter()
            .map(|share| {
                let kind = share
                    .kind
                    .as_ref()
                    .map_or_else(|| "unknown".to_owned(), word);
                format!("slot {} {kind} sound {}", share.slot, word(&share.audio))
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let hand = if participant.hand_raised {
        "up"
    } else {
        "down"
    };
    let speaking = match speaking {
        Some(speaking) => format!("  speaking: {speaking}"),
        None => String::new(),
    };
    format!(
        "{indent}participant: {}  muted: {}  deafened: {}  microphone block: \
         {microphone_block}  sound block: {sound_block}  hand: {hand}  screen share: \
         {screen_share}{speaking}",
        participant.account_id, participant.muted, participant.deafened
    )
}

fn lock_line(lock: Option<&CallLock>, indent: &str) -> String {
    match lock {
        Some(lock) => format!("{indent}locked: true (by {})", lock.by),
        None => format!("{indent}locked: false"),
    }
}

fn meeting_line(meeting: Option<&Meeting>, indent: &str) -> String {
    match meeting {
        Some(meeting) => format!("{indent}meeting: on (presenter {})", meeting.presenter),
        None => format!("{indent}meeting: off"),
    }
}

fn recording_line(recording: Option<&Recording>, indent: &str) -> String {
    match recording {
        Some(recording) => format!(
            "{indent}recording: {}  state: {}  started by {}  transcript: {}",
            recording.id,
            word(&recording.state),
            recording.started_by,
            word(&recording.transcript)
        ),
        None => format!("{indent}recording: none"),
    }
}

fn started_line(started_at: Option<OffsetDateTime>, indent: &str) -> String {
    match started_at {
        Some(started_at) => format!("{indent}started: {started_at}"),
        None => format!("{indent}started: not yet"),
    }
}

fn call_lines(call: &Call, indent: &str) -> Vec<String> {
    let mut lines = vec![
        format!("{indent}call: {}", call.id),
        started_line(call.started_at, indent),
        lock_line(call.lock.as_ref(), indent),
        meeting_line(call.meeting.as_ref(), indent),
        recording_line(call.recording.as_ref(), indent),
    ];
    lines.extend(
        call.participants
            .iter()
            .map(|participant| participant_line(participant, None, indent)),
    );
    lines
}

fn join_lines(join: &CallJoin, show_token: bool) -> Vec<String> {
    let mut lines = call_lines(&join.call, "");
    lines.push(format!("media address: {}", shown(&join.media_url)));
    lines.push(format!("identity: {}", shown(&join.identity)));
    lines.push(format!("token expires: {}", join.token_expires_at));
    lines.push(if show_token {
        format!("token: {}", shown(join.token.expose()))
    } else {
        "token: not shown (--show-token prints it)".to_owned()
    });
    lines.extend(join.entry_requests.iter().map(entry_request_line));
    lines
}

fn invitation_line(invitation: &CallInvitation) -> String {
    format!(
        "invitation: {}  {}  from {}  to {}  until {}",
        invitation.id,
        place(&invitation.place),
        invitation.invited_by,
        invitation.account_id,
        invitation.expires_at
    )
}

fn entry_request_line(request: &EntryRequest) -> String {
    format!(
        "entry request: {}  {}  from {}  until {}",
        request.id,
        place(&request.place),
        request.account_id,
        request.expires_at
    )
}

fn move_line(call_move: &CallMove) -> String {
    format!(
        "move: {}  account {}  from {}  to {}  by {}  until {}",
        call_move.id,
        call_move.account_id,
        place(&call_move.from),
        place(&call_move.to),
        call_move.moved_by,
        call_move.expires_at
    )
}

fn channel_line(channel: &Channel) -> String {
    format!(
        "channel: {}  {}  {}",
        channel.id,
        word(&channel.kind),
        shown(&channel.name)
    )
}

fn setting_lines(channel: &Channel, indent: &str) -> Vec<String> {
    let limit = match channel.user_limit {
        Some(limit) => limit.to_string(),
        None => "none".to_owned(),
    };
    vec![
        format!("{indent}user limit: {limit}"),
        format!("{indent}hides who is inside: {}", channel.hides_occupants),
    ]
}

/// The state of a server, one fact per line.
fn state_lines(model: &ServerModel) -> Vec<String> {
    let mut lines = vec![
        format!("account: {}", model.account.id),
        format!("name: {}", shown(&model.account.name)),
        format!("owner: {}", model.account.is_owner),
    ];
    for workspace in &model.workspaces {
        lines.push(format!(
            "workspace: {}  {}",
            workspace.workspace.id,
            shown(&workspace.workspace.name)
        ));
        let permissions: Vec<&str> = workspace
            .permissions
            .iter()
            .map(|permission| permission.name())
            .collect();
        lines.push(format!("permissions: {}", permissions.join(", ")));
        for held in &workspace.channels {
            lines.push(channel_line(&held.channel));
            if held.channel.kind != ChannelKind::Voice {
                continue;
            }
            lines.push(format!("  in use: {}", held.in_use));
            lines.extend(setting_lines(&held.channel, "  "));
            let Some(call) = &held.call else {
                continue;
            };
            lines.push(lock_line(call.lock.as_ref(), "  "));
            lines.push(meeting_line(call.meeting.as_ref(), "  "));
            lines.push(recording_line(call.recording.as_ref(), "  "));
            lines.extend(call.participants.iter().map(|participant| {
                let speaking = call.speaking.contains(&participant.account_id);
                participant_line(participant, Some(speaking), "  ")
            }));
        }
    }
    lines.push(match &model.own_call {
        Some(own) => format!(
            "own call: {}  {}  device {}",
            own.call_id,
            place(&own.place),
            own.device_id
        ),
        None => "own call: none".to_owned(),
    });
    for call in &model.direct_calls {
        lines.push(format!("direct call: {}  {}", call.id, place(&call.place)));
        lines.push(started_line(call.started_at, "  "));
        lines.extend(call.participants.iter().map(|participant| {
            let speaking = call.speaking.contains(&participant.account_id);
            participant_line(participant, Some(speaking), "  ")
        }));
    }
    lines.extend(model.invitations.iter().map(invitation_line));
    lines.extend(model.entry_requests.iter().map(entry_request_line));
    lines.extend(model.moves.iter().map(move_line));
    lines.extend(model.direct_mutes.iter().map(|mute| {
        format!(
            "muted conversation: {}  {}",
            mute.account_id,
            mute_end(mute.until)
        )
    }));
    lines.extend(
        model
            .conversations
            .iter()
            .map(crate::messages::conversation_line),
    );
    lines.extend(model.message_states.iter().map(crate::messages::state_line));
    lines.extend(model.threads.iter().map(crate::discovery::thread_line));
    if model.threads_more {
        lines.push("more unread threads: yes".to_owned());
    }
    lines.extend(model.notices.iter().map(crate::discovery::notice_line));
    for held in &model.notification_settings {
        let scope = if held.scope_id.is_nil() {
            "account".to_owned()
        } else {
            held.scope_id.to_string()
        };
        lines.extend(crate::discovery::scoped_settings_lines(
            &scope,
            &held.settings,
        ));
    }
    lines.push(format!("cursor: {}", model.cursor.position));
    lines
}

fn fault_text(fault: StreamFault) -> String {
    match fault {
        StreamFault::Unreachable(_) => "no connection to the server".to_owned(),
        StreamFault::Refused(code) => format!("refused with the error {}", code_name(code)),
        StreamFault::Closed(Some(code)) => format!("closed by the server with the code {code}"),
        StreamFault::Closed(None) => "closed by the server".to_owned(),
        StreamFault::Silent => "no frame of the server in time".to_owned(),
        StreamFault::Unreadable => "a message of the server cannot be read".to_owned(),
        StreamFault::SessionEnded(Some(reason)) => {
            format!("the session ended: {}", word(&reason))
        }
        StreamFault::SessionEnded(None) => "the session ended".to_owned(),
    }
}

fn status_text(status: StreamStatus) -> String {
    match status {
        StreamStatus::Off => "off".to_owned(),
        StreamStatus::Connecting => "connecting".to_owned(),
        StreamStatus::Live => "live".to_owned(),
        StreamStatus::Interrupted(fault) => format!("interrupted ({})", fault_text(fault)),
    }
}

/// The identifiers that an event names, as `name=value` pairs.
fn event_details(event: &Event) -> String {
    let at =
        |call_id: &CallId, call_place: &CallPlace| format!("call={call_id} {}", pair(call_place));
    match event {
        Event::DeviceAdded { device_id, .. }
        | Event::DeviceRenamed { device_id, .. }
        | Event::DeviceRemoved { device_id }
        | Event::DeviceLoggedIn { device_id, .. }
        | Event::DeviceLoggedOut { device_id } => format!("device={device_id}"),
        Event::AccountRecovered { recovery } => format!("recovery={}", recovery.id),
        Event::PermissionsChanged { workspace_id } => match workspace_id {
            Some(workspace_id) => format!("workspace={workspace_id}"),
            None => "server".to_owned(),
        },
        Event::AccountAdded { account } => format!("account={}", account.id),
        Event::AccountProfileUpdated { profile } => format!("account={}", profile.id),
        Event::AccountDepartureChanged {
            account_id,
            departure,
        } => match departure {
            Some(departure) => format!("account={account_id} departure={}", word(departure)),
            None => format!("account={account_id} returned"),
        },
        Event::AccountStatusChanged { account_id, status } => {
            format!("account={account_id} status={}", word(status))
        }
        Event::AccountServerRolesChanged { account_id, .. } => format!("account={account_id}"),
        Event::OwnerChanged { account_id, .. } => format!("account={account_id}"),
        Event::ServerRoleCreated { role } | Event::ServerRoleUpdated { role } => {
            format!("role={} position={}", role.id, role.position)
        }
        Event::ServerRolePositionsShifted { positions } => {
            format!("roles={}", positions.len())
        }
        Event::WorkspaceRolePositionsShifted {
            workspace_id,
            positions,
        } => format!("workspace={workspace_id} roles={}", positions.len()),
        Event::NotificationsRead { until, .. } => format!("until={until}"),
        Event::ThreadFollowChanged {
            root_id, following, ..
        } => format!("thread={root_id} following={following}"),
        Event::NotificationDefaultChanged {
            workspace_id,
            level,
        } => format!(
            "workspace={workspace_id} level={}",
            match level {
                Some(NotificationLevel::All) => "all",
                Some(NotificationLevel::Mentions) => "mentions",
                Some(NotificationLevel::Nothing) => "nothing",
                None => "not_set",
            }
        ),
        Event::InviteCreated { invite } | Event::InviteChanged { invite } => {
            format!("invite={}", invite.id)
        }
        Event::InviteRevoked {
            invite_id,
            revocation,
        } => format!("invite={invite_id} revocation={}", word(revocation)),
        Event::InviteUsed { invite_id, uses } => format!("invite={invite_id} uses={uses}"),
        Event::InvitesOfCreatorRevoked {
            created_by,
            revocation,
        } => format!("account={created_by} revocation={}", word(revocation)),
        Event::JoinRequestCreated { join_request } => format!("request={}", join_request.id),
        Event::JoinRequestDecided {
            join_request_id,
            state,
            ..
        } => format!("request={join_request_id} state={}", word(state)),
        Event::JoinRequestWithdrawn { join_request_id } => format!("request={join_request_id}"),
        Event::WorkspaceCreated { workspace } | Event::WorkspaceUpdated { workspace } => {
            format!("workspace={}", workspace.id)
        }
        Event::WorkspaceMemberAdded {
            workspace_id,
            member,
        } => format!("workspace={workspace_id} account={}", member.account_id),
        Event::WorkspaceMemberRemoved {
            workspace_id,
            account_id,
        }
        | Event::WorkspaceMemberRolesChanged {
            workspace_id,
            account_id,
            ..
        } => format!("workspace={workspace_id} account={account_id}"),
        Event::WorkspaceRoleCreated { role } | Event::WorkspaceRoleUpdated { role } => {
            format!(
                "workspace={} role={} position={}",
                role.workspace_id, role.id, role.position
            )
        }
        Event::CategoryCreated { category } | Event::CategoryUpdated { category } => {
            format!(
                "workspace={} category={}",
                category.workspace_id, category.id
            )
        }
        Event::ChannelCreated { channel } | Event::ChannelUpdated { channel } => {
            format!("workspace={} channel={}", channel.workspace_id, channel.id)
        }
        Event::ChannelDeleted {
            workspace_id,
            channel_id,
        } => format!("workspace={workspace_id} channel={channel_id}"),
        Event::ChannelOverrideSet {
            workspace_id,
            channel_id,
            role_id,
            ..
        } => format!("workspace={workspace_id} channel={channel_id} role={role_id}"),
        Event::CallStarted { call_id, place, .. }
        | Event::CallEnded { call_id, place }
        | Event::CallLockChanged { call_id, place, .. } => at(call_id, place),
        Event::CallParticipantJoined {
            call_id,
            place,
            participant,
        }
        | Event::CallParticipantUpdated {
            call_id,
            place,
            participant,
        } => format!("{} account={}", at(call_id, place), participant.account_id),
        Event::CallParticipantLeft {
            call_id,
            place,
            account_id,
            reason,
        } => format!(
            "{} account={account_id} reason={}",
            at(call_id, place),
            word(reason)
        ),
        Event::CallMeetingStarted {
            call_id,
            place,
            meeting,
        } => format!("{} presenter={}", at(call_id, place), meeting.presenter),
        Event::CallMeetingEnded {
            call_id,
            place,
            reason,
        } => format!("{} reason={}", at(call_id, place), word(reason)),
        Event::CallSpeaking {
            call_id,
            place,
            account_ids,
        } => {
            let speakers: Vec<String> = account_ids.iter().map(ToString::to_string).collect();
            format!("{} speaking={}", at(call_id, place), speakers.join(","))
        }
        Event::RecordingStarted { recording } | Event::RecordingUpdated { recording } => format!(
            "recording={} call={} channel={} state={}",
            recording.id,
            recording.call_id,
            recording.channel_id,
            word(&recording.state)
        ),
        Event::RecordingRemoved {
            recording_id,
            channel_id,
            ..
        } => format!("recording={recording_id} channel={channel_id}"),
        Event::TranscriptLineCorrected {
            recording_id,
            channel_id,
            line,
            ..
        } => format!("recording={recording_id} channel={channel_id} line={line}"),
        Event::TranscriptSpeakerRenamed {
            recording_id,
            channel_id,
            speaker,
            ..
        } => format!("recording={recording_id} channel={channel_id} speaker={speaker}"),
        Event::GlossaryReplaced { workspace_id } => format!("workspace={workspace_id}"),
        Event::CallEntryRequested { request } => format!(
            "request={} {} account={}",
            request.id,
            at(&request.call_id, &request.place),
            request.account_id
        ),
        Event::CallEntryResolved {
            request_id,
            call_id,
            place,
            account_id,
            outcome,
            ..
        } => format!(
            "request={request_id} {} account={account_id} outcome={}",
            at(call_id, place),
            word(outcome)
        ),
        Event::CallEntryAnswered {
            request_id,
            outcome,
        } => format!("request={request_id} outcome={}", word(outcome)),
        Event::CallInvited { invitation } => format!(
            "invitation={} {} from={} to={}",
            invitation.id,
            at(&invitation.call_id, &invitation.place),
            invitation.invited_by,
            invitation.account_id
        ),
        Event::CallInvitationResolved {
            invitation_id,
            outcome,
        } => format!("invitation={invitation_id} outcome={}", word(outcome)),
        Event::CallMoveRequested { call_move } => format!(
            "move={} account={} from_{} to_{} by={}",
            call_move.id,
            call_move.account_id,
            pair(&call_move.from),
            pair(&call_move.to),
            call_move.moved_by
        ),
        Event::CallMoveResolved { move_id, outcome } => {
            format!("move={move_id} outcome={}", word(outcome))
        }
        Event::OwnCallChanged { call, left } => match (call, left) {
            (Some(own), _) => format!("{} device={}", at(&own.call_id, &own.place), own.device_id),
            (None, Some(reason)) => format!("call=none left={}", word(reason)),
            (None, None) => "call=none".to_owned(),
        },
        Event::CallAccessEnded {} => String::new(),
        Event::DirectMuteSet { mute } => format!("account={}", mute.account_id),
        Event::DirectMuteRemoved { account_id } => format!("account={account_id}"),
        Event::ConversationOpened { conversation } => format!(
            "conversation={} accounts={},{}",
            conversation.id, conversation.low_account_id, conversation.high_account_id
        ),
        Event::ServerLogoChanged { logo } => match logo {
            Some(logo) => format!("logo={}", shown(&logo.hash)),
            None => "logo=none".to_owned(),
        },
        Event::AttachmentLimitsChanged { limits } => format!(
            "bytes_per_file={} files_per_message={}",
            limits.bytes_per_file, limits.files_per_message
        ),
        Event::MessageMentioned {
            channel_id,
            message_id,
            position,
            ..
        } => format!("channel={channel_id} message={message_id} position={position}"),
        Event::MessageChanged {
            place,
            message_id,
            position,
            deleted,
        } => format!(
            "{} message={message_id} position={position} deleted={deleted}",
            message_place(place)
        ),
        Event::MessageRead { place, position } => {
            format!("{} position={position}", message_place(place))
        }
        Event::ThreadRead { root_id, position } => {
            format!("thread={root_id} position={position}")
        }
        Event::NotificationChanged { message_id } => format!("message={message_id}"),
        Event::NotificationSettingsChanged {} => String::new(),
        Event::RetentionChanged { workspace_id } => match workspace_id {
            Some(workspace_id) => format!("workspace={workspace_id}"),
            None => "direct_messages".to_owned(),
        },
        Event::AccessTokenCreated { access_token } => format!(
            "token={} account={}",
            access_token.id, access_token.created_by
        ),
        Event::AccessTokenRevoked { access_token_id } => format!("token={access_token_id}"),
        Event::AccessTokensOfCreatorRevoked { created_by } => format!("account={created_by}"),
        Event::ChannelArchived { channel } | Event::ChannelDeletionDue { channel } => {
            let deletion = match channel.deletes_at {
                Some(at) => format!(" deletes_at={}", at.unix_timestamp()),
                None => String::new(),
            };
            format!(
                "workspace={} channel={}{deletion}",
                channel.channel.workspace_id, channel.channel.id
            )
        }
        Event::ChannelRestored { channel } => {
            format!("workspace={} channel={}", channel.workspace_id, channel.id)
        }
        Event::CategoryDeleted {
            workspace_id,
            category_id,
        } => format!("workspace={workspace_id} category={category_id}"),
        Event::WorkspaceRoleDeleted {
            workspace_id,
            role_id,
        } => format!("workspace={workspace_id} role={role_id}"),
        Event::ServerRoleDeleted { role_id } => format!("role={role_id}"),
        Event::WorkspaceDeleted { workspace_id } => format!("workspace={workspace_id}"),
        Event::ExportUpdated { export } => {
            format!("export={} state={}", export.id, word(&export.state))
        }
        Event::ExportRemoved { export_id, reason } => {
            format!("export={export_id} reason={}", word(reason))
        }
    }
}

fn message_place(place: &MessagePlace) -> String {
    match place {
        MessagePlace::Channel { channel_id, .. } => format!("channel={channel_id}"),
        MessagePlace::Direct {
            conversation_id, ..
        } => format!("conversation={conversation_id}"),
    }
}

fn pair(place: &CallPlace) -> String {
    match place {
        CallPlace::Channel { channel_id, .. } => format!("channel={channel_id}"),
        CallPlace::Direct {
            low_account_id,
            high_account_id,
        } => format!("direct={low_account_id},{high_account_id}"),
    }
}

fn notice_line(notice: &Notice) -> String {
    match notice {
        Notice::Event { id, event, .. } => {
            let line = format!("event {id} {} {}", event.name(), event_details(event));
            line.trim_end().to_owned()
        }
        Notice::Reloaded { cursor } => format!("reloaded {}", cursor.position),
        Notice::Ended { reason, .. } => format!("ended {}", word(reason)),
    }
}

/// Prints one line and gives it to the reader at once, also through a pipe.
pub(crate) fn say(line: &str) {
    let mut output = std::io::stdout().lock();
    let _ = writeln!(output, "{line}");
    let _ = output.flush();
}

fn text_of(failure: Failure) -> String {
    match failure {
        Failure::Usage(problem) | Failure::Failed(problem) => problem,
    }
}

/// Starts the event streams with this server as the selected one.
fn start(client: &Client, server: &ServerHandle) -> Result<(), Failure> {
    client.start_event_streams().map_err(failed)?;
    client.select(Some(server.id()));
    Ok(())
}

async fn live_view(server: &ServerHandle) -> Result<LiveView, Failure> {
    let live = server.live();
    let mut watch = live.watch();
    let delivers = |view: &LiveView| view.status == StreamStatus::Live && view.model.is_some();
    match tokio::time::timeout(LIVE_WAIT, watch.wait_for(delivers)).await {
        Ok(Some(view)) => Ok(view),
        _ => Err(Failure::Failed(format!(
            "the event stream does not deliver: its status is {}",
            status_text(live.view().status)
        ))),
    }
}

/// `state`: one read of the state of a server.
pub(crate) async fn state(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let server = logged_in(client, arguments).await?;
    let view = match start(client, &server) {
        Ok(()) => live_view(&server).await,
        Err(failure) => Err(failure),
    };
    end_session(&server).await;
    if let Some(model) = view?.model {
        for line in state_lines(&model) {
            say(&line);
        }
    }
    Ok(())
}

/// `call`: one action in one process.
pub(crate) async fn call(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let words = arguments.positional.get(1..).unwrap_or_default();
    let action = parse_call(words, arguments)?;
    let server = logged_in(client, arguments).await?;
    let lines = perform(&server.api(), action).await;
    end_session(&server).await;
    for line in lines? {
        say(&line);
    }
    Ok(())
}

/// The lines of standard input, read by a thread of its own.
fn input_lines() -> mpsc::UnboundedReceiver<String> {
    let (sender, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                return;
            };
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    receiver
}

async fn until(end: Option<tokio::time::Instant>) {
    match end {
        Some(end) => tokio::time::sleep_until(end).await,
        None => std::future::pending().await,
    }
}

/// `watch`: the state of a server, then every step of its event stream.
pub(crate) async fn watch(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let seconds = arguments.number("seconds")?;
    let server = logged_in(client, arguments).await?;
    let watched = watching(client, &server, seconds).await;
    end_session(&server).await;
    watched
}

async fn watching(
    client: &Client,
    server: &ServerHandle,
    seconds: Option<u32>,
) -> Result<(), Failure> {
    let live = server.live();
    // Both are taken before the streams start, so that no step is lost.
    let mut notices = live.notices();
    let mut views = live.watch();
    start(client, server)?;
    let api = server.api();
    let mut lines = input_lines();
    let end =
        seconds.map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds.into()));
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);

    let mut status = StreamStatus::Off;
    // The cursor of the state that was printed. The notices up to it are in that state.
    let mut printed: Option<Cursor> = None;
    let mut catching_up = true;
    loop {
        tokio::select! {
            view = views.changed() => {
                let Some(view) = view else {
                    return Ok(());
                };
                if view.status != status {
                    status = view.status;
                    say(&format!("status: {}", status_text(status)));
                }
                if let (None, StreamStatus::Live, Some(model)) = (printed, view.status, &view.model)
                {
                    for line in state_lines(model) {
                        say(&line);
                    }
                    printed = Some(model.cursor);
                }
                let logged_in = matches!(server.access().state, AccessState::LoggedIn { .. });
                if view.status == StreamStatus::Off && !logged_in {
                    return Err(Failure::Failed("this device is not logged in".to_owned()));
                }
            }
            notice = notices.next(), if printed.is_some() => {
                let notice = match notice {
                    Some(Ok(notice)) => notice,
                    Some(Err(missed)) => {
                        say(&format!("missed {} notices", missed.0));
                        continue;
                    }
                    None => return Ok(()),
                };
                let covered = match (&notice, printed) {
                    (Notice::Event { id, .. }, Some(cursor)) => *id <= cursor.position,
                    (Notice::Reloaded { cursor: loaded }, Some(cursor)) => {
                        loaded.epoch == cursor.epoch && loaded.position <= cursor.position
                    }
                    _ => false,
                };
                if catching_up && covered {
                    continue;
                }
                catching_up = false;
                say(&notice_line(&notice));
            }
            Some(line) = lines.recv() => {
                let line = line.trim();
                if line == "quit" {
                    return Ok(());
                }
                if line.is_empty() {
                    continue;
                }
                if line.len() > LONGEST_LINE {
                    say("error: the line is too long");
                    continue;
                }
                let words = line.split_whitespace().map(str::to_owned);
                let action = Arguments::parse(words)
                    .and_then(|arguments| parse_call(&arguments.positional, &arguments));
                let result = match action {
                    Ok(action) => perform(&api, action).await,
                    Err(failure) => Err(failure),
                };
                match result {
                    Ok(output) => output.iter().for_each(|line| say(line)),
                    Err(failure) => say(&format!("error: {}", text_of(failure))),
                }
            }
            _ = &mut interrupt => return Ok(()),
            () = until(end) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::account::{Avatar, ImageType};
    use junto_client_core::protocol::ids::{InviteId, WorkspaceId};
    use junto_client_core::protocol::invite::Invite;

    use super::*;

    #[test]
    fn the_details_of_an_event_name_a_changed_invite_and_the_logo() {
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let invite = Invite {
            id: InviteId::generate(),
            created_by: AccountId::generate(),
            created_by_name: None,
            created_at: at,
            expires_at: at,
            max_uses: 5,
            uses: 2,
            workspace_ids: vec![WorkspaceId::generate()],
            revocation: None,
        };
        let created = Event::InviteCreated {
            invite: invite.clone(),
        };
        let changed = Event::InviteChanged {
            invite: invite.clone(),
        };
        assert_eq!(changed.name(), "invite_changed");
        assert_eq!(event_details(&changed), format!("invite={}", invite.id));
        assert_eq!(event_details(&changed), event_details(&created));

        let logo = Avatar {
            hash: "ab12\u{1b}".to_owned(),
            media_type: ImageType::Png,
            bytes: 2048,
            width: 64,
            height: 64,
        };
        let set = Event::ServerLogoChanged { logo: Some(logo) };
        assert_eq!(set.name(), "server_logo_changed");
        assert_eq!(event_details(&set), "logo=ab12");
        let taken = Event::ServerLogoChanged { logo: None };
        assert_eq!(event_details(&taken), "logo=none");
    }
}
