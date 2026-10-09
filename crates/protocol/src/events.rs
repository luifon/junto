//! The messages of the WebSocket event stream (ADR 0206, ADR 0207).
//!
//! Every message is one JSON object in a text frame, and its field `type` names it. A reader
//! ignores a field that it does not know and skips a message or an event whose `type` it does
//! not know.

use junto_authz::WorkspacePermission;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::account::{AccountStatus, AccountSummary, DeviceOrigin, RecoveryNotice};
use crate::ids::{
    AccountId, ChannelId, DeviceId, InviteId, JoinRequestId, ServerRoleId, StreamEpoch,
    WorkspaceId, WorkspaceRoleId,
};
use crate::invite::{Invite, InviteRevocation, JoinRequest, JoinRequestState};
use crate::keys::PublicKey;
use crate::roles::{ServerRole, WorkspaceRole};
use crate::structure::{Category, Channel, Workspace, WorkspaceMember};

/// The path of the event stream. A device opens it with its session token in the
/// `Authorization` header of the upgrade request.
pub const STREAM_PATH: &str = "/api/v1/events";

/// A place in the order of the events of one server. An event with a position at or below the
/// position of a cursor is covered by the state that the cursor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct Cursor {
    pub epoch: StreamEpoch,
    pub position: u64,
}

/// How much a connection receives (ADR 0107).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Every event that the account may see.
    Full,
    /// Direct messages, mentions, call invites and control events.
    Background,
}

/// What a client sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Starts or restarts the delivery after `cursor` at `level`. Without a cursor the
    /// delivery starts after the newest event.
    Subscribe {
        level: Level,
        #[serde(default)]
        cursor: Option<Cursor>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    /// The cursor belongs to another epoch or lies after the newest event.
    UnknownCursor,
    /// More events lie after the cursor than the server replays.
    TooFarBehind,
    /// The server could not send an event that the account may receive. The position of the
    /// connection is after that event.
    SkippedEvent,
}

/// Why the server ended the access of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Banned,
    Removed,
    DeviceRemoved,
    /// A recovery of the account ended every session of it.
    Recovered,
    /// The session was logged out or has expired.
    SessionEnded,
}

/// What the server sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The first message of every connection.
    Hello {
        server_version: String,
        api_versions: Vec<u32>,
        epoch: StreamEpoch,
        /// The server sends a ping at this interval and expects a frame from the client within
        /// `idle_seconds`.
        heartbeat_seconds: u32,
        idle_seconds: u32,
    },
    /// The answer to a `subscribe`. Events after `cursor` follow.
    Subscribed { level: Level, cursor: Cursor },
    Event {
        /// The position of the event. It grows from one event to the next; an event with an
        /// `id` at or below the last one that the client has applied is a repeat.
        id: u64,
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        event: Event,
    },
    /// The connection has received every event up to `cursor` that it may receive.
    Checkpoint { cursor: Cursor },
    /// The server does not continue from the cursor of the client: as the answer to a
    /// `subscribe`, or as the end of a subscription. The connection stays open and delivers
    /// nothing. Whatever the reason, the client discards every collection of this server
    /// that it holds, loads a snapshot and subscribes with the cursor of the snapshot. That
    /// includes the collections that the snapshot does not carry, such as members, accounts,
    /// devices, invites, join requests, server roles and channel overrides: the client loads
    /// them again from their routes when it needs them.
    Resync { reason: ResyncReason },
    /// The last message before the server closes a connection whose session no longer counts.
    Ended {
        reason: EndReason,
        /// Present with the reason `recovered`.
        recovery: Option<RecoveryNotice>,
    },
}

impl ServerMessage {
    pub const NAMES: &'static [&'static str] = &[
        "hello",
        "subscribed",
        "event",
        "checkpoint",
        "resync",
        "ended",
    ];
}

/// The code of the close frame with which the server ends a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum CloseCode {
    /// The server stops. The client connects again with its cursor.
    Stopping = 1001,
    /// A client message was larger than the limit.
    MessageTooBig = 1009,
    /// The server could not serve the connection. The client connects again with its cursor.
    Failed = 1011,
    /// The session no longer counts; an `ended` message came before.
    Ended = 4001,
    /// A newer connection of the same device took the place of this one.
    Replaced = 4002,
    /// The client did not take a message in time. It connects again with its cursor.
    Slow = 4003,
    /// The client sent no frame within the idle time, or no `subscribe` in time.
    Idle = 4004,
    /// A client message was not a known message.
    InvalidMessage = 4005,
    /// The client sent more messages than the limit.
    TooManyMessages = 4006,
}

impl CloseCode {
    pub const ALL: &'static [Self] = &[
        Self::Stopping,
        Self::MessageTooBig,
        Self::Failed,
        Self::Ended,
        Self::Replaced,
        Self::Slow,
        Self::Idle,
        Self::InvalidMessage,
        Self::TooManyMessages,
    ];

    pub const fn code(self) -> u16 {
        self as u16
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Stopping => "stopping",
            Self::MessageTooBig => "message_too_big",
            Self::Failed => "failed",
            Self::Ended => "ended",
            Self::Replaced => "replaced",
            Self::Slow => "slow",
            Self::Idle => "idle",
            Self::InvalidMessage => "invalid_message",
            Self::TooManyMessages => "too_many_messages",
        }
    }

    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.iter().copied().find(|known| known.code() == code)
    }
}

/// One change of the state of a server. New events are added; the fields of an existing event
/// are not changed or removed within one API version (ADR 0206).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A device was added to the account of the recipient.
    DeviceAdded {
        device_id: DeviceId,
        name: String,
        public_key: PublicKey,
        origin: DeviceOrigin,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    DeviceRenamed {
        device_id: DeviceId,
        name: String,
    },
    /// A device of the account was removed. Its sessions have ended.
    DeviceRemoved {
        device_id: DeviceId,
    },
    DeviceLoggedIn {
        device_id: DeviceId,
        #[serde(with = "time::serde::rfc3339")]
        logged_in_at: OffsetDateTime,
    },
    DeviceLoggedOut {
        device_id: DeviceId,
    },
    /// The server owner gave a new device access to the account of the recipient.
    AccountRecovered {
        recovery: RecoveryNotice,
    },
    /// What the recipient may see or do has changed: in one workspace, or on the server when
    /// no workspace is named. The client loads a snapshot.
    PermissionsChanged {
        workspace_id: Option<WorkspaceId>,
    },
    AccountAdded {
        account: AccountSummary,
    },
    AccountStatusChanged {
        account_id: AccountId,
        status: AccountStatus,
    },
    AccountServerRolesChanged {
        account_id: AccountId,
        server_role_ids: Vec<ServerRoleId>,
    },
    OwnerChanged {
        account_id: AccountId,
        previous_owner_id: Option<AccountId>,
    },
    ServerRoleCreated {
        role: ServerRole,
    },
    ServerRoleUpdated {
        role: ServerRole,
    },
    InviteCreated {
        invite: Invite,
    },
    InviteRevoked {
        invite_id: InviteId,
        revocation: InviteRevocation,
    },
    InviteUsed {
        invite_id: InviteId,
        uses: u32,
    },
    /// Every invite that the account created and that was not revoked is revoked now.
    InvitesOfCreatorRevoked {
        created_by: AccountId,
        revocation: InviteRevocation,
    },
    JoinRequestCreated {
        join_request: JoinRequest,
    },
    JoinRequestDecided {
        join_request_id: JoinRequestId,
        state: JoinRequestState,
        account_id: Option<AccountId>,
    },
    WorkspaceCreated {
        workspace: Workspace,
    },
    WorkspaceUpdated {
        workspace: Workspace,
    },
    WorkspaceMemberAdded {
        workspace_id: WorkspaceId,
        member: WorkspaceMember,
    },
    WorkspaceMemberRemoved {
        workspace_id: WorkspaceId,
        account_id: AccountId,
    },
    WorkspaceMemberRolesChanged {
        workspace_id: WorkspaceId,
        account_id: AccountId,
        role_ids: Vec<WorkspaceRoleId>,
    },
    WorkspaceRoleCreated {
        role: WorkspaceRole,
    },
    WorkspaceRoleUpdated {
        role: WorkspaceRole,
    },
    CategoryCreated {
        category: Category,
    },
    CategoryUpdated {
        category: Category,
    },
    ChannelCreated {
        channel: Channel,
    },
    ChannelUpdated {
        channel: Channel,
    },
    ChannelDeleted {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// The allow set and the deny set of one role in a channel. Two empty sets mean that the
    /// role has no override in the channel.
    ChannelOverrideSet {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        role_id: WorkspaceRoleId,
        allow: Vec<WorkspacePermission>,
        deny: Vec<WorkspacePermission>,
    },
}

impl Event {
    pub const NAMES: &'static [&'static str] = &[
        "device_added",
        "device_renamed",
        "device_removed",
        "device_logged_in",
        "device_logged_out",
        "account_recovered",
        "permissions_changed",
        "account_added",
        "account_status_changed",
        "account_server_roles_changed",
        "owner_changed",
        "server_role_created",
        "server_role_updated",
        "invite_created",
        "invite_revoked",
        "invite_used",
        "invites_of_creator_revoked",
        "join_request_created",
        "join_request_decided",
        "workspace_created",
        "workspace_updated",
        "workspace_member_added",
        "workspace_member_removed",
        "workspace_member_roles_changed",
        "workspace_role_created",
        "workspace_role_updated",
        "category_created",
        "category_updated",
        "channel_created",
        "channel_updated",
        "channel_deleted",
        "channel_override_set",
    ];

    pub const fn name(&self) -> &'static str {
        match self {
            Self::DeviceAdded { .. } => "device_added",
            Self::DeviceRenamed { .. } => "device_renamed",
            Self::DeviceRemoved { .. } => "device_removed",
            Self::DeviceLoggedIn { .. } => "device_logged_in",
            Self::DeviceLoggedOut { .. } => "device_logged_out",
            Self::AccountRecovered { .. } => "account_recovered",
            Self::PermissionsChanged { .. } => "permissions_changed",
            Self::AccountAdded { .. } => "account_added",
            Self::AccountStatusChanged { .. } => "account_status_changed",
            Self::AccountServerRolesChanged { .. } => "account_server_roles_changed",
            Self::OwnerChanged { .. } => "owner_changed",
            Self::ServerRoleCreated { .. } => "server_role_created",
            Self::ServerRoleUpdated { .. } => "server_role_updated",
            Self::InviteCreated { .. } => "invite_created",
            Self::InviteRevoked { .. } => "invite_revoked",
            Self::InviteUsed { .. } => "invite_used",
            Self::InvitesOfCreatorRevoked { .. } => "invites_of_creator_revoked",
            Self::JoinRequestCreated { .. } => "join_request_created",
            Self::JoinRequestDecided { .. } => "join_request_decided",
            Self::WorkspaceCreated { .. } => "workspace_created",
            Self::WorkspaceUpdated { .. } => "workspace_updated",
            Self::WorkspaceMemberAdded { .. } => "workspace_member_added",
            Self::WorkspaceMemberRemoved { .. } => "workspace_member_removed",
            Self::WorkspaceMemberRolesChanged { .. } => "workspace_member_roles_changed",
            Self::WorkspaceRoleCreated { .. } => "workspace_role_created",
            Self::WorkspaceRoleUpdated { .. } => "workspace_role_updated",
            Self::CategoryCreated { .. } => "category_created",
            Self::CategoryUpdated { .. } => "category_updated",
            Self::ChannelCreated { .. } => "channel_created",
            Self::ChannelUpdated { .. } => "channel_updated",
            Self::ChannelDeleted { .. } => "channel_deleted",
            Self::ChannelOverrideSet { .. } => "channel_override_set",
        }
    }
}

/// A server message as a client of this API version reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    Message(ServerMessage),
    /// An event of a type that this version does not know. The client moves its cursor to
    /// `id` and applies nothing.
    UnknownEvent {
        id: u64,
    },
    /// A message of a type that this version does not know. The client skips it.
    UnknownMessage,
}

/// Reads one text frame of the server. A type that this version does not know is not an
/// error (ADR 0206).
pub fn decode(text: &str) -> Result<Received, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    let named = |value: &serde_json::Value| value.get("type")?.as_str().map(str::to_owned);
    let Some(message_type) = named(&value) else {
        return serde_json::from_value(value).map(Received::Message);
    };
    if !ServerMessage::NAMES.contains(&message_type.as_str()) {
        return Ok(Received::UnknownMessage);
    }
    if message_type == "event" {
        let event_type = value.get("event").and_then(named);
        let id = value.get("id").and_then(serde_json::Value::as_u64);
        if let (Some(event_type), Some(id)) = (event_type, id)
            && !Event::NAMES.contains(&event_type.as_str())
        {
            return Ok(Received::UnknownEvent { id });
        }
    }
    serde_json::from_value(value).map(Received::Message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CategoryId, RecoveryId};
    use crate::structure::ChannelKind;

    fn time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    /// One event of every type.
    pub(crate) fn samples() -> Vec<Event> {
        let account_id = AccountId::generate();
        let device_id = DeviceId::generate();
        let workspace_id = WorkspaceId::generate();
        let channel_id = ChannelId::generate();
        let workspace = Workspace {
            id: workspace_id,
            name: "Acme".to_owned(),
            is_default: true,
        };
        let channel = Channel {
            id: channel_id,
            workspace_id,
            category_id: None,
            kind: ChannelKind::Text,
            name: "general".to_owned(),
            position: 0,
            permanent_messages: false,
        };
        let category = Category {
            id: CategoryId::generate(),
            workspace_id,
            name: "Projects".to_owned(),
            position: 0,
        };
        let workspace_role = WorkspaceRole {
            id: WorkspaceRoleId::generate(),
            workspace_id,
            name: "helpers".to_owned(),
            permissions: vec![WorkspacePermission::ViewChannel],
            is_everyone: false,
        };
        let server_role = ServerRole {
            id: ServerRoleId::generate(),
            name: "moderators".to_owned(),
            permissions: vec![junto_authz::ServerPermission::BanMembers],
        };
        let invite = Invite {
            id: InviteId::generate(),
            created_by: account_id,
            created_at: time(),
            expires_at: time(),
            max_uses: 1,
            uses: 0,
            workspace_ids: vec![workspace_id],
            revocation: None,
        };
        vec![
            Event::DeviceAdded {
                device_id,
                name: "desk".to_owned(),
                public_key: PublicKey::from_bytes([7; 32]),
                origin: DeviceOrigin::Link,
                created_at: time(),
            },
            Event::DeviceRenamed {
                device_id,
                name: "laptop".to_owned(),
            },
            Event::DeviceRemoved { device_id },
            Event::DeviceLoggedIn {
                device_id,
                logged_in_at: time(),
            },
            Event::DeviceLoggedOut { device_id },
            Event::AccountRecovered {
                recovery: RecoveryNotice {
                    id: RecoveryId::generate(),
                    device_id: Some(device_id),
                    performed_by: None,
                    performed_at: time(),
                },
            },
            Event::PermissionsChanged {
                workspace_id: Some(workspace_id),
            },
            Event::AccountAdded {
                account: AccountSummary {
                    id: account_id,
                    name: "Newcomer".to_owned(),
                    status: AccountStatus::Active,
                    is_owner: false,
                    server_role_ids: vec![],
                    created_at: time(),
                },
            },
            Event::AccountStatusChanged {
                account_id,
                status: AccountStatus::Banned,
            },
            Event::AccountServerRolesChanged {
                account_id,
                server_role_ids: vec![server_role.id],
            },
            Event::OwnerChanged {
                account_id,
                previous_owner_id: None,
            },
            Event::ServerRoleCreated {
                role: server_role.clone(),
            },
            Event::ServerRoleUpdated { role: server_role },
            Event::InviteCreated {
                invite: invite.clone(),
            },
            Event::InviteRevoked {
                invite_id: invite.id,
                revocation: InviteRevocation::Revoked,
            },
            Event::InviteUsed {
                invite_id: invite.id,
                uses: 1,
            },
            Event::InvitesOfCreatorRevoked {
                created_by: account_id,
                revocation: InviteRevocation::CreatorBanned,
            },
            Event::JoinRequestCreated {
                join_request: JoinRequest {
                    id: JoinRequestId::generate(),
                    name: "Newcomer".to_owned(),
                    created_at: time(),
                    expires_at: time(),
                },
            },
            Event::JoinRequestDecided {
                join_request_id: JoinRequestId::generate(),
                state: JoinRequestState::Approved,
                account_id: Some(account_id),
            },
            Event::WorkspaceCreated {
                workspace: workspace.clone(),
            },
            Event::WorkspaceUpdated { workspace },
            Event::WorkspaceMemberAdded {
                workspace_id,
                member: WorkspaceMember {
                    account_id,
                    name: "Newcomer".to_owned(),
                    role_ids: vec![],
                },
            },
            Event::WorkspaceMemberRemoved {
                workspace_id,
                account_id,
            },
            Event::WorkspaceMemberRolesChanged {
                workspace_id,
                account_id,
                role_ids: vec![workspace_role.id],
            },
            Event::WorkspaceRoleCreated {
                role: workspace_role.clone(),
            },
            Event::WorkspaceRoleUpdated {
                role: workspace_role.clone(),
            },
            Event::CategoryCreated {
                category: category.clone(),
            },
            Event::CategoryUpdated { category },
            Event::ChannelCreated {
                channel: channel.clone(),
            },
            Event::ChannelUpdated { channel },
            Event::ChannelDeleted {
                workspace_id,
                channel_id,
            },
            Event::ChannelOverrideSet {
                workspace_id,
                channel_id,
                role_id: workspace_role.id,
                allow: vec![],
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ]
    }

    #[test]
    fn every_event_type_has_one_name_that_is_its_wire_tag() {
        let samples = samples();
        let names: Vec<&str> = samples.iter().map(Event::name).collect();
        assert_eq!(names, Event::NAMES);
        for event in samples {
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["type"], event.name());
            assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
        }
    }

    #[test]
    fn server_messages_round_trip_and_carry_their_names() {
        let cursor = Cursor {
            epoch: StreamEpoch::generate(),
            position: 41,
        };
        let messages = [
            ServerMessage::Hello {
                server_version: "0.1.0".to_owned(),
                api_versions: vec![1],
                epoch: cursor.epoch,
                heartbeat_seconds: 25,
                idle_seconds: 60,
            },
            ServerMessage::Subscribed {
                level: Level::Background,
                cursor,
            },
            ServerMessage::Event {
                id: 42,
                at: time(),
                event: samples().remove(0),
            },
            ServerMessage::Checkpoint { cursor },
            ServerMessage::Resync {
                reason: ResyncReason::TooFarBehind,
            },
            ServerMessage::Ended {
                reason: EndReason::Banned,
                recovery: None,
            },
        ];
        let mut names = Vec::new();
        for message in messages {
            let text = serde_json::to_string(&message).unwrap();
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            names.push(json["type"].as_str().unwrap().to_owned());
            assert_eq!(decode(&text).unwrap(), Received::Message(message));
        }
        assert_eq!(names, ServerMessage::NAMES);
    }

    #[test]
    fn a_subscribe_without_a_cursor_starts_after_the_newest_event() {
        let message: ClientMessage =
            serde_json::from_str(r#"{"type":"subscribe","level":"full"}"#).unwrap();
        assert_eq!(
            message,
            ClientMessage::Subscribe {
                level: Level::Full,
                cursor: None
            }
        );
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"listen"}"#).is_err());
    }

    #[test]
    fn unknown_types_are_skipped_and_unknown_fields_are_ignored() {
        let unknown_event = r#"{"type":"event","id":9,"at":"2030-01-01T00:00:00Z",
            "event":{"type":"message_created","text":"hello"}}"#;
        assert_eq!(
            decode(unknown_event).unwrap(),
            Received::UnknownEvent { id: 9 }
        );
        assert_eq!(
            decode(r#"{"type":"notice","text":"hello"}"#).unwrap(),
            Received::UnknownMessage
        );
        let with_new_field = r#"{"type":"event","id":9,"at":"2030-01-01T00:00:00Z","new":1,
            "event":{"type":"device_removed","new":true,
                     "device_id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f"}}"#;
        assert!(matches!(
            decode(with_new_field).unwrap(),
            Received::Message(ServerMessage::Event { id: 9, .. })
        ));
        assert!(decode(r#"{"type":"event","id":9}"#).is_err());
        assert!(decode("[]").is_err());
    }

    #[test]
    fn close_codes_are_distinct_and_named() {
        let mut codes: Vec<u16> = CloseCode::ALL.iter().map(|code| code.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), CloseCode::ALL.len());
        for code in CloseCode::ALL {
            assert_eq!(CloseCode::from_code(code.code()), Some(*code));
            assert!(!code.name().is_empty());
        }
        assert_eq!(CloseCode::from_code(1000), None);
    }
}
