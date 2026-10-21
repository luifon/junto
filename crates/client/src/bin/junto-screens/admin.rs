//! The screens of the administration (ADR 0761) and of an archived channel in the main
//! area (ADR 0762): every page of a server and of the settings of a channel, with its
//! forms, its questions, and the states of its collections and of its requests.

use std::path::PathBuf;
use std::sync::Arc;

use junto_client::shell::Shell;
use junto_client::shell::admin::Mode;
use junto_client::shell::admin::people::Open as PeopleOpen;
use junto_client::shell::admin::records::AuditMenu;
use junto_client::state::admin::view::records::{AuditGroup, ExportSave, TargetKind};
use junto_client::state::admin::{
    AdminOpen, AdminPage, Ask, AskKind, AskState, AuditFilter, AuditLog, AuditPeriod, AuditScope,
    AuditTarget, ChannelPage, ExportFiles, Made,
};
use junto_client::state::feed::Zone;
use junto_client::state::view::ServerSnapshot;
use junto_client::text;
use junto_client_core::api::{ApiError, Rejection, TransportError, TransportErrorKind};
use junto_client_core::model::{
    AdminModel, Collection, GlossaryList, LoadFailure, Loadable, ServerModel, Timeline,
    WorkspaceSettings,
};
use junto_client_core::protocol::access_token::{AccessTokenInfo, TokenAction};
use junto_client_core::protocol::account::{AccountStatus, AccountSummary};
use junto_client_core::protocol::audit::{AuditEntry, AuditEvent};
use junto_client_core::protocol::discovery::{NotificationLevel, Retention};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::export::{Export, ExportFailure, ExportState};
use junto_client_core::protocol::ids::{WorkspaceId, WorkspaceRoleId};
use junto_client_core::protocol::invite::{Invite, InviteRevocation, JoinRequest};
use junto_client_core::protocol::message::MessagePlace;
use junto_client_core::protocol::recording::RecordingStopReason;
use junto_client_core::protocol::roles::ServerRole;
use junto_client_core::protocol::secret::{AccessToken, InviteCode};
use junto_client_core::protocol::structure::{
    ArchivePeriod, ArchivedChannel, Category, ChannelKind, ChannelOverride,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};
use time::{Duration, OffsetDateTime, UtcOffset};

use super::messages::{self, acme_with, none, prepare};
use super::{PEOPLE, Screen, Studio, at, channel, harbor, id, person};

// The numbers of the identifiers of "Acme Studio" (see `studio` and `acme_model`).
const ACME: u32 = 1;
const STUDIO: u32 = 10;
const CLIENT_ALPHA: u32 = 11;
const CLIENT_BETA: u32 = 12;
const EVERYONE: u32 = 20;
const LEADS: u32 = 21;
const ENGINEERS: u32 = 22;
const GENERAL: u32 = 30;
const ALPHA: u32 = 31;
const DEPLOYS: u32 = 32;
const RANDOM: u32 = 33;
const DAILY: u32 = 34;
const HR: u32 = 36;

// The numbers of what only the administration shows.
const DECISIONS: u32 = 37;
const ROADMAP: u32 = 38;
const MODERATORS: u32 = 50;
const INVITERS: u32 = 51;
const ADMINISTRATORS: u32 = 54;
const PROJECTS: u32 = 60;
const ROOMS: u32 = 61;
const INVITE: u32 = 600;
const REQUEST: u32 = 620;
const TOKEN: u32 = 640;
const EXPORT: u32 = 660;
const AUDIT: u32 = 9000;

/// A moment this number of days before the newest record of the screens, which is at
/// 10 October 2026, 10:04 UTC.
fn ago(days: i64) -> OffsetDateTime {
    at() - Duration::days(days)
}

pub(crate) fn collections(model: &mut ServerModel) -> &mut AdminModel {
    Arc::make_mut(&mut model.admin)
}

/// The two servers of the screens, with the state of "Acme Studio" changed.
pub(crate) fn servers(change: impl FnOnce(&mut ServerModel)) -> Vec<ServerSnapshot> {
    vec![acme_with(change), harbor()]
}

/// Luca owns the server.
fn owner(model: &mut ServerModel) {
    model.account.is_owner = true;
}

/// Luca is an administrator by the server role "Administrators", and another account
/// owns the server.
fn administrator(model: &mut ServerModel) {
    model.account.server_role_ids = vec![id(ADMINISTRATORS)];
    model.account.server_permissions = vec![ServerPermission::Administrator];
}

/// Luca holds the role "Leads" of "Studio", and that role gives these permissions.
fn lead(model: &mut ServerModel, permissions: &[WorkspacePermission]) {
    let studio = Arc::make_mut(&mut model.workspaces[0]);
    let leads: WorkspaceRoleId = id(LEADS);
    studio.own_role_ids = vec![leads];
    for role in &mut studio.roles {
        if role.id == leads {
            role.permissions = permissions.to_vec();
        }
    }
    if let Loadable::Loaded(members) = &mut studio.members {
        for member in members
            .iter_mut()
            .filter(|member| member.account_id == person("Luca"))
        {
            member.role_ids = vec![leads];
        }
    }
}

/// Gives "Studio" the categories "Projects" and "Rooms". `#general` and `#random` stay
/// without a category, `#deploys` has permanent messages, and "HR" has a user limit.
fn structure(model: &mut ServerModel) {
    let studio = Arc::make_mut(&mut model.workspaces[0]);
    let workspace_id = studio.workspace.id;
    studio.categories = vec![
        Category {
            id: id(PROJECTS),
            workspace_id,
            name: "Projects".to_owned(),
            position: 0,
        },
        Category {
            id: id(ROOMS),
            workspace_id,
            name: "Rooms".to_owned(),
            position: 1,
        },
    ];
    for held in &mut studio.channels {
        match held.channel.name.as_str() {
            "client-alpha" => held.channel.category_id = Some(id(PROJECTS)),
            "deploys" => {
                held.channel.category_id = Some(id(PROJECTS));
                held.channel.permanent_messages = true;
            }
            "Daily" | "Focus room" => held.channel.category_id = Some(id(ROOMS)),
            "HR" => {
                held.channel.category_id = Some(id(ROOMS));
                held.channel.user_limit = Some(8);
            }
            _ => {}
        }
    }
}

/// Every account of "Acme Studio": Elisa is banned, Diego is removed, and `owner` owns
/// the server.
pub(crate) fn accounts(owner: &str) -> Vec<AccountSummary> {
    PEOPLE
        .iter()
        .enumerate()
        .map(|(index, name)| AccountSummary {
            id: person(name),
            name: (*name).to_owned(),
            status: match *name {
                "Elisa" => AccountStatus::Banned,
                "Diego" => AccountStatus::Removed,
                _ => AccountStatus::Active,
            },
            is_owner: *name == owner,
            server_role_ids: match *name {
                "Bruno" => vec![id(MODERATORS), id(INVITERS)],
                "Carla" => vec![id(MODERATORS)],
                _ => Vec::new(),
            },
            created_at: ago(200 - 20 * index as i64),
        })
        .collect()
}

fn server_roles() -> Vec<ServerRole> {
    vec![
        ServerRole {
            position: 3,
            id: id(ADMINISTRATORS),
            name: "Administrators".to_owned(),
            permissions: vec![ServerPermission::Administrator],
        },
        ServerRole {
            position: 2,
            id: id(MODERATORS),
            name: "Moderators".to_owned(),
            permissions: vec![
                ServerPermission::BanMembers,
                ServerPermission::ApproveJoinRequests,
            ],
        },
        ServerRole {
            position: 1,
            id: id(INVITERS),
            name: "Inviters".to_owned(),
            permissions: vec![ServerPermission::CreateInvites],
        },
    ]
}

fn invite(number: u32, by: &str, created: OffsetDateTime, days: i64, uses: (u32, u32)) -> Invite {
    Invite {
        created_by_name: None,
        id: id(INVITE + number),
        created_by: person(by),
        created_at: created,
        expires_at: created + Duration::days(days),
        max_uses: uses.1,
        uses: uses.0,
        workspace_ids: Vec::new(),
        revocation: None,
    }
}

/// One invite in each state, the newest first. The page compares an expiry with the
/// date of the run: the invites that are not expired have the longest lifetime, so that
/// they keep their state for a year after `at()`.
fn invites() -> Vec<Invite> {
    let mut usable = invite(1, "Luca", ago(1), 365, (2, 25));
    usable.workspace_ids = vec![id(STUDIO), id(CLIENT_ALPHA)];
    let used_up = invite(2, "Ana", ago(3), 365, (5, 5));
    let mut revoked = invite(3, "Ana", ago(6), 365, (1, 5));
    revoked.revocation = Some(InviteRevocation::Revoked);
    // The state holds no workspace with the second identifier: the page counts it.
    revoked.workspace_ids = vec![id(STUDIO), id(99)];
    let mut creator_banned = invite(4, "Elisa", ago(9), 365, (3, 100));
    creator_banned.revocation = Some(InviteRevocation::CreatorBanned);
    let expired = invite(5, "Bruno", ago(40), 7, (0, 1));
    vec![usable, used_up, revoked, creator_banned, expired]
}

/// The join requests that wait, the oldest first.
fn join_requests() -> Vec<JoinRequest> {
    [("Marta", 3), ("Diego", 2), ("Elisa", 1)]
        .into_iter()
        .enumerate()
        .map(|(index, (name, days))| JoinRequest {
            id: id(REQUEST + index as u32),
            name: name.to_owned(),
            created_at: ago(days),
            expires_at: ago(days) + Duration::days(7),
        })
        .collect()
}

fn token(
    number: u32,
    name: &str,
    by: &str,
    actions: &[TokenAction],
    created: OffsetDateTime,
    days: i64,
) -> AccessTokenInfo {
    AccessTokenInfo {
        id: id(TOKEN + number),
        name: name.to_owned(),
        created_by: person(by),
        actions: actions.to_vec(),
        created_at: created,
        expires_at: created + Duration::days(days),
    }
}

/// The access tokens of the server, the newest first: two that work, and one that has
/// expired and holds an action that this build does not know. As with the invites, the
/// tokens that work have the longest lifetime.
fn access_tokens() -> Vec<AccessTokenInfo> {
    vec![
        token(
            1,
            "deploy-setup",
            "Luca",
            &[TokenAction::ReadStructure, TokenAction::ManageStructure],
            ago(0) - Duration::hours(6),
            366,
        ),
        token(
            2,
            "audit-reader",
            "Ana",
            &[TokenAction::ReadAuditLog],
            ago(20),
            366,
        ),
        token(
            3,
            "member-sync",
            "Luca",
            &[TokenAction::ManageMembers, TokenAction::Unknown],
            ago(40),
            30,
        ),
    ]
}

fn export(number: u32, place: MessagePlace, state: ExportState, hours: i64) -> Export {
    let requested_at = at() - Duration::hours(hours);
    let finished_at = (!matches!(state, ExportState::Waiting | ExportState::Running))
        .then(|| requested_at + Duration::minutes(6));
    Export {
        id: id(EXPORT + number),
        place,
        state,
        failure: None,
        requested_at,
        finished_at,
        expires_at: finished_at.map(|finished| finished + Duration::days(7)),
        bytes: None,
    }
}

/// The exports of Luca, the newest first: one that waits, one that the server writes
/// now, three that are ready, and one that failed.
fn exports() -> Vec<Export> {
    let (own, other) = (person("Luca"), person("Ana"));
    let direct = MessagePlace::Direct {
        conversation_id: id(8001),
        low_account_id: own.min(other),
        high_account_id: own.max(other),
    };
    let mut ready = export(3, messages::place(GENERAL), ExportState::Ready, 5);
    ready.bytes = Some(48_300_000);
    let mut saving = export(4, messages::place(DAILY), ExportState::Ready, 26);
    saving.bytes = Some(912_000_000);
    let mut saved = export(5, direct, ExportState::Ready, 48);
    saved.bytes = Some(2_100_000);
    let mut failed = export(6, messages::place(ALPHA), ExportState::Failed, 70);
    failed.failure = Some(ExportFailure::StorageLow);
    vec![
        export(1, messages::place(RANDOM), ExportState::Waiting, 0),
        export(2, messages::place(DEPLOYS), ExportState::Running, 1),
        ready,
        saving,
        saved,
        failed,
    ]
}

/// The files of the exports on this device: one that is saved now, and one that was
/// saved. The path names no directory of a real machine.
fn export_files() -> ExportFiles {
    ExportFiles::of([
        (
            (id(ACME), id(EXPORT + 4)),
            ExportSave::Saving {
                written: 310_000_000,
            },
        ),
        (
            (id(ACME), id(EXPORT + 5)),
            ExportSave::Saved(
                PathBuf::from("Downloads").join("junto-export-conversation-2026-10-08.zip"),
            ),
        ),
    ])
}

/// The archived channels of "Studio", in the order of their archiving. The server does
/// not delete the one with permanent messages.
fn archived() -> Vec<ArchivedChannel> {
    let mut decisions = channel(DECISIONS, id(STUDIO), ChannelKind::Text, "decisions-2025");
    decisions.permanent_messages = true;
    let roadmap_archived = messages::morning() + Duration::minutes(55);
    vec![
        ArchivedChannel {
            channel: decisions,
            archived_at: ago(40),
            deletes_at: None,
        },
        ArchivedChannel {
            channel: channel(ROADMAP, id(STUDIO), ChannelKind::Text, "old-roadmap"),
            archived_at: roadmap_archived,
            deletes_at: Some(roadmap_archived + Duration::days(90)),
        },
    ]
}

/// The archived list of "Studio" is read and holds these channels.
fn read_archived(model: &mut ServerModel, channels: Vec<ArchivedChannel>) {
    collections(model).archived = vec![(id(STUDIO), Collection::Loaded(channels))];
}

/// The archive period and the retention of "Studio", and the retention of the files of
/// direct messages, are read.
fn read_retention(model: &mut ServerModel) {
    let admin = collections(model);
    admin.settings = vec![(
        id(STUDIO),
        Collection::Loaded(WorkspaceSettings {
            archive_period: ArchivePeriod { days: Some(90) },
            file_retention: Retention { days: Some(365) },
            recording_retention: Retention { days: None },
        }),
    )];
    admin.direct_retention = Collection::Loaded(Retention { days: Some(180) });
}

/// The messages of the archived channel `#old-roadmap`.
fn roadmap_messages() -> Timeline {
    let mut made = messages::Made::new(ROADMAP);
    made.system(-30, "channel_created", Some("Ana"), None);
    made.message(
        "Ana",
        4,
        "The roadmap of the first half of the year is done: **export**, *retention* and \
         the recorder moved to the new storage.",
    );
    made.message(
        "Diego",
        12,
        "The open items moved to `#client-alpha`. Nothing is left here.",
    );
    made.message(
        "Luca",
        31,
        "Then this channel can be archived. The plan of the second half gets its own \
         channel.",
    );
    made.system(55, "channel_archived", Some("Ana"), None);
    made.timeline()
}

/// The override of the "everyone" role in a channel: one allow and one deny.
fn everyone_override() -> ChannelOverride {
    ChannelOverride {
        role_id: id(EVERYONE),
        allow: vec![WorkspacePermission::RecordCalls],
        deny: vec![WorkspacePermission::MentionEveryone],
    }
}

fn leads_override() -> ChannelOverride {
    ChannelOverride {
        role_id: id(LEADS),
        allow: vec![
            WorkspacePermission::ManageMessages,
            WorkspacePermission::MentionEveryone,
        ],
        deny: Vec::new(),
    }
}

/// The overrides of a private channel: the "everyone" role is denied "view channel",
/// and the role "Leads" is allowed it.
fn private_overrides() -> Vec<ChannelOverride> {
    vec![
        ChannelOverride {
            role_id: id(EVERYONE),
            allow: Vec::new(),
            deny: vec![WorkspacePermission::ViewChannel],
        },
        ChannelOverride {
            role_id: id(LEADS),
            allow: vec![WorkspacePermission::ViewChannel],
            deny: Vec::new(),
        },
    ]
}

/// The overrides of a channel are in this state.
fn set_overrides(
    model: &mut ServerModel,
    channel: u32,
    overrides: Collection<Vec<ChannelOverride>>,
) {
    collections(model).overrides = vec![(id(channel), overrides)];
}

/// The entries of one audit log, the newest first.
struct Log {
    workspace: Option<WorkspaceId>,
    entries: Vec<AuditEntry>,
}

impl Log {
    fn new(workspace: Option<u32>) -> Self {
        Self {
            workspace: workspace.map(id::<WorkspaceId>),
            entries: Vec::new(),
        }
    }

    /// Adds an entry of this number of minutes before the newest record. Without an
    /// actor the server itself acted.
    fn entry(&mut self, minutes: i64, actor: Option<&str>, event: AuditEvent) -> &mut AuditEntry {
        let number = self.entries.len();
        self.entries.push(AuditEntry {
            id: id(AUDIT + number as u32),
            occurred_at: at() - Duration::minutes(minutes),
            actor_id: actor.map(person),
            access_token_id: None,
            workspace_id: self.workspace,
            channel_id: None,
            event,
        });
        &mut self.entries[number]
    }
}

/// The audit log of the server: actions of several groups, one of the server itself,
/// one that was made with an access token, and one that this build does not know.
fn server_log() -> Vec<AuditEntry> {
    let (hour, day) = (60, 24 * 60);
    let mut log = Log::new(None);
    log.entry(
        12,
        Some("Luca"),
        AuditEvent::AccountBanned {
            recipient_message: None,
            reason: Some(
                "Sent the same advertisement to every channel after two warnings.".to_owned(),
            ),
            account_id: person("Elisa"),
            cancelled_invite_ids: vec![id(INVITE + 4)],
        },
    );
    log.entry(
        47,
        Some("Ana"),
        AuditEvent::InviteCreated {
            invite_id: id(INVITE + 2),
            expires_at: ago(3) + Duration::days(365),
            max_uses: 5,
            workspace_ids: vec![id(STUDIO), id(CLIENT_ALPHA)],
        },
    );
    log.entry(
        2 * hour,
        Some("Bruno"),
        AuditEvent::JoinRequestApproved {
            join_request_id: id(REQUEST + 9),
            account_id: person("Marta"),
            workspace_ids: vec![id(STUDIO)],
        },
    );
    log.entry(
        3 * hour,
        Some("Carla"),
        AuditEvent::AccountJoined {
            account_id: person("Carla"),
            invite_id: id(INVITE + 2),
            invite_created_by: person("Ana"),
        },
    );
    let renamed = log.entry(
        5 * hour,
        Some("Luca"),
        AuditEvent::WorkspaceRenamed {
            workspace_id: id(CLIENT_ALPHA),
            name: "Client Alpha".to_owned(),
        },
    );
    renamed.access_token_id = Some(id(TOKEN + 1));
    renamed.workspace_id = Some(id(CLIENT_ALPHA));
    log.entry(
        6 * hour,
        Some("Luca"),
        AuditEvent::AccessTokenCreated {
            access_token_id: id(TOKEN + 1),
            name: "deploy-setup".to_owned(),
            actions: vec![TokenAction::ReadStructure, TokenAction::ManageStructure],
            expires_at: ago(0) - Duration::hours(6) + Duration::days(366),
        },
    );
    log.entry(
        day,
        None,
        AuditEvent::OwnerCodeIssued {
            replaced_a_code: false,
            expires_at: Some(at()),
        },
    );
    log.entry(
        day + 2 * hour,
        Some("Luca"),
        AuditEvent::ServerRoleAssigned {
            role_id: id(MODERATORS),
            account_id: person("Bruno"),
        },
    );
    log.entry(
        day + 3 * hour,
        Some("Luca"),
        AuditEvent::ServerRoleCreated {
            role_id: id(MODERATORS),
            name: "Moderators".to_owned(),
            permissions: vec![
                ServerPermission::BanMembers,
                ServerPermission::ApproveJoinRequests,
            ],
        },
    );
    log.entry(2 * day, Some("Carla"), AuditEvent::Unknown);
    log.entry(
        2 * day + 4 * hour,
        Some("Ana"),
        AuditEvent::AccountRemoved {
            recipient_message: None,
            reason: None,
            account_id: person("Diego"),
        },
    );
    log.entry(
        3 * day,
        Some("Luca"),
        AuditEvent::WorkspaceCreated {
            workspace_id: id(CLIENT_BETA),
            name: "Client Beta".to_owned(),
        },
    )
    .workspace_id = Some(id(CLIENT_BETA));
    log.entry(
        3 * day + hour,
        Some("Luca"),
        AuditEvent::RetentionChanged { days: Some(180) },
    );
    log.entry(
        4 * day,
        Some("Ana"),
        AuditEvent::AttachmentLimitsChanged {
            bytes_per_file: 104_857_600,
            files_per_message: 10,
        },
    );
    log.entry(
        5 * day,
        Some("Bruno"),
        AuditEvent::JoinRequestDenied {
            join_request_id: id(REQUEST + 8),
        },
    );
    log.entries
}

/// The audit log of "Studio": actions about its channels, roles, calls and messages, one
/// of the server itself, and one about a channel that the state does not hold any more.
fn workspace_log() -> Vec<AuditEntry> {
    let (hour, day) = (60, 24 * 60);
    let mut log = Log::new(Some(STUDIO));
    log.entry(20, Some("Ana"), AuditEvent::ChannelArchived)
        .channel_id = Some(id(ROADMAP));
    log.entry(
        hour,
        Some("Ana"),
        AuditEvent::ChannelOverrideSet {
            role_id: id(LEADS),
            allow: vec![WorkspacePermission::ManageMessages],
            deny: Vec::new(),
        },
    )
    .channel_id = Some(id(GENERAL));
    log.entry(
        2 * hour + 20,
        None,
        AuditEvent::RecordingStopped {
            recording_id: id(41),
            reason: RecordingStopReason::CallEnded,
        },
    )
    .channel_id = Some(id(DAILY));
    log.entry(
        2 * hour + 50,
        Some("Bruno"),
        AuditEvent::CallMicrophoneBlocked {
            account_id: person("Marta"),
        },
    )
    .channel_id = Some(id(DAILY));
    log.entry(
        3 * hour,
        Some("Carla"),
        AuditEvent::RecordingStarted {
            recording_id: id(41),
        },
    )
    .channel_id = Some(id(DAILY));
    log.entry(
        5 * hour,
        Some("Luca"),
        AuditEvent::ChannelCreated {
            name: "deploys".to_owned(),
            kind: ChannelKind::Text,
        },
    )
    .channel_id = Some(id(DEPLOYS));
    log.entry(
        6 * hour,
        Some("Luca"),
        AuditEvent::CategoryCreated {
            category_id: id(PROJECTS),
            name: "Projects".to_owned(),
        },
    );
    log.entry(
        day,
        Some("Ana"),
        AuditEvent::WorkspaceRoleAssigned {
            role_id: id(ENGINEERS),
            account_id: person("Diego"),
        },
    );
    log.entry(
        day + hour,
        Some("Luca"),
        AuditEvent::WorkspaceMemberAdded {
            account_id: person("Marta"),
        },
    );
    log.entry(
        2 * day,
        Some("Luca"),
        AuditEvent::ArchivePeriodChanged { days: Some(90) },
    );
    log.entry(
        2 * day + 3 * hour,
        Some("Ana"),
        AuditEvent::ExportRequested {
            export_id: id(EXPORT + 3),
        },
    )
    .channel_id = Some(id(GENERAL));
    // The state holds no channel with this identifier: the channel was deleted.
    log.entry(
        3 * day,
        Some("Luca"),
        AuditEvent::ChannelDeleted {
            name: "old-links".to_owned(),
        },
    )
    .channel_id = Some(id(39));
    log.entry(
        3 * day + 2 * hour,
        Some("Ana"),
        AuditEvent::PermanentMessageDeleted {
            message_id: id(5321),
        },
    )
    .channel_id = Some(id(DEPLOYS));
    log.entry(
        4 * day,
        Some("Bruno"),
        AuditEvent::GlossaryReplaced { terms: 42 },
    );
    log.entries
}

/// A request that the server refused with this code, with the sentence that the page
/// states for it.
pub(crate) fn refused(what: AskKind, code: ErrorCode) -> Option<Ask> {
    let error = ApiError::Rejected(Rejection {
        code,
        status: code.http_status(),
        retry_after: None,
        message: String::new(),
        left: None,
    });
    Some(Ask {
        what,
        state: AskState::Refused(text::admin::refusal(what, &error)),
    })
}

/// The times of the pages are shown in UTC on every machine.
fn utc(shell: &mut Shell, cx: &mut gpui::Context<Shell>) {
    shell.state().clone().update(cx, |state, cx| {
        state.set_zone(Zone::Fixed(UtcOffset::UTC), cx)
    });
}

/// Opens the administration of "Acme Studio" at a page.
pub(crate) fn open_page(
    shell: &mut Shell,
    page: AdminPage,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    utc(shell, cx);
    let open = AdminOpen::Server {
        server_id: id(ACME),
        page,
    };
    shell.open_admin(open, window, cx);
}

/// Opens the settings of a channel of "Studio" at a page.
fn open_channel(
    shell: &mut Shell,
    channel: u32,
    page: ChannelPage,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    utc(shell, cx);
    let open = AdminOpen::Channel {
        server_id: id(ACME),
        workspace_id: id(STUDIO),
        channel_id: id(channel),
        page,
    };
    shell.open_admin(open, window, cx);
}

/// Shows this state of a request and this result of a request. A call that opens a
/// form or a question puts both away, so this call comes after it.
pub(crate) fn tell(
    shell: &mut Shell,
    ask: Option<Ask>,
    made: Option<Made>,
    cx: &mut gpui::Context<Shell>,
) {
    shell
        .state()
        .clone()
        .update(cx, |state, cx| state.show_admin_fixture(ask, made, cx));
}

/// Shows this audit log and these files of exports. The opening of the administration
/// puts the audit log away, so this call comes after it.
fn show_records(
    shell: &mut Shell,
    audit: AuditLog,
    files: ExportFiles,
    cx: &mut gpui::Context<Shell>,
) {
    shell
        .state()
        .clone()
        .update(cx, |state, cx| state.show_records_fixture(audit, files, cx));
}

fn draw_workspaces(studio: &mut Studio) -> Result<(), String> {
    // The owner is no member of "Client Beta" and still sees it.
    studio.draw(
        Screen::new(
            "admin-workspaces-owner",
            servers(|model| {
                owner(model);
                let beta: WorkspaceId = id(CLIENT_BETA);
                model.account.workspace_ids.retain(|held| *held != beta);
            }),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Workspaces, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-workspaces-member", servers(|_| {})),
        |shell, window, cx| open_page(shell, AdminPage::Workspaces, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-workspace-create", servers(owner)),
        |shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
            shell.set_admin_mode(Some(Mode::CreateWorkspace), "", window, cx);
            shell.type_admin_name("Client Gamma", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-workspace-rename", servers(owner)),
        |shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
            let mode = Mode::RenameWorkspace(id(CLIENT_ALPHA));
            shell.set_admin_mode(Some(mode), "Client Alpha", window, cx);
            shell.type_admin_name("Alpha Partners", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-workspace-delete", servers(owner)),
        |shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
            let mode = Mode::DeleteWorkspace(id(CLIENT_BETA));
            shell.set_admin_mode(Some(mode), "", window, cx);
        },
    )?;
    // An administrator who is not the owner deletes a workspace with permanent messages.
    studio.draw(
        Screen::new("admin-workspace-delete-refused", servers(administrator)),
        |shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
            let mode = Mode::DeleteWorkspace(id(CLIENT_ALPHA));
            shell.set_admin_mode(Some(mode), "", window, cx);
            let what = AskKind::DeleteWorkspace(id(CLIENT_ALPHA));
            tell(shell, refused(what, ErrorCode::OwnerOnly), None, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-workspace-code", servers(|_| {})),
        |shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
            shell.set_admin_mode(Some(Mode::EnterCode), "", window, cx);
            shell.type_admin_value("ABCD-EFGH-JKMN-PQRS", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-workspace-entered", servers(|_| {})),
        |shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
            let entered = Made::Entered(vec![id(CLIENT_ALPHA), id(CLIENT_BETA)]);
            tell(shell, None, Some(entered), cx);
        },
    )
}

/// "Acme Studio" with the categories of the screens, for an administrator.
fn with_structure() -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        structure(model);
    })
}

fn draw_channels(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new("admin-channels", with_structure()),
        |shell, window, cx| open_page(shell, AdminPage::Channels, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-channels-150", with_structure()).scale(150),
        |shell, window, cx| open_page(shell, AdminPage::Channels, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-channel-create", with_structure()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Channels, window, cx);
            shell.set_admin_mode(Some(Mode::CreateChannel(None)), "", window, cx);
            shell.type_admin_name("release-notes", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-channel-move", with_structure()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Channels, window, cx);
            let mode = Mode::MoveChannel(id(RANDOM));
            shell.set_admin_mode(Some(mode), "", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-category-rename", with_structure()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Channels, window, cx);
            let mode = Mode::RenameCategory(id(PROJECTS));
            shell.set_admin_mode(Some(mode), "Projects", window, cx);
            shell.type_admin_name("Client projects", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-category-delete", with_structure()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Channels, window, cx);
            let mode = Mode::DeleteCategory(id(ROOMS));
            shell.set_admin_mode(Some(mode), "", window, cx);
        },
    )?;
    // The window checks that a name is not empty. The server refuses one of more than 64
    // characters.
    studio.draw(
        Screen::new("admin-channel-refused", with_structure()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Channels, window, cx);
            shell.set_admin_mode(Some(Mode::CreateChannel(None)), "", window, cx);
            shell.type_admin_name(
                "release-notes-for-every-client-of-the-studio-in-the-first-half-of-2027",
                window,
                cx,
            );
            let what = AskKind::CreateChannel;
            tell(shell, refused(what, ErrorCode::InvalidRequest), None, cx);
        },
    )
}

/// "Acme Studio" for the settings of a channel, for an administrator: the categories of
/// the screens, the archived channels, and these overrides of the channel.
fn with_channel(channel: u32, overrides: Collection<Vec<ChannelOverride>>) -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        structure(model);
        read_archived(model, archived());
        set_overrides(model, channel, overrides);
    })
}

fn draw_channel_settings(studio: &mut Studio) -> Result<(), String> {
    let read = |overrides: Vec<ChannelOverride>| Collection::Loaded(overrides);

    // The overview.
    studio.draw(
        Screen::new(
            "admin-channel-overview-text",
            with_channel(GENERAL, read(Vec::new())),
        ),
        |shell, window, cx| open_channel(shell, GENERAL, ChannelPage::Overview, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-overview-voice",
            with_channel(HR, read(Vec::new())),
        ),
        |shell, window, cx| open_channel(shell, HR, ChannelPage::Overview, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-overview-private",
            with_channel(ALPHA, read(private_overrides())),
        ),
        |shell, window, cx| open_channel(shell, ALPHA, ChannelPage::Overview, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-overview-archived",
            with_channel(ROADMAP, read(Vec::new())),
        ),
        |shell, window, cx| open_channel(shell, ROADMAP, ChannelPage::Overview, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-rename",
            with_channel(GENERAL, read(Vec::new())),
        ),
        |shell, window, cx| {
            open_channel(shell, GENERAL, ChannelPage::Overview, window, cx);
            shell.set_admin_mode(Some(Mode::RenameChannel), "general", window, cx);
            shell.type_admin_name("announcements", window, cx);
        },
    )?;

    // The permissions. The page shows the "everyone" role first, with its permissions.
    studio.draw(
        Screen::new(
            "admin-channel-permissions",
            with_channel(GENERAL, read(vec![everyone_override(), leads_override()])),
        ),
        |shell, window, cx| open_channel(shell, GENERAL, ChannelPage::Permissions, window, cx),
    )?;
    // A member who holds "manage channel" and not the permissions of the override: the
    // grant rule refuses each choice that gives a permission.
    studio.draw(
        Screen::new(
            "admin-channel-permissions-ceiling",
            servers(|model| {
                lead(model, &[WorkspacePermission::ManageChannel]);
                structure(model);
                set_overrides(
                    model,
                    GENERAL,
                    Collection::Loaded(vec![everyone_override()]),
                );
            }),
        ),
        |shell, window, cx| open_channel(shell, GENERAL, ChannelPage::Permissions, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-permissions-loading",
            with_channel(GENERAL, Collection::Loading),
        ),
        |shell, window, cx| open_channel(shell, GENERAL, ChannelPage::Permissions, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-permissions-failed",
            with_channel(GENERAL, Collection::Failed(LoadFailure::NoAnswer)),
        ),
        |shell, window, cx| open_channel(shell, GENERAL, ChannelPage::Permissions, window, cx),
    )?;

    // Archive, export and deletion.
    studio.draw(
        Screen::new(
            "admin-channel-archive",
            with_channel(RANDOM, read(Vec::new())),
        ),
        |shell, window, cx| open_channel(shell, RANDOM, ChannelPage::Archive, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-archive-question",
            with_channel(RANDOM, read(Vec::new())),
        ),
        |shell, window, cx| {
            open_channel(shell, RANDOM, ChannelPage::Archive, window, cx);
            let mode = Mode::ArchiveChannel(id(RANDOM));
            shell.set_admin_mode(Some(mode), "", window, cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-archived",
            with_channel(ROADMAP, read(Vec::new())),
        ),
        |shell, window, cx| open_channel(shell, ROADMAP, ChannelPage::Archive, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-channel-export-asked",
            with_channel(RANDOM, read(Vec::new())),
        ),
        |shell, window, cx| {
            open_channel(shell, RANDOM, ChannelPage::Archive, window, cx);
            tell(shell, None, Some(Made::Export(id(EXPORT + 1))), cx);
        },
    )?;
    // Only the owner is offered the deletion of a channel with permanent messages.
    studio.draw(
        Screen::new(
            "admin-channel-delete-question",
            servers(|model| {
                owner(model);
                structure(model);
            }),
        ),
        |shell, window, cx| {
            open_channel(shell, DEPLOYS, ChannelPage::Archive, window, cx);
            let mode = Mode::DeleteChannel(id(DEPLOYS));
            shell.set_admin_mode(Some(mode), "", window, cx);
        },
    )?;
    // A member without "manage channel" has the page of the archive only.
    studio.draw(
        Screen::new("admin-channel-settings-member", servers(structure)),
        |shell, window, cx| open_channel(shell, DEPLOYS, ChannelPage::Archive, window, cx),
    )
}

fn draw_archive(studio: &mut Studio) -> Result<(), String> {
    let for_administrator = || {
        servers(|model| {
            administrator(model);
            read_archived(model, archived());
            read_retention(model);
        })
    };
    studio.draw(
        Screen::new(
            "admin-archive-member",
            servers(|model| read_archived(model, archived())),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Archive, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-archive-empty",
            servers(|model| read_archived(model, Vec::new())),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Archive, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-archive-administrator", for_administrator()),
        |shell, window, cx| open_page(shell, AdminPage::Archive, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-archive-days-form", for_administrator()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Archive, window, cx);
            let mode = Mode::Days(AskKind::ArchivePeriod);
            shell.set_admin_mode(Some(mode), "", window, cx);
            shell.type_admin_value("120", window, cx);
        },
    )?;
    // No collection of the page is read yet.
    studio.draw(
        Screen::new("admin-archive-loading", servers(administrator)),
        |shell, window, cx| open_page(shell, AdminPage::Archive, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-archive-failed",
            servers(|model| {
                administrator(model);
                let admin = collections(model);
                admin.archived = vec![(id(STUDIO), Collection::Failed(LoadFailure::NoAnswer))];
                admin.settings = vec![(id(STUDIO), Collection::Failed(LoadFailure::Unavailable))];
                admin.direct_retention = Collection::Failed(LoadFailure::Unavailable);
            }),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Archive, window, cx),
    )?;

    // The archived channel in the main area, for a member and for a person who may
    // restore it. The channel is in the archived list only.
    let in_main_area = |manager: bool| {
        servers(move |model| {
            if manager {
                lead(model, &[WorkspacePermission::ManageChannel]);
            }
            read_archived(model, archived());
            model.timelines = vec![Arc::new(roadmap_messages())];
        })
    };
    for (name, manager) in [
        ("admin-archived-channel", false),
        ("admin-archived-channel-manager", true),
    ] {
        studio.draw(Screen::new(name, in_main_area(manager)), |shell, _, cx| {
            prepare(shell, none(), cx);
            shell.state().clone().update(cx, |state, cx| {
                state.select_channel(id(STUDIO), id(ROADMAP), cx);
            });
        })?;
    }
    Ok(())
}

/// "Acme Studio" for an administrator, with the server roles and this state of the
/// accounts. Ana owns the server.
fn with_accounts(accounts: Collection<Vec<AccountSummary>>) -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        let admin = collections(model);
        admin.accounts = accounts;
        admin.server_roles = Collection::Loaded(server_roles());
    })
}

fn with_invites(invites: Collection<Vec<Invite>>) -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        collections(model).invites = invites;
    })
}

fn with_requests(requests: Collection<Vec<JoinRequest>>) -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        collections(model).join_requests = requests;
    })
}

fn draw_people(studio: &mut Studio) -> Result<(), String> {
    let everyone = || with_accounts(Collection::Loaded(accounts("Ana")));

    // The accounts of the server.
    studio.draw(
        Screen::new("admin-people", everyone()),
        |shell, window, cx| open_page(shell, AdminPage::People, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-people-ban-question", everyone()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            let open = PeopleOpen::BanAccount(person("Carla"));
            shell.set_people_open(Some(open), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-people-remove-question", everyone()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            let open = PeopleOpen::RemoveAccount(person("Bruno"));
            shell.set_people_open(Some(open), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-people-banned-filter", everyone()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            shell.show_banned_only(true, cx);
        },
    )?;
    // The question takes a reason, which the audit log keeps.
    studio.draw(
        Screen::new("admin-people-ban-reason", everyone()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            let open = PeopleOpen::BanAccount(person("Carla"));
            shell.set_people_open(Some(open), window, cx);
            shell.type_reason(
                "Sent the same advertisement to every channel after two warnings.",
                window,
                cx,
            );
        },
    )?;
    // The question takes a message to the person beside the reason (ADR 0764).
    studio.draw(
        Screen::new("admin-people-ban-message", everyone()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            let open = PeopleOpen::BanAccount(person("Carla"));
            shell.set_people_open(Some(open), window, cx);
            shell.type_reason(
                "Sent the same advertisement to every channel after two warnings.",
                window,
                cx,
            );
            shell.type_moderation_message(
                "You can ask the owner for an unban by mail.",
                window,
                cx,
            );
        },
    )?;
    studio.draw(
        Screen::new("admin-people-reason-too-long", everyone()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            let open = PeopleOpen::RemoveAccount(person("Bruno"));
            shell.set_people_open(Some(open), window, cx);
            shell.type_reason(&"The same sentence again. ".repeat(41), window, cx);
        },
    )?;
    // Another person banned the account while the question was open.
    studio.draw(
        Screen::new("admin-people-refused", everyone()),
        |shell, window, cx| {
            open_page(shell, AdminPage::People, window, cx);
            let open = PeopleOpen::BanAccount(person("Marta"));
            shell.set_people_open(Some(open), window, cx);
            let what = AskKind::BanAccount(person("Marta"));
            tell(shell, refused(what, ErrorCode::Conflict), None, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-people-loading", with_accounts(Collection::Loading)),
        |shell, window, cx| open_page(shell, AdminPage::People, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-people-failed",
            with_accounts(Collection::Failed(LoadFailure::NoAnswer)),
        ),
        |shell, window, cx| open_page(shell, AdminPage::People, window, cx),
    )?;

    // The members of "Studio": for a member, and for an administrator with the accounts
    // that are no members of it.
    studio.draw(
        Screen::new("admin-members", servers(|_| {})),
        |shell, window, cx| open_page(shell, AdminPage::Members, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-members-add",
            servers(|model| {
                administrator(model);
                collections(model).accounts = Collection::Loaded(accounts("Ana"));
                let workspace = Arc::make_mut(&mut model.workspaces[0]);
                if let Loadable::Loaded(members) = &mut workspace.members {
                    members.truncate(3);
                }
            }),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Members, window, cx),
    )?;
    // An administrator below the owner removes a member from the workspace: the owner,
    // the own account and an administrator at the same position state why not.
    let removing = || {
        servers(|model| {
            administrator(model);
            let mut listed = accounts("Ana");
            for account in &mut listed {
                if account.name == "Bruno" {
                    account.server_role_ids = vec![id(ADMINISTRATORS)];
                }
            }
            collections(model).accounts = Collection::Loaded(listed);
            collections(model).server_roles = Collection::Loaded(server_roles());
        })
    };
    studio.draw(
        Screen::new("admin-members-remove", removing()),
        |shell, window, cx| open_page(shell, AdminPage::Members, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-members-remove-question", removing()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Members, window, cx);
            let open = PeopleOpen::RemoveMember(person("Carla"));
            shell.set_people_open(Some(open), window, cx);
            shell.type_reason("The project of this workspace ended.", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-members-remove-refused", removing()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Members, window, cx);
            let what = AskKind::RemoveMember(person("Carla"));
            tell(shell, refused(what, ErrorCode::Forbidden), None, cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "admin-members-loading",
            servers(|model| {
                Arc::make_mut(&mut model.workspaces[0]).members = Loadable::Loading;
            }),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Members, window, cx),
    )?;

    // The invites.
    let listed = || with_invites(Collection::Loaded(invites()));
    studio.draw(
        Screen::new("admin-invites", listed()),
        |shell, window, cx| open_page(shell, AdminPage::Invites, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-invite-form", listed()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Invites, window, cx);
            shell.set_people_open(Some(PeopleOpen::NewInvite), window, cx);
            shell.type_invite_uses("12", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-invite-code", listed()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Invites, window, cx);
            let made = Made::Invite {
                invite_id: id(INVITE + 1),
                code: InviteCode::new("ABCD-EFGH-JKMN-PQRS"),
            };
            tell(shell, None, Some(made), cx);
        },
    )?;
    for (name, state) in [
        ("admin-invites-empty", Collection::Loaded(Vec::new())),
        ("admin-invites-loading", Collection::Loading),
        (
            "admin-invites-failed",
            Collection::Failed(LoadFailure::NoAnswer),
        ),
    ] {
        studio.draw(
            Screen::new(name, with_invites(state)),
            |shell, window, cx| open_page(shell, AdminPage::Invites, window, cx),
        )?;
    }

    // The join requests. An approval grants the workspaces that are chosen above the
    // list.
    let waiting = || with_requests(Collection::Loaded(join_requests()));
    studio.draw(
        Screen::new("admin-requests", waiting()),
        |shell, window, cx| open_page(shell, AdminPage::JoinRequests, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-requests-approve", waiting()),
        |shell, window, cx| {
            open_page(shell, AdminPage::JoinRequests, window, cx);
            shell.choose_approval_workspaces(vec![id(STUDIO), id(CLIENT_ALPHA)], cx);
        },
    )?;
    for (name, state) in [
        ("admin-requests-empty", Collection::Loaded(Vec::new())),
        (
            "admin-requests-failed",
            Collection::Failed(LoadFailure::NoAnswer),
        ),
    ] {
        studio.draw(
            Screen::new(name, with_requests(state)),
            |shell, window, cx| open_page(shell, AdminPage::JoinRequests, window, cx),
        )?;
    }
    Ok(())
}

fn with_tokens(tokens: Collection<Vec<AccessTokenInfo>>) -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        collections(model).access_tokens = tokens;
    })
}

fn with_exports(exports: Collection<Vec<Export>>) -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        collections(model).exports = exports;
    })
}

fn draw_records(studio: &mut Studio) -> Result<(), String> {
    // The access tokens.
    let tokens = || with_tokens(Collection::Loaded(access_tokens()));
    studio.draw(
        Screen::new("admin-tokens", tokens()),
        |shell, window, cx| open_page(shell, AdminPage::Tokens, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-token-form", tokens()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Tokens, window, cx);
            shell.set_token_form(true, window, cx);
            shell.type_token_name("setup tool", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-token-secret", tokens()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Tokens, window, cx);
            let made = Made::Token {
                access_token_id: id(TOKEN + 1),
                token: AccessToken::new("jat_example-token-that-is-shown-once-0000"),
            };
            tell(shell, None, Some(made), cx);
        },
    )?;
    // The server holds more tokens than the client reads.
    studio.draw(
        Screen::new(
            "admin-tokens-more",
            servers(|model| {
                administrator(model);
                let admin = collections(model);
                admin.access_tokens = Collection::Loaded(access_tokens());
                admin.access_tokens_more = true;
            }),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Tokens, window, cx),
    )?;
    for (name, state) in [
        ("admin-tokens-empty", Collection::Loaded(Vec::new())),
        ("admin-tokens-loading", Collection::Loading),
        (
            "admin-tokens-failed",
            Collection::Failed(LoadFailure::NoAnswer),
        ),
    ] {
        studio.draw(
            Screen::new(name, with_tokens(state)),
            |shell, window, cx| open_page(shell, AdminPage::Tokens, window, cx),
        )?;
    }

    // The exports of the person.
    let listed = || with_exports(Collection::Loaded(exports()));
    studio.draw(
        Screen::new("admin-exports", listed()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Exports, window, cx);
            show_records(shell, AuditLog::default(), export_files(), cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-export-remove-question", listed()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Exports, window, cx);
            show_records(shell, AuditLog::default(), export_files(), cx);
            shell.set_export_removing(Some(id(EXPORT + 3)), cx);
        },
    )?;
    // The server ended the answer with the file of the ready export several times.
    studio.draw(
        Screen::new("admin-export-save-failed", listed()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Exports, window, cx);
            let failed = ExportSave::Failed(text::admin::records::SAVE_ENDED.to_owned());
            let files = ExportFiles::of([((id(ACME), id(EXPORT + 3)), failed)]);
            show_records(shell, AuditLog::default(), files, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-exports-refused", listed()),
        |shell, window, cx| {
            open_page(shell, AdminPage::Exports, window, cx);
            show_records(shell, AuditLog::default(), export_files(), cx);
            let what = AskKind::RequestExport;
            tell(shell, refused(what, ErrorCode::LimitReached), None, cx);
        },
    )?;
    for (name, state) in [
        ("admin-exports-empty", Collection::Loaded(Vec::new())),
        ("admin-exports-loading", Collection::Loading),
        (
            "admin-exports-failed",
            Collection::Failed(LoadFailure::NoAnswer),
        ),
    ] {
        studio.draw(
            Screen::new(name, with_exports(state)),
            |shell, window, cx| open_page(shell, AdminPage::Exports, window, cx),
        )?;
    }
    Ok(())
}

/// "Acme Studio" for the audit log, for an administrator, with what the entries name:
/// the categories, the archived channels and the server roles.
fn with_names() -> Vec<ServerSnapshot> {
    servers(|model| {
        administrator(model);
        structure(model);
        read_archived(model, archived());
        collections(model).server_roles = Collection::Loaded(server_roles());
    })
}

/// Shows the audit log of a scope of "Acme Studio" in the open administration.
fn show_log(
    shell: &mut Shell,
    scope: AuditScope,
    change: impl FnOnce(AuditLog) -> AuditLog,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    open_page(shell, AdminPage::AuditLog, window, cx);
    let (entries, older) = match scope {
        // The server holds entries before these: the page offers to read them.
        AuditScope::Server => (server_log(), true),
        AuditScope::Workspace(_) | AuditScope::DeletedWorkspace(_) => (workspace_log(), false),
    };
    let log = change(AuditLog::loaded(id(ACME), scope, entries, older));
    show_records(shell, log, ExportFiles::default(), cx);
}

/// Shows the audit log of the server without an entry, as a read left it.
fn show_empty_log(
    shell: &mut Shell,
    change: impl FnOnce(AuditLog) -> AuditLog,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    open_page(shell, AdminPage::AuditLog, window, cx);
    let log = change(AuditLog::loaded(
        id(ACME),
        AuditScope::Server,
        Vec::new(),
        false,
    ));
    show_records(shell, log, ExportFiles::default(), cx);
}

fn draw_audit(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new("admin-audit-server", with_names()),
        |shell, window, cx| show_log(shell, AuditScope::Server, |log| log, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-audit-server-150", with_names()).scale(150),
        |shell, window, cx| show_log(shell, AuditScope::Server, |log| log, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-audit-workspace", with_names()),
        |shell, window, cx| {
            let scope = AuditScope::Workspace(id(STUDIO));
            show_log(shell, scope, |log| log, window, cx);
        },
    )?;
    // The server applied the filters: the log holds the entries that pass them.
    fn banned(log: AuditLog) -> AuditLog {
        let filter = AuditFilter {
            action: Some("account_banned"),
            about: Some(AuditTarget::Account(person("Elisa"))),
            period: AuditPeriod::Month,
            ..AuditFilter::default()
        };
        log.with_filter(filter, |entry| {
            matches!(entry.event, AuditEvent::AccountBanned { .. })
        })
        .with_people(vec![person("Luca"), person("Ana"), person("Elisa")])
    }
    studio.draw(
        Screen::new("admin-audit-filtered", with_names()),
        |shell, window, cx| show_log(shell, AuditScope::Server, banned, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-audit-filter-actions", with_names()),
        |shell, window, cx| {
            show_log(shell, AuditScope::Server, banned, window, cx);
            shell.show_audit_menu(Some(AuditMenu::Action(AuditGroup::Members)), cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-audit-filter-people", with_names()),
        |shell, window, cx| {
            show_log(shell, AuditScope::Server, banned, window, cx);
            shell.show_audit_menu(Some(AuditMenu::About(TargetKind::People)), cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-audit-filter-workspaces", with_names()),
        |shell, window, cx| {
            show_log(shell, AuditScope::Server, |log| log, window, cx);
            shell.show_audit_menu(Some(AuditMenu::About(TargetKind::Workspaces)), cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-audit-filter-dates", with_names()),
        |shell, window, cx| {
            show_log(shell, AuditScope::Server, |log| log, window, cx);
            shell.show_audit_menu(Some(AuditMenu::Dates), cx);
            shell.type_audit_dates("2026-10-01", "2026-10-12", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-audit-filter-none-pass", with_names()),
        |shell, window, cx| {
            let none = |log: AuditLog| {
                let filter = AuditFilter {
                    action: Some("server_logo_changed"),
                    ..AuditFilter::default()
                };
                log.with_filter(filter, |_| false)
            };
            show_log(shell, AuditScope::Server, none, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-audit-loading", with_names()),
        |shell, window, cx| show_empty_log(shell, AuditLog::with_reading, window, cx),
    )?;
    studio.draw(
        Screen::new("admin-audit-failed", with_names()),
        |shell, window, cx| {
            let error = ApiError::Unreachable(TransportError::new(
                TransportErrorKind::Timeout,
                "no answer in the screen tool",
            ));
            let sentence = text::admin::records::audit_failure(&error);
            show_empty_log(shell, |log| log.with_failure(sentence), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-audit-empty", with_names()),
        |shell, window, cx| show_empty_log(shell, |log| log, window, cx),
    )
}

fn draw_notifications(studio: &mut Studio) -> Result<(), String> {
    let with_level = |level: Option<NotificationLevel>| {
        servers(move |model| {
            administrator(model);
            Arc::make_mut(&mut model.workspaces[0]).notification_default = level;
        })
    };
    studio.draw(
        Screen::new("admin-notifications-not-set", with_level(None)),
        |shell, window, cx| open_page(shell, AdminPage::Notifications, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-notifications-all",
            with_level(Some(NotificationLevel::All)),
        ),
        |shell, window, cx| open_page(shell, AdminPage::Notifications, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-notifications-nothing-150",
            with_level(Some(NotificationLevel::Nothing)),
        )
        .scale(150),
        |shell, window, cx| open_page(shell, AdminPage::Notifications, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-notifications-sending",
            with_level(Some(NotificationLevel::Mentions)),
        ),
        |shell, window, cx| {
            open_page(shell, AdminPage::Notifications, window, cx);
            let ask = Ask {
                what: AskKind::NotificationDefault(id(STUDIO)),
                state: AskState::Sending,
            };
            tell(shell, Some(ask), None, cx);
        },
    )?;
    studio.draw(
        Screen::new("admin-notifications-refused", with_level(None)),
        |shell, window, cx| {
            open_page(shell, AdminPage::Notifications, window, cx);
            let what = AskKind::NotificationDefault(id(STUDIO));
            tell(shell, refused(what, ErrorCode::Forbidden), None, cx);
        },
    )
}

/// The glossary page (ADR 0616): for a member, for a person who manages it, with the form
/// of a changed term, the question before a removal, and a list that the client holds in
/// part.
fn draw_glossary(studio: &mut Studio) -> Result<(), String> {
    let glossary = |manages: bool, cut: bool| {
        servers(move |model| {
            if manages {
                lead(model, &[WorkspacePermission::ManageGlossary]);
            }
            let terms = [
                "Marta Lindqvist",
                "mixdown",
                "staging",
                "Quorvex",
                "retention spec",
            ];
            collections(model).glossaries = vec![(
                id(STUDIO),
                Collection::Loaded(GlossaryList {
                    terms: terms.iter().map(|term| (*term).to_owned()).collect(),
                    cut,
                }),
            )];
        })
    };
    let page = |shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
        open_page(shell, AdminPage::Glossary, window, cx);
    };
    studio.draw(
        Screen::new("admin-glossary-member", glossary(false, false)),
        page,
    )?;
    studio.draw(
        Screen::new("admin-glossary-manager", glossary(true, false)),
        page,
    )?;
    studio.draw(
        Screen::new("admin-glossary-cut", glossary(true, true)),
        page,
    )?;
    studio.draw(
        Screen::new("admin-glossary-change-form", glossary(true, false)),
        |shell, window, cx| {
            open_page(shell, AdminPage::Glossary, window, cx);
            shell.open_term_mode(Mode::ChangeTerm(1), "mixdown", window, cx);
        },
    )?;
    studio.draw(Screen::new("admin-glossary-loading", servers(|_| {})), page)?;
    studio.draw(
        Screen::new("admin-glossary-refused", glossary(true, false)),
        |shell, window, cx| {
            open_page(shell, AdminPage::Glossary, window, cx);
            tell(
                shell,
                refused(AskKind::MoveTerm(2), ErrorCode::Conflict),
                None,
                cx,
            );
        },
    )
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    draw_glossary(studio)?;
    draw_notifications(studio)?;
    draw_workspaces(studio)?;
    draw_channels(studio)?;
    draw_channel_settings(studio)?;
    draw_archive(studio)?;
    draw_people(studio)?;
    draw_records(studio)?;
    draw_audit(studio)
}
