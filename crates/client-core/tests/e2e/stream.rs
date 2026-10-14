//! The event stream client and the state of a server against the real server.

use std::sync::Arc;
use std::time::Duration;

use junto_client_core::ServerHandle;
use junto_client_core::api::ApiError;
use junto_client_core::flow::access::{AccessAction, AccessState};
use junto_client_core::live::{LiveView, Notice, Notices, StreamFault, StreamStatus};
use junto_client_core::model::{Loadable, ServerModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::call::UpdateCallSettings;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::events::{EndReason, Event, Level};
use junto_client_core::protocol::ids::{ChannelId, WorkspaceId};
use junto_client_core::protocol::structure::{
    Channel, ChannelKind, CreateChannel, SetChannelOverride, UpdateChannel,
};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;

use crate::support::{Device, FaultProxy, TestServer, claim, join, logged_in};

pub const PATIENCE: Duration = Duration::from_secs(60);

/// Waits until the view of a server fulfils a condition.
pub async fn when(
    handle: &ServerHandle,
    what: &str,
    holds: impl FnMut(&LiveView) -> bool,
) -> LiveView {
    let mut watch = handle.live().watch();
    match tokio::time::timeout(PATIENCE, watch.wait_for(holds)).await {
        Ok(Some(view)) => view,
        _ => panic!(
            "after {PATIENCE:?} this did not hold: {what}; the status is {:?}",
            handle.live().view().status
        ),
    }
}

pub async fn model_when(
    handle: &ServerHandle,
    what: &str,
    mut holds: impl FnMut(&ServerModel) -> bool,
) -> Arc<ServerModel> {
    let view = when(handle, what, |view| {
        view.model.as_deref().is_some_and(&mut holds)
    })
    .await;
    view.model.unwrap()
}

/// Starts the event streams of a device, selects the server and waits for its state.
pub async fn live(device: &Device, handle: &ServerHandle) -> Arc<ServerModel> {
    device.client.start_event_streams().unwrap();
    device.client.select(Some(handle.id()));
    let view = when(handle, "the stream delivers", |view| {
        view.status == StreamStatus::Live
    })
    .await;
    assert_eq!(view.level, Some(Level::Full));
    view.model.unwrap()
}

pub async fn text_channel(owner: &ServerHandle, workspace_id: WorkspaceId, name: &str) -> Channel {
    channel(owner, workspace_id, name, ChannelKind::Text).await
}

pub async fn channel(
    owner: &ServerHandle,
    workspace_id: WorkspaceId,
    name: &str,
    kind: ChannelKind,
) -> Channel {
    let create = CreateChannel {
        name: name.to_owned(),
        kind,
        category_id: None,
        position: None,
    };
    owner
        .api()
        .create_channel(workspace_id, &create)
        .await
        .unwrap()
}

fn same(held: &ServerModel, fresh: &ServerModel) -> bool {
    held.account == fresh.account
        && held.own_call == fresh.own_call
        && held.invitations == fresh.invitations
        && held.direct_mutes == fresh.direct_mutes
        && held.workspaces.len() == fresh.workspaces.len()
        && held
            .workspaces
            .iter()
            .zip(&fresh.workspaces)
            .all(|(held, fresh)| {
                held.workspace == fresh.workspace
                    && held.own_role_ids == fresh.own_role_ids
                    && held.roles == fresh.roles
                    && held.categories == fresh.categories
                    && held.channels == fresh.channels
                    && held.permissions == fresh.permissions
                    && held.channel_permissions == fresh.channel_permissions
                    && held.notification_default == fresh.notification_default
            })
}

/// The state that the stream keeps is the state that a snapshot of now gives.
pub async fn agrees_with_a_snapshot(handle: &ServerHandle) {
    let started = tokio::time::Instant::now();
    loop {
        let snapshot = handle.api().snapshot().await.unwrap();
        let fresh = ServerModel::from_snapshot(snapshot);
        let held = handle.live().view().model.unwrap();
        if same(&held, &fresh) {
            return;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "the state differs from a snapshot:\n{held:#?}\n{fresh:#?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The notices that have arrived and those of the next moment.
pub async fn arrived(notices: &mut Notices) -> Vec<Notice> {
    let mut found = Vec::new();
    while let Ok(Some(notice)) =
        tokio::time::timeout(Duration::from_millis(300), notices.next()).await
    {
        found.push(notice.expect("the test read too slowly"));
    }
    found
}

/// Reads the notices of a server until `done` holds for what was read. The stream gives a
/// state first and its notices after it, so a test that needs a notice waits for the
/// notice itself and not for a time after the state.
pub async fn told_until(
    notices: &mut Notices,
    what: &str,
    mut done: impl FnMut(&[Notice]) -> bool,
) -> Vec<Notice> {
    let mut found = Vec::new();
    let reading = async {
        while !done(&found) {
            match notices.next().await {
                Some(notice) => found.push(notice.expect("the test read too slowly")),
                None => break,
            }
        }
    };
    let ended = tokio::time::timeout(PATIENCE, reading).await.is_ok();
    assert!(
        ended && done(&found),
        "after {PATIENCE:?} the notices did not tell: {what}; told: {found:?}"
    );
    found
}

fn created_channels(notices: &[Notice]) -> Vec<(u64, ChannelId)> {
    events(notices)
        .into_iter()
        .filter_map(|(id, event)| match event {
            Event::ChannelCreated { channel } => Some((id, channel.id)),
            _ => None,
        })
        .collect()
}

pub fn events(notices: &[Notice]) -> Vec<(u64, &Event)> {
    notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::Event { id, event, .. } => Some((*id, event.as_ref())),
            _ => None,
        })
        .collect()
}

fn reloads(notices: &[Notice]) -> usize {
    notices
        .iter()
        .filter(|notice| matches!(notice, Notice::Reloaded { .. }))
        .count()
}

pub struct Pair {
    pub server: TestServer,
    pub first: Device,
    pub second: Device,
    pub owner: ServerHandle,
    pub member: ServerHandle,
    pub workspace_id: WorkspaceId,
}

/// An owner and a member on two devices. The member reaches the server at `member_at`.
pub async fn pair(server: TestServer, member_at: &str) -> Pair {
    let (first, second) = (Device::new(), Device::new());
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = super::invite(&owner, 2).await;
    let member = second.add(member_at).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let workspace_id = owner.api().workspaces().await.unwrap()[0].id;
    Pair {
        server,
        first,
        second,
        owner,
        member,
        workspace_id,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_clients_follow_one_server_and_each_sees_what_its_account_may_see() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = pair(server, &address).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    assert_eq!(owner.live().view().status, StreamStatus::Off);
    assert!(owner.live().view().model.is_none());

    let model = live(&scene.first, owner).await;
    assert_eq!(model.account.id, logged_in(owner));
    assert!(model.is_administrator());
    let seen_by_member = live(&scene.second, member).await;
    assert_eq!(seen_by_member.account.name, "Grace");
    assert!(!seen_by_member.is_administrator());
    assert_eq!(seen_by_member.workspaces.len(), 1);
    let mut told_to_member = member.live().notices();

    // A channel that every member sees reaches both clients as an event.
    let plans = text_channel(owner, workspace_id, "plans").await;
    for handle in [owner, member] {
        model_when(handle, "the new channel", |model| {
            model.channel(plans.id).is_some()
        })
        .await;
    }
    let told = arrived(&mut told_to_member).await;
    assert!(
        events(&told).iter().any(
            |(_, event)| matches!(event, Event::ChannelCreated { channel } if channel.id == plans.id)
        ),
        "{told:?}"
    );
    assert_eq!(reloads(&told), 0);

    // The channel is hidden from the members. The member is told that its permissions
    // changed, loads the state again and no longer holds the channel.
    let everyone = model.workspaces[0]
        .roles
        .iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    let hide = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::ViewChannel],
    };
    owner
        .api()
        .set_channel_override(plans.id, everyone, &hide)
        .await
        .unwrap();
    model_when(member, "the channel is hidden", |model| {
        model.channel(plans.id).is_none()
    })
    .await;
    let told = arrived(&mut told_to_member).await;
    assert_eq!(reloads(&told), 1, "{told:?}");

    // A change of the hidden channel is told to the owner only.
    let rename = UpdateChannel {
        name: Some("secret-plans".to_owned()),
        ..UpdateChannel::default()
    };
    owner.api().update_channel(plans.id, &rename).await.unwrap();
    let marker = text_channel(owner, workspace_id, "marker").await;
    for handle in [owner, member] {
        model_when(handle, "the later channel", |model| {
            model.channel(marker.id).is_some()
        })
        .await;
    }
    let held = owner.live().view().model.unwrap();
    assert_eq!(held.channel(plans.id).unwrap().channel.name, "secret-plans");
    let told = arrived(&mut told_to_member).await;
    let names_hidden = |event: &Event| match event {
        Event::ChannelUpdated { channel } | Event::ChannelCreated { channel } => {
            channel.id == plans.id
        }
        Event::ChannelOverrideSet { .. } => true,
        _ => false,
    };
    assert!(
        events(&told).iter().all(|(_, event)| !names_hidden(event)),
        "{told:?}"
    );
    let held_by_member = member.live().view().model.unwrap();
    assert!(held_by_member.channel(plans.id).is_none());

    for handle in [owner, member] {
        agrees_with_a_snapshot(handle).await;
    }

    // A logout ends the stream and discards the state.
    member.dispatch(AccessAction::LogOut).await.unwrap();
    let view = when(member, "off", |view| view.status == StreamStatus::Off).await;
    assert!(view.model.is_none());
    assert_eq!(owner.live().view().status, StreamStatus::Live);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cut_connection_is_continued_from_the_cursor_without_a_snapshot() {
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let scene = pair(server, &proxy.address()).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    live(&scene.second, member).await;
    assert_eq!(proxy.seen("/api/v1/snapshot").len(), 1);
    assert_eq!(proxy.seen("/api/v1/events").len(), 1);
    // The reader of the notices exists before the first change, and the notice of that
    // change is read before the cut: no notice of the time before the cut is counted as
    // one of the replay.
    let mut told = member.live().notices();
    let before = text_channel(owner, workspace_id, "before").await;
    told_until(&mut told, "the channel", |told| {
        created_channels(told)
            .iter()
            .any(|(_, id)| *id == before.id)
    })
    .await;

    proxy.block_streams(true);
    proxy.cut_streams();
    let view = when(member, "the loss", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    assert!(matches!(
        view.status,
        StreamStatus::Interrupted(StreamFault::Unreachable(_) | StreamFault::Closed(_))
    ));
    // Without a connection nothing moves the cursor: its position at the loss is the one
    // that the replay continues from. A position that was read before the cut can be
    // older: a further event of the same change can arrive between the read and the cut.
    let position = view.model.as_ref().unwrap().cursor.position;
    let mut missed = Vec::new();
    for name in ["one", "two", "three"] {
        missed.push(text_channel(owner, workspace_id, name).await.id);
    }
    // The state is the last one that was known.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let stale = member.live().view().model.unwrap();
    assert!(stale.channel(before.id).is_some());
    assert!(missed.iter().all(|id| stale.channel(*id).is_none()));
    assert_eq!(stale.cursor.position, position);

    proxy.block_streams(false);
    let view = when(member, "the replay", |view| {
        view.status == StreamStatus::Live
            && view
                .model
                .as_ref()
                .is_some_and(|model| missed.iter().all(|id| model.channel(*id).is_some()))
    })
    .await;
    assert!(view.model.unwrap().cursor.position > position);
    assert_eq!(
        proxy.seen("/api/v1/snapshot").len(),
        1,
        "the events were replayed and no snapshot was loaded"
    );
    assert!(proxy.seen("/api/v1/events").len() >= 2);
    // A change after the replay ends what the test reads: an event that the replay told
    // a second time would stand before the notice of this change.
    let after = text_channel(owner, workspace_id, "after").await;
    let told = told_until(&mut told, "the replay and the channel after it", |told| {
        created_channels(told).iter().any(|(_, id)| *id == after.id)
    })
    .await;
    assert_eq!(reloads(&told), 0);
    let mut expected = missed.clone();
    expected.push(after.id);
    assert_eq!(
        created_channels(&told)
            .iter()
            .map(|(_, id)| *id)
            .collect::<Vec<_>>(),
        expected
    );
    let ids: Vec<u64> = events(&told).iter().map(|(id, _)| *id).collect();
    assert!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "an event was told twice or out of order: {ids:?}"
    );
    agrees_with_a_snapshot(member).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn more_events_than_the_replay_bound_while_cut_load_a_snapshot() {
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let scene = pair(server, &proxy.address()).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    live(&scene.second, member).await;
    let renamed = text_channel(owner, workspace_id, "start").await;
    model_when(member, "the channel", |model| {
        model.channel(renamed.id).is_some()
    })
    .await;

    proxy.block_streams(true);
    proxy.cut_streams();
    when(member, "the loss", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    // The model is published before its notices; exclude notices from before the cut.
    let mut told = member.live().notices();
    // The server replays at most 1000 events that the account may receive (ADR 0207).
    let api = owner.api();
    for round in 0..1005 {
        let rename = UpdateChannel {
            name: Some(format!("name-{round}")),
            ..UpdateChannel::default()
        };
        api.update_channel(renamed.id, &rename).await.unwrap();
    }
    proxy.block_streams(false);
    let view = when(member, "the state of now", |view| {
        view.status == StreamStatus::Live
            && view.model.as_ref().is_some_and(|model| {
                model
                    .channel(renamed.id)
                    .is_some_and(|held| held.channel.name == "name-1004")
            })
    })
    .await;
    assert_eq!(view.level, Some(Level::Full));
    assert_eq!(proxy.seen("/api/v1/snapshot").len(), 2);
    let told = arrived(&mut told).await;
    assert_eq!(reloads(&told), 1);
    assert!(
        events(&told).is_empty(),
        "{} events of the time of the cut were applied",
        events(&told).len()
    );

    // The stream goes on from the cursor of the snapshot.
    let after = text_channel(owner, workspace_id, "after").await;
    model_when(member, "the next event", |model| {
        model.channel(after.id).is_some()
    })
    .await;
    assert_eq!(proxy.seen("/api/v1/snapshot").len(), 2);
    agrees_with_a_snapshot(member).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restart_with_a_new_epoch_loads_a_snapshot() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let mut scene = pair(server, &address).await;
    let before = live(&scene.second, &scene.member).await;
    let mut told = scene.member.live().notices();

    scene.server.stop();
    when(&scene.member, "the loss", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    assert!(scene.member.live().view().model.is_some());
    scene.server.host_command(&["new-event-epoch"]);
    scene.server.restart().await;

    let view = when(&scene.member, "the new epoch", |view| {
        view.status == StreamStatus::Live
            && view
                .model
                .as_ref()
                .is_some_and(|model| model.cursor.epoch != before.cursor.epoch)
    })
    .await;
    assert_eq!(reloads(&arrived(&mut told).await), 1);
    assert_eq!(view.model.unwrap().workspaces, before.workspaces);

    let after = text_channel(&scene.owner, scene.workspace_id, "after").await;
    model_when(&scene.member, "an event of the new epoch", |model| {
        model.channel(after.id).is_some()
    })
    .await;
    agrees_with_a_snapshot(&scene.member).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_is_not_selected_gets_control_events_and_no_structure() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = pair(server, &address).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let owner_id = logged_in(owner);

    scene.second.client.start_event_streams().unwrap();
    let view = when(member, "the background level", |view| {
        view.status == StreamStatus::Live
    })
    .await;
    assert_eq!(view.level, Some(Level::Background));
    let mut told = member.live().notices();

    // A general event is not delivered at this level; a control event after it is.
    let unseen = text_channel(owner, workspace_id, "unseen").await;
    member
        .api()
        .mute_direct_conversation(owner_id, None)
        .await
        .unwrap();
    let model = model_when(member, "the mute", |model| !model.direct_mutes.is_empty()).await;
    assert_eq!(model.direct_mutes[0].account_id, owner_id);
    assert!(model.channel(unseen.id).is_none());
    let told_now = arrived(&mut told).await;
    assert!(
        events(&told_now)
            .iter()
            .all(|(_, event)| matches!(event, Event::DirectMuteSet { .. })),
        "{told_now:?}"
    );

    // The selection loads a snapshot and subscribes at the full level (ADR 0207).
    scene.second.client.select(Some(member.id()));
    let view = when(member, "the full level", |view| {
        view.level == Some(Level::Full)
    })
    .await;
    assert!(view.model.unwrap().channel(unseen.id).is_some());
    assert_eq!(reloads(&arrived(&mut told).await), 1);

    member
        .api()
        .unmute_direct_conversation(owner_id)
        .await
        .unwrap();
    model_when(member, "the mute is off", |model| {
        model.direct_mutes.is_empty()
    })
    .await;

    // The mode "connect only when selected" ends the connection of a server that is not
    // selected.
    member
        .set_connection_mode(junto_client_core::book::ConnectionMode::WhenSelected)
        .unwrap();
    assert_eq!(member.live().view().status, StreamStatus::Live);
    scene.second.client.select(None);
    let view = when(member, "off", |view| view.status == StreamStatus::Off).await;
    assert!(view.model.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn members_are_loaded_on_demand_and_follow_the_events() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let model = live(&first, &owner).await;
    let workspace_id = model.workspaces[0].workspace.id;
    assert_eq!(model.workspaces[0].members, Loadable::NotLoaded);

    owner.live().load_members(workspace_id).await.unwrap();
    let names = |model: &ServerModel| -> Vec<String> {
        let members = model.workspace(workspace_id).unwrap().members.loaded();
        let mut names: Vec<String> = members
            .map(|members| members.iter().map(|member| member.name.clone()).collect())
            .unwrap_or_default();
        names.sort();
        names
    };
    assert_eq!(names(&owner.live().view().model.unwrap()), ["Ada"]);

    let code = super::invite(&owner, 1).await;
    let second = Device::new();
    let member = second.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    model_when(&owner, "the new member", |model| {
        names(model) == ["Ada", "Grace"]
    })
    .await;
    member.api().update_profile("Grace H.").await.unwrap();
    model_when(&owner, "the new name", |model| {
        names(model) == ["Ada", "Grace H."]
    })
    .await;
    let listed = owner.api().workspace_members(workspace_id).await.unwrap();
    let held = owner.live().view().model.unwrap();
    assert_eq!(
        held.workspace(workspace_id).unwrap().members.loaded(),
        Some(&listed)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ban_ends_the_stream_and_the_access_flow_shows_it() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = pair(server, &address).await;
    let (owner, member) = (&scene.owner, &scene.member);
    let account_id = logged_in(member);
    live(&scene.second, member).await;
    let mut told = member.live().notices();

    owner.api().ban_account(account_id).await.unwrap();
    let view = when(member, "off", |view| view.status == StreamStatus::Off).await;
    assert!(view.model.is_none());
    // The stream reported the ended session, and the one login of ADR 0712 found the ban.
    assert!(matches!(member.access().state, AccessState::Banned { .. }));
    let told = arrived(&mut told).await;
    let banned = |notice: &Notice| {
        matches!(
            notice,
            Notice::Ended {
                reason: EndReason::Banned,
                ..
            }
        )
    };
    assert!(told.iter().any(banned), "{told:?}");

    owner.api().unban_account(account_id).await.unwrap();
    member.connect().await.unwrap();
    when(member, "live again", |view| {
        view.status == StreamStatus::Live
    })
    .await;
    agrees_with_a_snapshot(member).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_that_the_server_ended_is_renewed_and_the_stream_goes_on() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let scene = pair(server, &address).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let account_id = logged_in(member);
    let before = live(&scene.second, member).await;
    let mut told = member.live().notices();

    let options: PgConnectOptions = scene.server.database_url.parse().unwrap();
    let mut database = options.connect().await.unwrap();
    let expired = sqlx::query(
        "update sessions set expires_at = now() - interval '1 hour'
         where device_id in (select id from devices where account_id = $1)",
    )
    .bind(account_id.as_uuid())
    .execute(&mut database)
    .await
    .unwrap();
    assert_eq!(expired.rows_affected(), 1);

    // The next delivery finds the session expired. The client logs in again by itself,
    // and the new connection continues the state from its cursor.
    let created = text_channel(owner, workspace_id, "after-the-login").await;
    let model = model_when(member, "the channel", |model| {
        model.channel(created.id).is_some()
    })
    .await;
    assert_eq!(model.cursor.epoch, before.cursor.epoch);
    assert_eq!(logged_in(member), account_id);
    let told = arrived(&mut told).await;
    let ended = |notice: &Notice| {
        matches!(
            notice,
            Notice::Ended {
                reason: EndReason::SessionEnded,
                ..
            }
        )
    };
    assert!(told.iter().any(ended), "{told:?}");
    assert_eq!(reloads(&told), 0, "{told:?}");
    when(member, "live", |view| view.status == StreamStatus::Live).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_call_routes_answer_with_typed_results_and_their_events_reach_the_state() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let mut scene = pair(server, &address).await;
    let (owner, member, workspace_id) = (&scene.owner, &scene.member, scene.workspace_id);
    let (owner_id, member_id) = (logged_in(owner), logged_in(member));
    let voice = channel(owner, workspace_id, "Daily", ChannelKind::Voice).await;
    live(&scene.second, member).await;
    let api = member.api();

    // Success with a value.
    assert!(api.own_call().await.unwrap().call.is_none());
    let shown = api.channel_call(voice.id).await.unwrap();
    assert!(!shown.in_use && shown.call.is_none());
    assert_eq!(shown.channel_id, voice.id);

    // A typed refusal: this server has no media server, and the member may not change
    // the settings of the channel.
    let refused = |error: ApiError| error.code().expect("a refusal of the server");
    assert_eq!(
        refused(api.join_channel_call(voice.id).await.unwrap_err()),
        ErrorCode::MediaUnavailable
    );
    assert_eq!(
        refused(api.join_direct_call(owner_id).await.unwrap_err()),
        ErrorCode::MediaUnavailable
    );
    let settings = UpdateCallSettings {
        user_limit: Some(Some(4)),
        hides_occupants: Some(true),
    };
    let denied = api.update_call_settings(voice.id, &settings).await;
    assert_eq!(refused(denied.unwrap_err()), ErrorCode::Forbidden);
    assert_eq!(
        refused(api.report_call_state(true, false).await.unwrap_err()),
        ErrorCode::NotInCall
    );
    assert_eq!(
        refused(api.raise_hand().await.unwrap_err()),
        ErrorCode::NotInCall
    );
    // A recording belongs to a call that the caller is in. The member holds no
    // "record calls" either.
    let started = api.start_recording(voice.id, None).await;
    assert_eq!(refused(started.unwrap_err()), ErrorCode::Forbidden);
    let stopped = api.stop_recording(voice.id).await;
    assert_eq!(refused(stopped.unwrap_err()), ErrorCode::Forbidden);
    let started = owner.api().start_recording(voice.id, Some("pt")).await;
    assert_eq!(refused(started.unwrap_err()), ErrorCode::NotInCall);
    // A leave that names a place is answered as a leave without a place to end.
    api.leave_call_as("no-such-place").await.unwrap();
    api.leave_call().await.unwrap();

    // A change that the owner makes reaches the state of the member as an event.
    let changed = owner
        .api()
        .update_call_settings(voice.id, &settings)
        .await
        .unwrap();
    assert_eq!(changed.user_limit, Some(4));
    assert!(changed.hides_occupants);
    let model = model_when(member, "the settings", |model| {
        model
            .channel(voice.id)
            .is_some_and(|held| held.channel.hides_occupants)
    })
    .await;
    assert_eq!(model.channel(voice.id).unwrap().channel.user_limit, Some(4));

    // A mute belongs to the account that set it.
    owner
        .api()
        .mute_direct_conversation(member_id, None)
        .await
        .unwrap();
    let marker = text_channel(owner, workspace_id, "marker").await;
    let model = model_when(member, "a later event", |model| {
        model.channel(marker.id).is_some()
    })
    .await;
    assert!(model.direct_mutes.is_empty());

    // A connection fault: the server does not answer.
    scene.server.stop();
    let error = api.own_call().await.unwrap_err();
    assert!(matches!(error, ApiError::Unreachable(_)), "{error}");
    assert!(error.code().is_none());
}
