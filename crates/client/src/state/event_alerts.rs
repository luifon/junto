//! The system notifications of single events of a server that a person needs to know while
//! no window of the application is active (ADR 0742): the answer to the own request to
//! enter a call, a move or a removal by a moderator, a recording of the call, a recording,
//! a transcript or an export that finished, a device or a recovery of the account, an
//! addition to a workspace, a call that starts in a channel that notifies of every message,
//! and the answer to the own join request.

use junto_client_core::model::ServerModel;
use junto_client_core::protocol::call::{CallPlace, EntryOutcome, LeaveReason};
use junto_client_core::protocol::discovery::NotificationLevel;
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::export::ExportState;
use junto_client_core::protocol::ids::{
    AccountId, ChannelId, DeviceId, EntryRequestId, ServerId, WorkspaceId,
};
use junto_client_core::protocol::message::MessagePlace;
use junto_client_core::protocol::recording::RecordingState;
use junto_client_core::protocol::transcript::TranscriptState;
use time::OffsetDateTime;
use uuid::Uuid;

use super::account::AccountPage;
use super::admin::AdminPage;
use super::notices::{
    ALERT_TITLE_CHARACTERS, Alert, AlertTarget, Alerts, EXCERPT_CHARACTERS,
    MOST_ALERTS, Mute, Seen, account_names, channel_level, one_line, quiet_at,
};
use super::view::Known;
use crate::text;

/// What one event means to the person, when it is worth a system notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    /// The request of this device to enter a voice channel was answered.
    EntryAnswered {
        channel: Option<String>,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        outcome: EntryOutcome,
    },
    /// A moderator moves the person of this device into another voice channel.
    Moved {
        by: Option<String>,
        channel: Option<String>,
    },
    /// A moderator removed the person of this device from its call.
    Removed {
        channel: Option<String>,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// Another person started a recording of the call of this device.
    RecordingStarted {
        by: Option<String>,
        channel: Option<String>,
    },
    /// A recording that the person started finished.
    RecordingDone {
        channel: Option<String>,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        state: RecordingState,
    },
    /// The transcript of a recording that the person started finished.
    TranscriptDone {
        channel: Option<String>,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        state: TranscriptState,
    },
    /// An export of the person finished.
    ExportDone { place: String, ready: bool },
    /// Another device was added to the account.
    DeviceAdded { name: String },
    /// The owner of the server gave a new device access to the account.
    Recovered,
    /// The account of the person was added to a workspace.
    AddedToWorkspace {
        workspace: String,
        workspace_id: WorkspaceId,
    },
    /// A call started in a voice channel whose messages all notify the person.
    CallStarted {
        channel: String,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// The join request of this device was decided.
    JoinAnswered { approved: bool, server: String },
}

/// What the meaning of an event is read from beside the state of the server.
pub struct EventScene<'a> {
    pub server_id: ServerId,
    /// The state of the server after the event.
    pub model: &'a ServerModel,
    pub known: &'a Known,
    /// The device of this application on the server.
    pub own_device: Option<DeviceId>,
    /// The place of the call of this device on this server.
    pub call: Option<CallPlace>,
    /// The request of this device to enter a voice channel of this server, with its
    /// identifier on the server.
    pub entry: Option<(ChannelId, EntryRequestId)>,
}

/// What the rules of the system notification of a fact read.
pub struct TellScene<'a> {
    pub server_id: ServerId,
    pub server_name: &'a str,
    /// The device has more than one saved server: a notification names its server.
    pub several_servers: bool,
    /// The account is in its quiet time.
    pub quiet: bool,
    /// A window of the application is the active window of the system.
    pub active: bool,
    pub now: OffsetDateTime,
}

/// The quiet time of the account of a server now, when the state of the server is held.
pub fn quiet_now(model: Option<&ServerModel>, now: OffsetDateTime) -> bool {
    model.is_some_and(|model| quiet_at(model, now))
}

/// The name of a channel of the state.
fn channel_name(model: &ServerModel, channel_id: ChannelId) -> Option<String> {
    model
        .channel(channel_id)
        .map(|channel| channel.channel.name.clone())
}

fn name_of(scene: &EventScene<'_>, account_id: AccountId) -> Option<String> {
    account_names(scene.server_id, scene.model, scene.known)
        .get(&account_id)
        .cloned()
}

/// What an event means to the person, when it is worth a system notification. Every
/// other event, and an event that the person caused on this device, means nothing here.
pub fn fact_of(event: &Event, scene: &EventScene<'_>) -> Option<Fact> {
    let model = scene.model;
    let own = model.account.id;
    let in_call_at = |place: &CallPlace| scene.call.as_ref() == Some(place);
    match event {
        Event::CallEntryAnswered {
            request_id,
            outcome,
        } => {
            let (channel_id, held) = scene.entry?;
            if held != *request_id || *outcome == EntryOutcome::Cancelled {
                return None;
            }
            let workspace_id = model.channel(channel_id)?.channel.workspace_id;
            Some(Fact::EntryAnswered {
                channel: channel_name(model, channel_id),
                workspace_id,
                channel_id,
                outcome: *outcome,
            })
        }
        Event::CallMoveRequested { call_move } => {
            (call_move.account_id == own && in_call_at(&call_move.from)).then(|| Fact::Moved {
                by: name_of(scene, call_move.moved_by),
                channel: call_move
                    .to
                    .channel_id()
                    .and_then(|channel_id| channel_name(model, channel_id)),
            })
        }
        Event::OwnCallChanged {
            call: None,
            left: Some(LeaveReason::Disconnected),
        } => {
            let CallPlace::Channel {
                workspace_id,
                channel_id,
            } = scene.call?
            else {
                return None;
            };
            Some(Fact::Removed {
                channel: channel_name(model, channel_id),
                workspace_id,
                channel_id,
            })
        }
        Event::RecordingStarted { recording } => {
            let place = CallPlace::Channel {
                workspace_id: recording.workspace_id,
                channel_id: recording.channel_id,
            };
            (recording.started_by != own && in_call_at(&place)).then(|| Fact::RecordingStarted {
                by: name_of(scene, recording.started_by),
                channel: channel_name(model, recording.channel_id),
            })
        }
        Event::RecordingUpdated { recording } if recording.started_by == own => {
            let channel = channel_name(model, recording.channel_id);
            let (workspace_id, channel_id) = (recording.workspace_id, recording.channel_id);
            match recording.transcript {
                TranscriptState::Ready | TranscriptState::Partial | TranscriptState::Failed => {
                    return Some(Fact::TranscriptDone {
                        channel,
                        workspace_id,
                        channel_id,
                        state: recording.transcript,
                    });
                }
                TranscriptState::None | TranscriptState::Waiting | TranscriptState::Running => {}
            }
            matches!(
                recording.state,
                RecordingState::Ready | RecordingState::PartlyFailed | RecordingState::Failed
            )
            .then_some(Fact::RecordingDone {
                channel,
                workspace_id,
                channel_id,
                state: recording.state,
            })
        }
        Event::ExportUpdated { export } => {
            let ready = match export.state {
                ExportState::Ready => true,
                ExportState::Failed => false,
                ExportState::Waiting | ExportState::Running => return None,
            };
            let place = match export.place {
                MessagePlace::Channel { channel_id, .. } => channel_name(model, channel_id)
                    .map_or_else(|| text::A_CHANNEL.to_owned(), |name| format!("#{name}")),
                MessagePlace::Direct { .. } => text::A_DIRECT_CONVERSATION.to_owned(),
            };
            Some(Fact::ExportDone { place, ready })
        }
        Event::DeviceAdded {
            device_id, name, ..
        } => (Some(*device_id) != scene.own_device).then(|| Fact::DeviceAdded {
            name: name.clone(),
        }),
        Event::AccountRecovered { .. } => Some(Fact::Recovered),
        Event::WorkspaceMemberAdded {
            workspace_id,
            member,
        } if member.account_id == own => Some(Fact::AddedToWorkspace {
            workspace: model
                .workspace(*workspace_id)
                .map(|workspace| workspace.workspace.name.clone())
                .unwrap_or_default(),
            workspace_id: *workspace_id,
        }),
        Event::CallStarted {
            place:
                place @ CallPlace::Channel {
                    workspace_id,
                    channel_id,
                },
            ..
        } => {
            // Not for the person who is in that call, on this device or another one.
            let inside = in_call_at(place)
                || model
                    .own_call
                    .as_ref()
                    .is_some_and(|own_call| own_call.place == *place);
            let notifies = notifies_all(scene.server_id, *channel_id, model, scene.known);
            (!inside && notifies)
                .then(|| channel_name(model, *channel_id))
                .flatten()
                .map(|channel| Fact::CallStarted {
                    channel,
                    workspace_id: *workspace_id,
                    channel_id: *channel_id,
                })
        }
        _ => None,
    }
}

/// The key under which a fact is told at most once: a recording, a transcript or an export
/// is told when it finishes, whatever later changes of it the server sends.
fn key_of(fact: &Fact, event: &Event) -> Option<String> {
    match (fact, event) {
        (Fact::RecordingDone { .. }, Event::RecordingUpdated { recording }) => {
            Some(format!("recording:{}", recording.id))
        }
        (Fact::TranscriptDone { .. }, Event::RecordingUpdated { recording }) => {
            Some(format!("transcript:{}", recording.id))
        }
        (Fact::ExportDone { .. }, Event::ExportUpdated { export }) => {
            Some(format!("export:{}", export.id))
        }
        _ => None,
    }
}

/// The title and the target of a fact.
fn told(fact: &Fact, server_id: ServerId) -> (String, String, AlertTarget) {
    let channel = |name: &Option<String>| name.clone().unwrap_or_else(|| text::A_VOICE_CHANNEL.to_owned());
    let place = |workspace_id, channel_id| AlertTarget::Channel {
        server_id,
        workspace_id,
        channel_id,
    };
    match fact {
        Fact::EntryAnswered {
            channel: name,
            workspace_id,
            channel_id,
            outcome,
        } => (
            text::entry_answered_alert(*outcome, &channel(name)),
            String::new(),
            match outcome {
                EntryOutcome::Accepted => AlertTarget::Call(server_id),
                _ => place(*workspace_id, *channel_id),
            },
        ),
        Fact::Moved { by, channel: name } => (
            text::moved_alert(by.as_deref(), &channel(name)),
            String::new(),
            AlertTarget::Call(server_id),
        ),
        Fact::Removed {
            channel: name,
            workspace_id,
            channel_id,
        } => (
            text::removed_alert(&channel(name)),
            String::new(),
            place(*workspace_id, *channel_id),
        ),
        Fact::RecordingStarted { by, channel: name } => (
            text::recording_started_alert(by.as_deref(), &channel(name)),
            String::new(),
            AlertTarget::Call(server_id),
        ),
        Fact::RecordingDone {
            channel: name,
            workspace_id,
            channel_id,
            state,
        } => (
            text::recording_done_alert(*state, &channel(name)),
            String::new(),
            place(*workspace_id, *channel_id),
        ),
        Fact::TranscriptDone {
            channel: name,
            workspace_id,
            channel_id,
            state,
        } => (
            text::transcript_done_alert(*state, &channel(name)),
            String::new(),
            place(*workspace_id, *channel_id),
        ),
        Fact::ExportDone { place: name, ready } => (
            text::export_done_alert(*ready, name),
            String::new(),
            AlertTarget::Admin(server_id, AdminPage::Exports),
        ),
        Fact::DeviceAdded { name } => (
            text::DEVICE_ADDED_ALERT.to_owned(),
            name.clone(),
            AlertTarget::Account(server_id, AccountPage::Devices),
        ),
        Fact::Recovered => (
            text::RECOVERED_ALERT.to_owned(),
            String::new(),
            AlertTarget::Account(server_id, AccountPage::Recoveries),
        ),
        Fact::AddedToWorkspace {
            workspace,
            workspace_id,
        } => (
            text::added_to_workspace_alert(workspace),
            String::new(),
            AlertTarget::Workspace {
                server_id,
                workspace_id: *workspace_id,
            },
        ),
        Fact::CallStarted {
            channel: name,
            workspace_id,
            channel_id,
        } => (
            text::call_started_alert(name),
            String::new(),
            place(*workspace_id, *channel_id),
        ),
        Fact::JoinAnswered { approved, server } => (
            text::join_answered_alert(*approved, server),
            String::new(),
            AlertTarget::Server(server_id),
        ),
    }
}

/// The tag of the system notification of a fact.
pub fn event_tag(server_id: ServerId, id: Uuid) -> String {
    format!("event:{server_id}:{id}")
}

/// The system notification of a fact, under the rules of ADR 0742: none in the quiet time
/// of the account or while a window of the application is active, at most
/// [`MOST_ALERTS`] of one server within [`super::notices::ALERT_SPAN`], and a fact with a
/// key at most once. The application remembers a bounded number of them; one that it
/// forgets is taken away.
pub fn event_alert(
    seen: &mut Seen,
    scene: &TellScene<'_>,
    fact: &Fact,
    event: Option<&Event>,
) -> Alerts {
    let mut alerts = Alerts::default();
    if let Some(key) = event.and_then(|event| key_of(fact, event))
        && !seen.tell_once(key)
    {
        return alerts;
    }
    if scene.quiet || scene.active || !seen.room_for_alert(scene.now) {
        return alerts;
    }
    let (title, body, target) = told(fact, scene.server_id);
    let body = match (body.is_empty(), scene.several_servers) {
        (true, true) => scene.server_name.to_owned(),
        (false, true) => format!("{body} · {}", scene.server_name),
        (_, false) => body,
    };
    let tag = event_tag(scene.server_id, Uuid::now_v7());
    alerts
        .dismiss
        .extend(seen.event_shown(tag.clone(), target, scene.now));
    alerts.show.push((
        Alert {
            tag,
            title: one_line(&title, ALERT_TITLE_CHARACTERS),
            body: one_line(&body, EXCERPT_CHARACTERS),
        },
        target,
    ));
    alerts
}

/// The answer to the join request of this device: the state of the access went from a
/// waiting request to an account (approved), or to no account with a notice (refused).
pub fn join_answer(
    waited: bool,
    now_logged_in: bool,
    now_refused: bool,
    server: &str,
) -> Option<Fact> {
    if !waited || !(now_logged_in || now_refused) {
        return None;
    }
    Some(Fact::JoinAnswered {
        approved: now_logged_in,
        server: server.to_owned(),
    })
}

/// The person gets a notice of every message of a channel: its level is "all messages"
/// and the channel is not muted.
fn notifies_all(
    server_id: ServerId,
    channel_id: ChannelId,
    model: &ServerModel,
    known: &Known,
) -> bool {
    channel_level(server_id, channel_id, model, known) == NotificationLevel::All
        && !Mute::of_channel(channel_id, model, known).is_on()
}

#[cfg(test)]
mod tests;
