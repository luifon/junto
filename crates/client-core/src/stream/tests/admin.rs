//! The collections of the administration in the stream client (ADR 0760), against routes
//! that the test plays: the read that a reader starts, the events of the time of a read,
//! a failed read, a reload, a change of the session, and the archived channel of a reader
//! of messages.

use junto_protocol::WorkspacePermission;
use junto_protocol::access_token::{AccessTokenInfo, AccessTokenPage, TokenAction};
use junto_protocol::account::{AccountStatus, AccountSummary};
use junto_protocol::discovery::Retention;
use junto_protocol::export::Export;
use junto_protocol::ids::{AccessTokenId, ChannelId, WorkspaceId};
use junto_protocol::invite::{Invite, JoinRequest};
use junto_protocol::roles::ServerRole;
use junto_protocol::structure::{ArchivePeriod, ArchivedChannel, Channel, ChannelOverride};

use super::messages::positions;
use super::*;
use crate::api::ApiError;
use crate::model::message_tests::message;
use crate::model::{
    AdminPart, Collection, CollectionState, GlossaryList, LoadFailure, MOST_GLOSSARY_TERMS,
    MOST_TOKENS, PlaceId, WorkspaceSettings,
};

/// The size of the page of access tokens that the client asks for.
const PAGE: usize = 200;
const SETTINGS: [&str; 3] = ["/archive-period", "/file-retention", "/recording-retention"];
const DIRECT: &str = "/direct-message-retention";

/// The collections of the administration as the played server holds them.
#[derive(Default)]
struct AdminRoutes {
    accounts: Vec<AccountSummary>,
    invites: Vec<Invite>,
    join_requests: Vec<JoinRequest>,
    exports: Vec<Export>,
    server_roles: Vec<ServerRole>,
    overrides: Vec<ChannelOverride>,
    archived: Vec<ArchivedChannel>,
    tokens: Vec<AccessTokenInfo>,
    /// Every page of tokens after the first holds no token and names a further page.
    tokens_stall: bool,
    /// The `before` and the `limit` of the pages of tokens that the client asked for.
    token_queries: Vec<(Option<AccessTokenId>, usize)>,
    direct_retention: Option<Retention>,
    settings: Option<WorkspaceSettings>,
    /// The workspace permissions that the route of one channel states for the account.
    permissions: Vec<junto_protocol::WorkspacePermission>,
    /// The glossary that the route of every workspace answers.
    glossary: Option<junto_protocol::transcript::Glossary>,
    /// The next requests whose path ends with the text get no answer.
    lost: Vec<&'static str>,
}

fn json<T: serde::Serialize>(body: &T) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse {
        status: 200,
        retry_after: None,
        body: serde_json::to_vec(body).unwrap().into(),
        receipt: crate::api::Receipt::NotStated,
    })
}

impl AdminRoutes {
    fn token_page(&mut self, request: &HttpRequest) -> AccessTokenPage {
        let value = |name: &str| {
            request
                .url
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        let before: Option<AccessTokenId> = value("before").map(|id| id.parse().unwrap());
        let limit: usize = value("limit").map_or(50, |limit| limit.parse().unwrap());
        self.token_queries.push((before, limit));
        if self.tokens_stall && before.is_some() {
            return AccessTokenPage {
                access_tokens: Vec::new(),
                next_before: before,
            };
        }
        let mut older: Vec<AccessTokenInfo> = self
            .tokens
            .iter()
            .filter(|held| before.is_none_or(|before| held.id < before))
            .cloned()
            .collect();
        older.sort_by_key(|held| std::cmp::Reverse(held.id));
        let more = older.len() > limit;
        older.truncate(limit);
        AccessTokenPage {
            next_before: older.last().filter(|_| more).map(|held| held.id),
            access_tokens: older,
        }
    }

    /// The answer to a request of a route of the administration. `None` for another route.
    fn answer(&mut self, request: &HttpRequest) -> Option<Result<HttpResponse, TransportError>> {
        let path = request.url.path();
        let ends = |end: &str| path.ends_with(end);
        let answer = if ends("/accounts") {
            json(&self.accounts)
        } else if ends("/invites") {
            json(&self.invites)
        } else if ends("/join-requests") {
            json(&self.join_requests)
        } else if ends("/exports") {
            json(&self.exports)
        } else if ends("/server-roles") {
            json(&self.server_roles)
        } else if ends("/overrides") {
            json(&self.overrides)
        } else if ends("/permissions") {
            // `/api/v1/channels/{channel_id}/permissions`: the answer names its channel.
            let channel_id = path
                .rsplit('/')
                .nth(1)
                .and_then(|id| id.parse().ok())
                .unwrap();
            json(&junto_protocol::structure::ChannelPermissions {
                channel_id,
                permissions: self.permissions.clone(),
            })
        } else if ends("/archived-channels") {
            json(&self.archived)
        } else if ends("/access-tokens") {
            json(&self.token_page(request))
        } else if ends(DIRECT) {
            json(self.direct_retention.as_ref().unwrap())
        } else if ends("/archive-period") {
            json(&self.settings.as_ref().unwrap().archive_period)
        } else if ends("/file-retention") {
            json(&self.settings.as_ref().unwrap().file_retention)
        } else if ends("/recording-retention") {
            json(&self.settings.as_ref().unwrap().recording_retention)
        } else if ends("/glossary") {
            json(self.glossary.as_ref().unwrap())
        } else {
            return None;
        };
        if let Some(position) = self.lost.iter().position(|end| path.ends_with(*end)) {
            self.lost.remove(position);
            return Some(Err(TransportError::new(
                TransportErrorKind::Interrupted,
                "the answer was lost",
            )));
        }
        Some(answer)
    }
}

/// The transport of the harness with the routes of the administration. The routes of the
/// harness answer no path of the administration, and `rig_with` takes no transport, so
/// this file builds its own rig. A request of the administration is counted, held and
/// refused by the same `Routes` as every other request.
struct AdminHttp {
    http: Arc<Http>,
    admin: Arc<Mutex<AdminRoutes>>,
}

impl crate::api::Transport for AdminHttp {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        // The answer is made when the request arrives. A held request gets it after the
        // wait.
        let Some(answer) = self.admin.lock().unwrap().answer(&request) else {
            return crate::api::Transport::send(&*self.http, request);
        };
        let path = request.url.path().to_owned();
        let token = request.bearer.as_ref().unwrap().expose().to_owned();
        let gate = {
            let mut routes = self.http.routes.lock().unwrap();
            routes.seen.push((path.clone(), token));
            routes
                .held
                .as_ref()
                .filter(|(end, _)| path.ends_with(*end))
                .map(|(_, gate)| gate.clone())
        };
        let routes = self.http.routes.clone();
        Box::pin(async move {
            let _active = Active::start(&routes);
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            let mut routes = routes.lock().unwrap();
            let failure = routes
                .failures
                .iter()
                .position(|(end, _, _)| path.ends_with(*end));
            let Some(position) = failure else {
                return answer;
            };
            let (_, status, code) = routes.failures.remove(position);
            let body = format!(r#"{{"error":{{"code":"{code}","message":""}}}}"#);
            Ok(HttpResponse {
                status,
                retry_after: None,
                body: body.into_bytes().into(),
                receipt: crate::api::Receipt::NotStated,
            })
        })
    }
}

/// The rig of the harness with a played server that also answers the routes of the
/// administration.
fn rig_with_admin() -> (Rig, Arc<Mutex<AdminRoutes>>) {
    let scene = scene();
    let http = Arc::new(Http::default());
    http.routes.lock().unwrap().snapshot = Some(scene.snapshot.clone());
    let admin = Arc::new(Mutex::new(AdminRoutes::default()));
    let (peers, accepted) = mpsc::unbounded_channel();
    let wire = Arc::new(Wire {
        peers,
        refusals: Mutex::default(),
        opened_at: Mutex::default(),
    });
    let environment = Arc::new(Environment {
        transport: Arc::new(AdminHttp {
            http: http.clone(),
            admin: admin.clone(),
        }),
        sockets: wire.clone(),
        retry: RetryPolicy::default(),
        policy: StreamPolicy::default(),
    });
    let shared = Shared::new();
    let host = Arc::new(Probe::default());
    let (desired, wanted) = watch::channel(Desired {
        level: ConnectionLevel::Off,
        target: None,
    });
    let (commands, received) = mpsc::unbounded_channel();
    shared.attach(commands);
    tokio::spawn(run(
        shared.clone(),
        host.clone(),
        environment,
        wanted,
        received,
    ));
    let rig = Rig {
        scene,
        live: LiveServer { shared },
        host,
        wire,
        peers: accepted,
        http,
        desired,
        address: ServerAddress::parse("chat.example.test").unwrap(),
        server_id: ServerId::generate(),
    };
    (rig, admin)
}

/// That rig with a served connection.
async fn served_rig() -> (Rig, Peer, Arc<Mutex<AdminRoutes>>) {
    let (mut rig, admin) = rig_with_admin();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    (rig, peer, admin)
}

/// That rig with a served connection, three messages in the text channel and the
/// timeline of the channel open.
async fn reading_rig() -> (Rig, Peer, Arc<Mutex<AdminRoutes>>) {
    let (mut rig, admin) = rig_with_admin();
    let place = PlaceId::Channel(rig.scene.general);
    {
        let mut routes = rig.http.routes.lock().unwrap();
        let other = AccountId::generate();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = (1..=3)
            .map(|position| message(rig.scene.text_place(), position, other, "text"))
            .collect();
    }
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    soon("the first page", rig.live.open_messages(place))
        .await
        .unwrap();
    rig.model_when("the limits", |model| model.attachment_limits.is_some())
        .await;
    (rig, peer, admin)
}

/// Waits until the client has sent `count` requests to the paths that end with a text.
async fn asked(rig: &Rig, end: &str, count: usize) {
    for _ in 0..1000 {
        if rig.http.count(end) >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!(
        "the client sent {} of {count} requests to {end}",
        rig.http.count(end)
    );
}

/// How often the client asked for the paths that end with each text.
fn counts<const N: usize>(rig: &Rig, ends: [&str; N]) -> [usize; N] {
    ends.map(|end| rig.http.count(end))
}

/// Replaces the state by the snapshot of the played server, as after a `resync`.
async fn reload(rig: &Rig, peer: &mut Peer, position: u64, change: impl FnOnce(&mut Snapshot)) {
    rig.set_snapshot(position, change);
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::TooFarBehind,
    })
    .await;
    peer.serve().await;
    rig.when("live", |view| view.status == StreamStatus::Live)
        .await;
}

fn account(name: &str) -> AccountSummary {
    AccountSummary {
        id: AccountId::generate(),
        name: name.to_owned(),
        status: AccountStatus::Active,
        is_owner: false,
        server_role_ids: Vec::new(),
        created_at: time(),
    }
}

fn invite(created_by: AccountId) -> Invite {
    Invite {
        created_by_name: None,
        id: junto_protocol::ids::InviteId::generate(),
        created_by,
        created_at: time(),
        expires_at: time() + time::Duration::days(7),
        max_uses: 5,
        uses: 0,
        workspace_ids: Vec::new(),
        revocation: None,
    }
}

/// Access tokens from the oldest to the newest: the identifier rises with the place.
fn tokens(created_by: AccountId, count: usize) -> Vec<AccessTokenInfo> {
    (1..=count)
        .map(|number| AccessTokenInfo {
            id: AccessTokenId::from_uuid(uuid::Uuid::from_u128(number as u128)),
            name: format!("token {number}"),
            created_by,
            actions: vec![TokenAction::ReadStructure],
            created_at: time(),
            expires_at: time() + time::Duration::days(30),
        })
        .collect()
}

fn archived_channel(channel: Channel) -> ArchivedChannel {
    ArchivedChannel {
        channel,
        archived_at: time(),
        deletes_at: Some(time() + time::Duration::days(30)),
    }
}

fn workspace_settings(
    archive: Option<u32>,
    files: Option<u32>,
    recordings: Option<u32>,
) -> WorkspaceSettings {
    WorkspaceSettings {
        archive_period: ArchivePeriod { days: archive },
        file_retention: Retention { days: files },
        recording_retention: Retention { days: recordings },
    }
}

#[tokio::test(start_paused = true)]
async fn a_collection_is_read_for_its_first_reader_and_leaves_the_state_with_its_last() {
    let (rig, _peer, admin) = served_rig().await;
    let part = AdminPart::Accounts;
    let mut accounts = vec![account("Ana"), account("Bruno")];
    admin.lock().unwrap().accounts = vec![accounts[1].clone(), accounts[0].clone()];
    assert_eq!(rig.model().admin.accounts, Collection::NotLoaded);
    assert_eq!(rig.http.count("/accounts"), 0);

    soon("the accounts", rig.live.open_admin(part))
        .await
        .unwrap();
    accounts.sort_by_key(|held| held.id);
    assert_eq!(
        rig.model().admin.accounts,
        Collection::Loaded(accounts.clone())
    );
    assert_eq!(rig.http.count("/accounts"), 1);
    // A second reader gets the accounts that are held.
    soon("the second reader", rig.live.open_admin(part))
        .await
        .unwrap();
    assert_eq!(rig.http.count("/accounts"), 1);

    // The accounts stay for the reader that is left.
    rig.live.release_admin(part);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.model().admin.accounts, Collection::Loaded(accounts));
    rig.live.release_admin(part);
    rig.model_when("no reader", |model| model.admin.accounts.is_not_loaded())
        .await;
    assert_eq!(rig.http.count("/accounts"), 1);

    // The last reader leaves while the read is on its way: the request ends, and no
    // answer is applied.
    let gate = rig.http.hold("/accounts");
    let opening = rig.live.open_admin(part);
    rig.model_when("the read", |model| {
        model.admin.accounts == Collection::Loading
    })
    .await;
    asked(&rig, "/accounts", 2).await;
    rig.live.release_admin(part);
    rig.model_when("no reader", |model| model.admin.accounts.is_not_loaded())
        .await;
    assert!(matches!(
        soon("the end of the wait", opening).await,
        Err(LoadError::Off)
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.http.routes.lock().unwrap().active, 0);
    gate.add_permits(1);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.model().admin.accounts, Collection::NotLoaded);
    assert_eq!(rig.http.count("/accounts"), 2);
}

#[tokio::test(start_paused = true)]
async fn the_events_of_the_time_of_a_read_are_applied_to_its_answer() {
    let (rig, peer, admin) = served_rig().await;
    let part = AdminPart::Invites;
    let own = rig.scene.account_id;
    let (early, answered, later) = (invite(own), invite(own), invite(own));

    // No reader has the invites open: the event changes nothing.
    peer.event(
        41,
        Event::InviteCreated {
            invite: early.clone(),
        },
    )
    .await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert_eq!(model.admin.invites, Collection::NotLoaded);

    // The server holds the first invite with a use that this reader was not told of.
    let mut early_used = early;
    early_used.uses = 1;
    admin.lock().unwrap().invites = vec![early_used.clone(), answered.clone()];
    let gate = rig.http.hold("/invites");
    let opening = rig.live.open_admin(part);
    asked(&rig, "/invites", 1).await;
    peer.event(
        42,
        Event::InviteCreated {
            invite: later.clone(),
        },
    )
    .await;
    peer.event(
        43,
        Event::InviteUsed {
            invite_id: answered.id,
            uses: 2,
        },
    )
    .await;
    let model = rig
        .model_when("the events", |model| model.cursor.position == 43)
        .await;
    assert_eq!(model.admin.invites, Collection::Loading);
    gate.add_permits(1);
    soon("the invites", opening).await.unwrap();

    // The list is the answer with the two events after the request.
    let mut answered_used = answered;
    answered_used.uses = 2;
    let mut newest_first = vec![early_used, answered_used, later];
    newest_first.sort_by_key(|held| std::cmp::Reverse(held.id));
    assert_eq!(rig.model().admin.invites, Collection::Loaded(newest_first));
    assert_eq!(rig.http.count("/invites"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_read_that_fails_tells_its_reader_and_the_state_and_is_sent_again_on_demand() {
    let (rig, _peer, admin) = served_rig().await;
    let general = rig.scene.general;
    let refusals = [
        (
            AdminPart::JoinRequests,
            "/join-requests",
            403,
            "forbidden",
            ErrorCode::Forbidden,
            LoadFailure::NotAllowed,
        ),
        (
            AdminPart::Overrides(general),
            "/overrides",
            404,
            "not_found",
            ErrorCode::NotFound,
            LoadFailure::NotFound,
        ),
        (
            AdminPart::ServerRoles,
            "/server-roles",
            500,
            "internal",
            ErrorCode::Internal,
            LoadFailure::Unavailable,
        ),
    ];
    for (part, end, status, name, code, failure) in refusals {
        rig.http.fail(end, status, name);
        let refused = soon("the refusal", rig.live.open_admin(part)).await;
        assert!(
            matches!(refused, Err(LoadError::Request(error)) if error.code() == Some(code)),
            "{part:?}"
        );
        assert_eq!(
            rig.model().admin.state(part),
            CollectionState::Failed(failure)
        );
        assert_eq!(rig.http.count(end), 1);
    }
    assert_eq!(
        rig.model().admin.join_requests,
        Collection::Failed(LoadFailure::NotAllowed)
    );
    assert_eq!(
        rig.model().admin.overrides(general),
        &Collection::Failed(LoadFailure::NotFound)
    );

    // No answer arrives.
    admin.lock().unwrap().lost.push("/exports");
    let unanswered = soon("the failure", rig.live.open_admin(AdminPart::Exports)).await;
    assert!(matches!(unanswered, Err(LoadError::Request(error))
        if matches!(&*error, ApiError::Unreachable(told)
            if told.kind == TransportErrorKind::Interrupted)));
    assert_eq!(
        rig.model().admin.exports,
        Collection::Failed(LoadFailure::NoAnswer)
    );
    assert_eq!(rig.http.count("/exports"), 1);

    // Each reader asks again, and the server answers.
    let rule = ChannelOverride {
        role_id: rig.scene.helpers,
        allow: vec![WorkspacePermission::ManageMessages],
        deny: Vec::new(),
    };
    admin.lock().unwrap().overrides = vec![rule.clone()];
    for (part, end) in [
        (AdminPart::JoinRequests, "/join-requests"),
        (AdminPart::Overrides(general), "/overrides"),
        (AdminPart::ServerRoles, "/server-roles"),
        (AdminPart::Exports, "/exports"),
    ] {
        rig.live.read_admin_again(part);
        rig.model_when("the collection", |model| {
            model.admin.state(part) == CollectionState::Loaded
        })
        .await;
        assert_eq!(rig.http.count(end), 2, "{part:?}");
    }
    assert_eq!(
        rig.model().admin.overrides(general),
        &Collection::Loaded(vec![rule])
    );

    // A collection that is loaded is not read again.
    rig.live.read_admin_again(AdminPart::Overrides(general));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.http.count("/overrides"), 2);
}

#[tokio::test(start_paused = true)]
async fn a_reload_reads_every_open_collection_again_and_closes_the_ones_of_a_lost_workspace() {
    let (rig, mut peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let (invites, list, settings) = (
        AdminPart::Invites,
        AdminPart::Archived(workspace_id),
        AdminPart::WorkspaceSettings(workspace_id),
    );
    let routes = [
        "/invites",
        "/archived-channels",
        "/archive-period",
        "/file-retention",
        "/recording-retention",
    ];
    let first = invite(rig.scene.account_id);
    let old = archived_channel(channel(workspace_id, ChannelKind::Text, "old", 7));
    {
        let mut admin = admin.lock().unwrap();
        admin.invites = vec![first.clone()];
        admin.archived = vec![old];
        admin.settings = Some(workspace_settings(Some(30), None, Some(90)));
    }
    for part in [invites, list, settings] {
        soon("the collection", rig.live.open_admin(part))
            .await
            .unwrap();
    }
    assert_eq!(counts(&rig, routes), [1, 1, 1, 1, 1]);

    // The server changes every collection, and no event tells this client.
    let second = invite(rig.scene.account_id);
    let changed = workspace_settings(Some(60), Some(14), None);
    {
        let mut admin = admin.lock().unwrap();
        admin.invites.push(second.clone());
        admin.archived.clear();
        admin.settings = Some(changed.clone());
    }
    reload(&rig, &mut peer, 90, |_| {}).await;
    let model = rig
        .model_when("the collections of the new state", |model| {
            model.cursor.position == 90
                && [invites, list, settings]
                    .iter()
                    .all(|part| model.admin.state(*part) == CollectionState::Loaded)
        })
        .await;
    let mut newest_first = vec![first, second];
    newest_first.sort_by_key(|held| std::cmp::Reverse(held.id));
    assert_eq!(model.admin.invites, Collection::Loaded(newest_first));
    assert_eq!(
        model.admin.archived(workspace_id),
        &Collection::Loaded(Vec::new())
    );
    assert_eq!(
        model.admin.workspace_settings(workspace_id),
        &Collection::Loaded(changed)
    );
    assert_eq!(counts(&rig, routes), [2, 2, 2, 2, 2]);

    // The account loses the workspace. Two readers open its collections while the
    // snapshot is on its way.
    let gate = rig.http.hold("/snapshot");
    rig.set_snapshot(95, |snapshot| {
        snapshot.workspaces.clear();
        snapshot.account.workspace_ids.clear();
    });
    peer.event(
        91,
        Event::PermissionsChanged {
            workspace_id: Some(workspace_id),
        },
    )
    .await;
    rig.when("the reload", |view| view.status == StreamStatus::Connecting)
        .await;
    let waiting = [rig.live.open_admin(list), rig.live.open_admin(settings)];
    tokio::time::sleep(Duration::from_secs(1)).await;
    gate.add_permits(1);
    peer.serve().await;
    for answer in waiting {
        assert!(matches!(
            soon("the answer", answer).await,
            Err(LoadError::NotInState)
        ));
    }
    let model = rig
        .model_when("the invites of the new state", |model| {
            model.cursor.position == 95 && model.admin.state(invites) == CollectionState::Loaded
        })
        .await;
    assert!(model.workspace(workspace_id).is_none());
    assert_eq!(model.admin.state(list), CollectionState::NotLoaded);
    assert_eq!(model.admin.state(settings), CollectionState::NotLoaded);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(counts(&rig, routes), [3, 2, 2, 2, 2]);
}

#[tokio::test(start_paused = true)]
async fn a_collection_of_a_place_that_the_state_does_not_hold_is_not_read() {
    let (mut rig, _admin) = rig_with_admin();
    // The server has no event stream.
    for part in [
        AdminPart::Invites,
        AdminPart::Archived(rig.scene.workspace_id),
    ] {
        let answer = soon("the answer", rig.live.open_admin(part)).await;
        assert!(matches!(answer, Err(LoadError::Off)), "{part:?}");
    }

    rig.want(ConnectionLevel::Selected, "jst_one");
    let _served = rig.served().await;
    for part in [
        AdminPart::Archived(WorkspaceId::generate()),
        AdminPart::WorkspaceSettings(WorkspaceId::generate()),
        AdminPart::Overrides(ChannelId::generate()),
    ] {
        let answer = soon("the answer", rig.live.open_admin(part)).await;
        assert!(matches!(answer, Err(LoadError::NotInState)), "{part:?}");
        assert_eq!(rig.model().admin.state(part), CollectionState::NotLoaded);
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let seen = rig.http.routes.lock().unwrap().seen.clone();
    assert_eq!(
        seen,
        [("/api/v1/snapshot".to_owned(), "jst_one".to_owned())]
    );
}

#[tokio::test(start_paused = true)]
async fn a_change_of_the_retention_reads_the_open_settings_again_and_keeps_them_on_screen() {
    let (rig, peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let of_workspace = Event::RetentionChanged {
        workspace_id: Some(workspace_id),
    };
    let of_direct = Event::RetentionChanged { workspace_id: None };

    // No reader has the settings open: nothing is read.
    peer.event(41, of_workspace.clone()).await;
    peer.event(42, of_direct.clone()).await;
    rig.model_when("the events", |model| model.cursor.position == 42)
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(counts(&rig, SETTINGS), [0, 0, 0]);
    assert_eq!(rig.http.count(DIRECT), 0);

    // The retention of the files of direct messages.
    let (ten_days, never) = (Retention { days: Some(10) }, Retention { days: None });
    admin.lock().unwrap().direct_retention = Some(ten_days.clone());
    soon(
        "the retention",
        rig.live.open_admin(AdminPart::DirectRetention),
    )
    .await
    .unwrap();
    assert_eq!(
        rig.model().admin.direct_retention,
        Collection::Loaded(ten_days.clone())
    );
    admin.lock().unwrap().direct_retention = Some(never.clone());
    let gate = rig.http.hold(DIRECT);
    peer.event(43, of_direct).await;
    asked(&rig, DIRECT, 2).await;
    // The value of before stays while the read is on its way.
    let model = rig
        .model_when("the event", |model| model.cursor.position == 43)
        .await;
    assert_eq!(model.admin.direct_retention, Collection::Loaded(ten_days));
    gate.add_permits(1);
    rig.model_when("the new retention", |model| {
        model.admin.direct_retention.loaded() == Some(&never)
    })
    .await;
    assert_eq!(rig.http.count(DIRECT), 2);
    assert_eq!(counts(&rig, SETTINGS), [0, 0, 0]);

    // The settings of the workspace: the three routes are read again.
    let part = AdminPart::WorkspaceSettings(workspace_id);
    let before = workspace_settings(Some(30), None, Some(90));
    let after = workspace_settings(Some(60), Some(14), None);
    admin.lock().unwrap().settings = Some(before.clone());
    soon("the settings", rig.live.open_admin(part))
        .await
        .unwrap();
    assert_eq!(
        rig.model().admin.workspace_settings(workspace_id),
        &Collection::Loaded(before.clone())
    );
    assert_eq!(counts(&rig, SETTINGS), [1, 1, 1]);
    admin.lock().unwrap().settings = Some(after.clone());
    let gate = rig.http.hold("/archive-period");
    peer.event(44, of_workspace).await;
    asked(&rig, "/archive-period", 2).await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 44)
        .await;
    assert_eq!(
        model.admin.workspace_settings(workspace_id),
        &Collection::Loaded(before)
    );
    gate.add_permits(1);
    rig.model_when("the new settings", |model| {
        model.admin.workspace_settings(workspace_id).loaded() == Some(&after)
    })
    .await;
    assert_eq!(counts(&rig, SETTINGS), [2, 2, 2]);

    // The retention of a workspace whose settings no reader has open.
    peer.event(
        45,
        Event::RetentionChanged {
            workspace_id: Some(WorkspaceId::generate()),
        },
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 45)
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(counts(&rig, SETTINGS), [2, 2, 2]);
    assert_eq!(rig.http.count(DIRECT), 2);
}

#[tokio::test(start_paused = true)]
async fn a_change_of_the_retention_during_the_first_read_starts_one_more_read_after_the_answer() {
    let (rig, peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let part = AdminPart::WorkspaceSettings(workspace_id);
    let after = workspace_settings(Some(60), Some(14), None);
    admin.lock().unwrap().settings = Some(workspace_settings(Some(30), None, Some(90)));
    let gate = rig.http.hold("/archive-period");
    let opening = rig.live.open_admin(part);
    asked(&rig, "/archive-period", 1).await;

    admin.lock().unwrap().settings = Some(after.clone());
    peer.event(
        41,
        Event::RetentionChanged {
            workspace_id: Some(workspace_id),
        },
    )
    .await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert_eq!(model.admin.state(part), CollectionState::Loading);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        counts(&rig, SETTINGS),
        [1, 0, 0],
        "the read that is on its way goes on, and no second read starts beside it"
    );

    // The answer can be older than the change: the settings are read once more.
    gate.add_permits(64);
    soon("the settings", opening).await.unwrap();
    rig.model_when("the settings of after the change", |model| {
        model.admin.workspace_settings(workspace_id).loaded() == Some(&after)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(counts(&rig, SETTINGS), [2, 2, 2]);
}

#[tokio::test(start_paused = true)]
async fn the_glossary_of_a_workspace_is_read_again_after_its_event_and_held_up_to_a_bound() {
    let (rig, peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let part = AdminPart::Glossary(workspace_id);
    let terms = |terms: &[&str]| -> Vec<String> { terms.iter().map(|t| (*t).to_owned()).collect() };
    let glossary = |workspace_id, held: &[&str]| junto_protocol::transcript::Glossary {
        workspace_id,
        terms: terms(held),
    };
    admin.lock().unwrap().glossary = Some(glossary(workspace_id, &["Quorvex", "Lumitrax"]));
    soon("the glossary", rig.live.open_admin(part))
        .await
        .unwrap();
    assert_eq!(
        rig.model().admin.glossary(workspace_id),
        &Collection::Loaded(GlossaryList {
            terms: terms(&["Quorvex", "Lumitrax"]),
            cut: false,
        })
    );
    assert_eq!(rig.http.count("/glossary"), 1);

    // The event names the workspace and carries no term: the glossary is read again, and
    // the list of before stays while the read is on its way.
    admin.lock().unwrap().glossary = Some(glossary(workspace_id, &["Lumitrax"]));
    let gate = rig.http.hold("/glossary");
    peer.event(41, Event::GlossaryReplaced { workspace_id })
        .await;
    asked(&rig, "/glossary", 2).await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert_eq!(
        model.admin.glossary(workspace_id).loaded().unwrap().terms,
        terms(&["Quorvex", "Lumitrax"])
    );
    gate.add_permits(64);
    rig.model_when("the new glossary", |model| {
        model
            .admin
            .glossary(workspace_id)
            .loaded()
            .is_some_and(|held| held.terms == terms(&["Lumitrax"]))
    })
    .await;

    // The glossary of another workspace is not read again.
    peer.event(
        42,
        Event::GlossaryReplaced {
            workspace_id: WorkspaceId::generate(),
        },
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 42)
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.http.count("/glossary"), 2);

    // A server that gives more terms than a glossary has: the client holds the first
    // ones and states that the list is cut.
    let many: Vec<String> = (0..=MOST_GLOSSARY_TERMS)
        .map(|number| format!("term {number}"))
        .collect();
    admin.lock().unwrap().glossary = Some(junto_protocol::transcript::Glossary {
        workspace_id,
        terms: many.clone(),
    });
    peer.event(43, Event::GlossaryReplaced { workspace_id })
        .await;
    let model = rig
        .model_when("the long glossary", |model| {
            model
                .admin
                .glossary(workspace_id)
                .loaded()
                .is_some_and(|held| held.cut)
        })
        .await;
    let held = model.admin.glossary(workspace_id).loaded().unwrap();
    assert_eq!(held.terms, many[..MOST_GLOSSARY_TERMS]);

    // An answer for another workspace is not taken (ADR 0714).
    admin.lock().unwrap().glossary = Some(glossary(WorkspaceId::generate(), &["Other"]));
    peer.event(44, Event::GlossaryReplaced { workspace_id })
        .await;
    rig.model_when("the refused answer", |model| {
        matches!(model.admin.glossary(workspace_id), Collection::Failed(_))
    })
    .await;

    // The last reader releases it: the state drops it.
    rig.live.release_admin(part);
    rig.model_when("the glossary is released", |model| {
        model.admin.glossary(workspace_id).is_not_loaded()
    })
    .await;
}

/// A reader that knows of a change newer than the reads that started so far asks for a read
/// now: a loaded collection is read again and keeps its values while the read is on its way.
#[tokio::test(start_paused = true)]
async fn a_loaded_collection_is_read_now_when_a_reader_asks_and_keeps_its_values_meanwhile() {
    let (rig, _peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let part = AdminPart::Glossary(workspace_id);
    let terms = |terms: &[&str]| -> Vec<String> { terms.iter().map(|t| (*t).to_owned()).collect() };
    let glossary = |held: &[&str]| junto_protocol::transcript::Glossary {
        workspace_id,
        terms: terms(held),
    };
    admin.lock().unwrap().glossary = Some(glossary(&["Quorvex"]));
    soon("the glossary", rig.live.open_admin(part))
        .await
        .unwrap();
    let first = rig.model().admin.read_times(part);
    assert_eq!(rig.http.count("/glossary"), 1);

    admin.lock().unwrap().glossary = Some(glossary(&["Quorvex", "Kestrel"]));
    let gate = rig.http.hold("/glossary");
    rig.live.read_admin_now(part);
    asked(&rig, "/glossary", 2).await;
    let model = rig
        .model_when("the read starts", |model| {
            model.admin.read_times(part).started != first.started
        })
        .await;
    assert_eq!(
        model.admin.glossary(workspace_id).loaded().unwrap().terms,
        terms(&["Quorvex"])
    );
    assert_eq!(model.admin.read_times(part).shown, first.shown);
    gate.add_permits(64);
    let model = rig
        .model_when("the read of now", |model| {
            model
                .admin
                .glossary(workspace_id)
                .loaded()
                .is_some_and(|held| held.terms == terms(&["Quorvex", "Kestrel"]))
        })
        .await;
    let times = model.admin.read_times(part);
    assert_eq!(times.shown, times.started);

    // A collection that no reader has open is not read.
    rig.live.release_admin(part);
    rig.model_when("the glossary is released", |model| {
        model.admin.glossary(workspace_id).is_not_loaded()
    })
    .await;
    rig.live.read_admin_now(part);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.http.count("/glossary"), 2);
}

#[tokio::test(start_paused = true)]
async fn access_tokens_are_read_in_pages_up_to_a_bound() {
    let (rig, _peer, admin) = served_rig().await;
    let part = AdminPart::AccessTokens;
    let loaded = |model: &ServerModel| -> Vec<AccessTokenId> {
        let held = model.admin.access_tokens.loaded();
        held.map(|held| held.iter().map(|token| token.id).collect())
            .unwrap_or_default()
    };
    let newest_first = |held: &[AccessTokenInfo]| -> Vec<AccessTokenId> {
        held.iter().rev().map(|token| token.id).collect()
    };

    // Two pages are one list.
    let held = tokens(rig.scene.account_id, PAGE + 50);
    admin.lock().unwrap().tokens = held.clone();
    soon("the tokens", rig.live.open_admin(part)).await.unwrap();
    let model = rig.model();
    assert_eq!(loaded(&model), newest_first(&held[..]));
    assert!(!model.admin.access_tokens_more);
    assert_eq!(
        admin.lock().unwrap().token_queries,
        [(None, PAGE), (Some(held[50].id), PAGE)]
    );

    // A page that names a further page and adds no token ends the read.
    rig.live.release_admin(part);
    rig.model_when("no reader", |model| {
        model.admin.access_tokens.is_not_loaded()
    })
    .await;
    {
        let mut admin = admin.lock().unwrap();
        admin.token_queries.clear();
        admin.tokens_stall = true;
    }
    soon("the tokens", rig.live.open_admin(part)).await.unwrap();
    let model = rig.model();
    assert_eq!(loaded(&model), newest_first(&held[50..]));
    assert!(model.admin.access_tokens_more);
    assert_eq!(admin.lock().unwrap().token_queries.len(), 2);

    // The server holds more tokens than the client reads.
    rig.live.release_admin(part);
    rig.model_when("no reader", |model| {
        model.admin.access_tokens.is_not_loaded()
    })
    .await;
    let many = tokens(rig.scene.account_id, MOST_TOKENS + 300);
    {
        let mut admin = admin.lock().unwrap();
        admin.token_queries.clear();
        admin.tokens_stall = false;
        admin.tokens = many.clone();
    }
    soon("the tokens", rig.live.open_admin(part)).await.unwrap();
    let model = rig.model();
    assert_eq!(loaded(&model), newest_first(&many[300..]));
    assert!(model.admin.access_tokens_more);
    let pages = admin.lock().unwrap().token_queries.clone();
    assert_eq!(pages.len(), MOST_TOKENS / PAGE);
    assert!(pages.iter().all(|(_, limit)| *limit == PAGE));
    assert_eq!(rig.http.count("/access-tokens"), 4 + MOST_TOKENS / PAGE);
}

#[tokio::test(start_paused = true)]
async fn an_archived_channel_stays_with_its_reader_until_the_archived_list_does_not_hold_it() {
    let (rig, mut peer, admin) = reading_rig().await;
    let (workspace_id, general) = (rig.scene.workspace_id, rig.scene.general);
    let place = PlaceId::Channel(general);

    let gone = archived_channel(rig.model().channel(general).unwrap().channel.clone());
    peer.event(
        41,
        Event::ChannelArchived {
            channel: gone.clone(),
        },
    )
    .await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert!(model.channel(general).is_none());
    assert_eq!(positions(&model, place), [1, 2, 3]);
    assert_eq!(model.admin.archived_channel(general), Some(&gone));

    // The reader also opens the archived channels of the workspace.
    let list = AdminPart::Archived(workspace_id);
    admin.lock().unwrap().archived = vec![gone.clone()];
    soon("the archived channels", rig.live.open_admin(list))
        .await
        .unwrap();
    let model = rig.model();
    assert_eq!(
        model.admin.archived(workspace_id),
        &Collection::Loaded(vec![gone.clone()])
    );
    assert_eq!(positions(&model, place), [1, 2, 3]);

    // The snapshot names no archived channel. The channel and its messages stay while
    // the archived list is on its way, and after it when the list holds the channel.
    let gate = rig.http.hold("/archived-channels");
    rig.set_snapshot(90, |snapshot| {
        snapshot.workspaces[0]
            .channels
            .retain(|held| held.id != general);
    });
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::TooFarBehind,
    })
    .await;
    peer.serve().await;
    let model = rig
        .model_when("the snapshot", |model| model.cursor.position == 90)
        .await;
    assert!(model.channel(general).is_none());
    assert_eq!(model.admin.state(list), CollectionState::Loading);
    assert_eq!(model.admin.archived_channel(general), Some(&gone));
    assert!(model.timeline(place).is_some());
    gate.add_permits(64);
    let model = rig
        .model_when("the archived channels again", |model| {
            model.admin.state(list) == CollectionState::Loaded
        })
        .await;
    assert_eq!(model.admin.archived_channel(general), Some(&gone));
    assert_eq!(positions(&model, place), [1, 2, 3]);
    assert_eq!(rig.http.count("/archived-channels"), 2);
    rig.when("live", |view| view.status == StreamStatus::Live)
        .await;

    // The list does not hold the channel: the server deleted it, and its messages go.
    admin.lock().unwrap().archived.clear();
    reload(&rig, &mut peer, 95, |_| {}).await;
    let model = rig
        .model_when("the archived channels without the channel", |model| {
            model.cursor.position == 95 && model.admin.state(list) == CollectionState::Loaded
        })
        .await;
    assert!(model.admin.archived_channel(general).is_none());
    assert!(model.timeline(place).is_none());
    assert_eq!(rig.http.count("/archived-channels"), 3);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(rig.model().timeline(place).is_none());
}

#[tokio::test(start_paused = true)]
async fn a_carried_archived_channel_is_checked_without_a_reader_of_its_list() {
    use junto_protocol::WorkspacePermission::ViewChannel;
    let (rig, mut peer, admin) = reading_rig().await;
    let (workspace_id, general) = (rig.scene.workspace_id, rig.scene.general);
    let place = PlaceId::Channel(general);
    let list = AdminPart::Archived(workspace_id);
    let part = AdminPart::ChannelPermissions(general);

    // The channel is archived while a reader has its messages open, and the reader opens
    // the permissions of the channel. No reader holds the archived list.
    let gone = archived_channel(rig.model().channel(general).unwrap().channel.clone());
    peer.event(
        41,
        Event::ChannelArchived {
            channel: gone.clone(),
        },
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;
    admin.lock().unwrap().permissions = vec![ViewChannel];
    soon("the permissions of the channel", rig.live.open_admin(part))
        .await
        .unwrap();
    assert_eq!(rig.http.count("/archived-channels"), 0);

    // Events are missed, and the snapshot names no archived channel. The client reads the
    // archived list by itself, which holds the channel: the channel and its readers stay,
    // and the list is not kept.
    admin.lock().unwrap().archived = vec![gone.clone()];
    reload(&rig, &mut peer, 90, |snapshot| {
        snapshot.workspaces[0]
            .channels
            .retain(|held| held.id != general);
    })
    .await;
    rig.model_when("the first check", |model| {
        model.cursor.position == 90
            && rig.http.count("/archived-channels") == 1
            && model.admin.state(part) == CollectionState::Loaded
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let model = rig.model();
    assert_eq!(model.admin.archived_channel(general), Some(&gone));
    assert_eq!(positions(&model, place), [1, 2, 3]);
    assert_eq!(model.admin.state(part), CollectionState::Loaded);
    assert_eq!(model.admin.state(list), CollectionState::NotLoaded);

    // The server deletes the channel while events are missed. The list does not hold it:
    // the channel, its messages and its collection leave the state.
    admin.lock().unwrap().archived.clear();
    reload(&rig, &mut peer, 95, |_| {}).await;
    let model = rig
        .model_when("the second check", |model| {
            model.cursor.position == 95 && model.admin.archived_channel(general).is_none()
        })
        .await;
    assert_eq!(rig.http.count("/archived-channels"), 2);
    assert!(model.readable_channel(general).is_none());
    assert!(model.timeline(place).is_none());
    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
    assert_eq!(model.admin.state(list), CollectionState::NotLoaded);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let model = rig.model();
    assert!(model.readable_channel(general).is_none());
    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
    assert_eq!(rig.http.count("/archived-channels"), 2);
}

#[tokio::test(start_paused = true)]
async fn a_carried_archived_channel_is_checked_when_the_reader_releases_its_list() {
    use junto_protocol::WorkspacePermission::ViewChannel;
    let (rig, mut peer, admin) = reading_rig().await;
    let (workspace_id, general) = (rig.scene.workspace_id, rig.scene.general);
    let place = PlaceId::Channel(general);
    let list = AdminPart::Archived(workspace_id);
    let part = AdminPart::ChannelPermissions(general);

    // A reader has the messages, the archived list and the permissions of the channel open.
    let gone = archived_channel(rig.model().channel(general).unwrap().channel.clone());
    admin.lock().unwrap().archived = vec![gone.clone()];
    soon("the archived channels", rig.live.open_admin(list))
        .await
        .unwrap();
    peer.event(
        41,
        Event::ChannelArchived {
            channel: gone.clone(),
        },
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;
    admin.lock().unwrap().permissions = vec![ViewChannel];
    soon("the permissions of the channel", rig.live.open_admin(part))
        .await
        .unwrap();

    // The server deletes the channel while the events are missed. The read of the list
    // after the reload is on its way when the reader releases the list.
    admin.lock().unwrap().archived.clear();
    let gate = rig.http.hold("/archived-channels");
    reload(&rig, &mut peer, 90, |snapshot| {
        snapshot.workspaces[0]
            .channels
            .retain(|held| held.id != general);
    })
    .await;
    asked(&rig, "/archived-channels", 2).await;
    rig.live.release_admin(list);
    gate.add_permits(64);
    let model = rig
        .model_when("the channel is checked", |model| {
            model.admin.archived_channel(general).is_none()
        })
        .await;
    assert!(model.readable_channel(general).is_none());
    assert!(model.timeline(place).is_none());
    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
    assert_eq!(model.admin.state(list), CollectionState::NotLoaded);
}

#[tokio::test(start_paused = true)]
async fn a_new_session_sends_the_read_of_a_collection_again() {
    let (mut rig, _first, admin) = served_rig().await;
    let part = AdminPart::Invites;
    let held = invite(rig.scene.account_id);
    admin.lock().unwrap().invites = vec![held.clone()];
    let gate = rig.http.hold("/invites");
    let opening = rig.live.open_admin(part);
    asked(&rig, "/invites", 1).await;

    rig.want(ConnectionLevel::Selected, "jst_two");
    let mut peer = rig.peer().await;
    peer.hello(rig.epoch()).await;
    peer.serve().await;
    asked(&rig, "/invites", 2).await;
    assert_eq!(rig.model().admin.invites, Collection::Loading);
    gate.add_permits(64);
    soon("the invites", opening).await.unwrap();
    assert_eq!(rig.model().admin.invites, Collection::Loaded(vec![held]));
    let seen = rig.http.routes.lock().unwrap().seen.clone();
    let sessions: Vec<&str> = seen
        .iter()
        .filter(|(path, _)| path.ends_with("/invites"))
        .map(|(_, token)| token.as_str())
        .collect();
    assert_eq!(sessions, ["jst_one", "jst_two"]);
}

#[tokio::test(start_paused = true)]
async fn a_deleted_workspace_takes_its_open_collection_out_of_the_state() {
    let (rig, peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let list = AdminPart::Archived(workspace_id);
    let old = archived_channel(channel(workspace_id, ChannelKind::Text, "old", 7));
    admin.lock().unwrap().archived = vec![old.clone()];
    soon("the archived channels", rig.live.open_admin(list))
        .await
        .unwrap();
    assert_eq!(
        rig.model().admin.archived(workspace_id),
        &Collection::Loaded(vec![old.clone()])
    );

    peer.event(41, Event::WorkspaceDeleted { workspace_id })
        .await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert!(model.workspace(workspace_id).is_none());
    assert_eq!(model.admin.state(list), CollectionState::NotLoaded);
    assert!(model.admin.archived.is_empty());
    assert!(model.admin.archived_channel(old.channel.id).is_none());

    // The reader lets go of a collection that the state dropped already.
    rig.live.release_admin(list);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*rig.model(), *model);
    assert_eq!(rig.http.count("/archived-channels"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_workspace_that_is_deleted_during_the_read_of_its_collection_closes_the_collection() {
    let (rig, peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let list = AdminPart::Archived(workspace_id);
    let old = archived_channel(channel(workspace_id, ChannelKind::Text, "old", 7));
    admin.lock().unwrap().archived = vec![old.clone()];
    let gate = rig.http.hold("/archived-channels");
    let opening = rig.live.open_admin(list);
    asked(&rig, "/archived-channels", 1).await;

    peer.event(41, Event::WorkspaceDeleted { workspace_id })
        .await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert!(model.workspace(workspace_id).is_none());

    // The answer belongs to a workspace that the state does not hold any more.
    gate.add_permits(1);
    assert!(matches!(
        soon("the answer", opening).await,
        Err(LoadError::NotInState)
    ));
    let model = rig.model();
    assert!(model.workspace(workspace_id).is_none());
    assert_eq!(model.admin.state(list), CollectionState::NotLoaded);
    assert!(model.admin.archived.is_empty());
    assert!(model.admin.archived_channel(old.channel.id).is_none());

    rig.live.release_admin(list);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*rig.model(), *model);
    assert_eq!(rig.http.count("/archived-channels"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_channel_that_is_deleted_during_the_read_of_its_overrides_closes_the_overrides() {
    let (rig, peer, admin) = served_rig().await;
    let (workspace_id, general) = (rig.scene.workspace_id, rig.scene.general);
    let part = AdminPart::Overrides(general);
    admin.lock().unwrap().overrides = vec![ChannelOverride {
        role_id: rig.scene.helpers,
        allow: vec![WorkspacePermission::ManageMessages],
        deny: Vec::new(),
    }];
    let gate = rig.http.hold("/overrides");
    let opening = rig.live.open_admin(part);
    asked(&rig, "/overrides", 1).await;

    peer.event(
        41,
        Event::ChannelDeleted {
            workspace_id,
            channel_id: general,
        },
    )
    .await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert!(model.channel(general).is_none());

    // The answer belongs to a channel that the state does not hold any more.
    gate.add_permits(1);
    assert!(matches!(
        soon("the answer", opening).await,
        Err(LoadError::NotInState)
    ));
    let model = rig.model();
    assert!(model.channel(general).is_none());
    assert_eq!(model.admin.state(part), CollectionState::NotLoaded);
    assert!(model.admin.overrides.is_empty());

    rig.live.release_admin(part);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(*rig.model(), *model);
    assert_eq!(rig.http.count("/overrides"), 1);
}

#[tokio::test(start_paused = true)]
async fn several_changes_of_the_retention_during_one_read_start_one_more_read() {
    let (rig, peer, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let part = AdminPart::WorkspaceSettings(workspace_id);
    let after = workspace_settings(Some(60), Some(14), None);
    admin.lock().unwrap().settings = Some(workspace_settings(Some(30), None, Some(90)));
    let gate = rig.http.hold("/archive-period");
    let opening = rig.live.open_admin(part);
    asked(&rig, "/archive-period", 1).await;

    admin.lock().unwrap().settings = Some(after.clone());
    for id in 41..=43 {
        peer.event(
            id,
            Event::RetentionChanged {
                workspace_id: Some(workspace_id),
            },
        )
        .await;
    }
    rig.model_when("the events", |model| model.cursor.position == 43)
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    // The read that is on its way goes on, and no event starts a read beside it.
    assert_eq!(counts(&rig, SETTINGS), [1, 0, 0]);
    assert_eq!(rig.http.routes.lock().unwrap().active, 1);

    gate.add_permits(64);
    soon("the settings", opening).await.unwrap();
    rig.model_when("the settings of after the changes", |model| {
        model.admin.workspace_settings(workspace_id).loaded() == Some(&after)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(counts(&rig, SETTINGS), [2, 2, 2]);
    assert_eq!(rig.http.routes.lock().unwrap().active, 0);
}

#[tokio::test(start_paused = true)]
async fn the_answer_of_the_archived_list_does_not_close_a_channel_that_was_archived_during_its_read()
 {
    let (rig, peer, _admin) = reading_rig().await;
    let (workspace_id, general) = (rig.scene.workspace_id, rig.scene.general);
    let place = PlaceId::Channel(general);
    let list = AdminPart::Archived(workspace_id);
    // The server makes its answer before the channel is archived: an empty list.
    let gate = rig.http.hold("/archived-channels");
    let opening = rig.live.open_admin(list);
    asked(&rig, "/archived-channels", 1).await;

    let gone = archived_channel(rig.model().channel(general).unwrap().channel.clone());
    peer.event(
        41,
        Event::ChannelArchived {
            channel: gone.clone(),
        },
    )
    .await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert_eq!(model.admin.archived_channel(general), Some(&gone));
    assert_eq!(positions(&model, place), [1, 2, 3]);

    // The event is applied after the answer: the list holds the channel, and the reader
    // keeps its messages.
    gate.add_permits(1);
    soon("the archived channels", opening).await.unwrap();
    let model = rig.model();
    assert!(model.timeline(place).is_some());
    assert_eq!(positions(&model, place), [1, 2, 3]);
    assert_eq!(
        model.admin.archived(workspace_id),
        &Collection::Loaded(vec![gone])
    );
    assert!(model.place(place).is_some());
    assert_eq!(rig.http.count("/archived-channels"), 1);
}

#[tokio::test(start_paused = true)]
async fn the_events_of_an_archived_list_read_do_not_change_the_channel_list_again() {
    let (rig, peer, _admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let list = AdminPart::Archived(workspace_id);
    let gate = rig.http.hold("/archived-channels");
    let opening = rig.live.open_admin(list);
    asked(&rig, "/archived-channels", 1).await;

    // A channel that the snapshot does not hold is restored and then renamed.
    let restored = channel(workspace_id, ChannelKind::Text, "old", 7);
    let mut renamed = restored.clone();
    renamed.name = "new".to_owned();
    peer.event(
        41,
        Event::ChannelRestored {
            channel: restored.clone(),
        },
    )
    .await;
    peer.event(42, Event::ChannelUpdated { channel: renamed })
        .await;
    let model = rig
        .model_when("the events", |model| model.cursor.position == 42)
        .await;
    assert_eq!(model.channel(restored.id).unwrap().channel.name, "new");

    gate.add_permits(1);
    soon("the archived channels", opening).await.unwrap();
    let model = rig.model();
    assert_eq!(model.channel(restored.id).unwrap().channel.name, "new");
    assert_eq!(
        model.admin.archived(workspace_id),
        &Collection::Loaded(Vec::new())
    );
}

#[tokio::test(start_paused = true)]
async fn a_new_session_during_a_refresh_of_loaded_settings_sends_the_read_again() {
    let (mut rig, first, admin) = served_rig().await;
    let workspace_id = rig.scene.workspace_id;
    let part = AdminPart::WorkspaceSettings(workspace_id);
    let before = workspace_settings(Some(30), None, Some(90));
    let after = workspace_settings(Some(60), Some(14), None);
    admin.lock().unwrap().settings = Some(before.clone());
    soon("the settings", rig.live.open_admin(part))
        .await
        .unwrap();

    // The refresh after the event is on its way, and the settings of before stay.
    admin.lock().unwrap().settings = Some(after.clone());
    let gate = rig.http.hold("/archive-period");
    first
        .event(
            41,
            Event::RetentionChanged {
                workspace_id: Some(workspace_id),
            },
        )
        .await;
    asked(&rig, "/archive-period", 2).await;
    let model = rig
        .model_when("the event", |model| model.cursor.position == 41)
        .await;
    assert_eq!(
        model.admin.workspace_settings(workspace_id),
        &Collection::Loaded(before)
    );

    rig.want(ConnectionLevel::Selected, "jst_two");
    let mut peer = rig.peer().await;
    peer.hello(rig.epoch()).await;
    peer.serve().await;
    asked(&rig, "/archive-period", 3).await;
    gate.add_permits(64);
    rig.model_when("the new settings", |model| {
        model.admin.workspace_settings(workspace_id).loaded() == Some(&after)
    })
    .await;
    assert_eq!(counts(&rig, SETTINGS), [3, 2, 2]);
    let seen = rig.http.routes.lock().unwrap().seen.clone();
    for end in SETTINGS {
        let last = seen.iter().rfind(|(path, _)| path.ends_with(end)).unwrap();
        assert_eq!(last.1, "jst_two", "{end}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_permissions_in_an_archived_channel_are_read_again_after_a_reload_and_stay_open() {
    use junto_protocol::WorkspacePermission::{ManageChannel, ViewChannel};
    let (rig, mut peer, admin) = served_rig().await;
    let (workspace_id, general) = (rig.scene.workspace_id, rig.scene.general);
    let list = AdminPart::Archived(workspace_id);
    soon("the archived channels", rig.live.open_admin(list))
        .await
        .unwrap();
    let gone = archived_channel(rig.model().channel(general).unwrap().channel.clone());
    admin.lock().unwrap().archived = vec![gone.clone()];
    peer.event(
        41,
        Event::ChannelArchived {
            channel: gone.clone(),
        },
    )
    .await;
    let model = rig
        .model_when("the archived channel", |model| {
            model.admin.archived_channel(general).is_some()
        })
        .await;
    // The snapshot stated the list of the workspace for the channel. An archived channel
    // has no list until its route answered (ADR 0310).
    assert_eq!(model.permissions_in(general), None);

    let part = AdminPart::ChannelPermissions(general);
    admin.lock().unwrap().permissions = vec![ViewChannel, ManageChannel];
    soon("the permissions of the channel", rig.live.open_admin(part))
        .await
        .unwrap();
    let model = rig.model();
    assert!(model.holds_in(general, ManageChannel));
    assert_eq!(rig.http.count("/permissions"), 1);

    // The permissions of the account changed: the state is replaced. The snapshot names
    // no archived channel, and the collection of the channel is read again without its
    // reader asking, while the archived list is still on its way.
    admin.lock().unwrap().permissions = vec![ViewChannel];
    let gate = rig.http.hold("/archived-channels");
    rig.set_snapshot(90, |snapshot| {
        snapshot.workspaces[0]
            .channels
            .retain(|held| held.id != general);
    });
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::TooFarBehind,
    })
    .await;
    peer.serve().await;
    let model = rig
        .model_when("the permissions after the reload", |model| {
            model.cursor.position == 90
                && model.admin.channel_permissions(general).loaded().is_some()
                && !model.holds_in(general, ManageChannel)
        })
        .await;
    assert!(model.holds_in(general, ViewChannel));
    assert_eq!(model.admin.state(list), CollectionState::Loading);
    assert_eq!(rig.http.count("/permissions"), 2);
    gate.add_permits(64);
    let model = rig
        .model_when("the archived channels again", |model| {
            model.admin.state(list) == CollectionState::Loaded
        })
        .await;
    assert_eq!(model.admin.state(part), CollectionState::Loaded);
    assert!(model.holds_in(general, ViewChannel));

    // The reader releases the collection: nothing of it stays.
    rig.live.release_admin(part);
    let model = rig
        .model_when("the released permissions", |model| {
            model.admin.state(part) == CollectionState::NotLoaded
        })
        .await;
    assert_eq!(model.permissions_in(general), None);
}
