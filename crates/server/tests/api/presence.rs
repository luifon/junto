use junto_protocol::ids::AccountId;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::support::TestServer;

#[tokio::test]
async fn presence_defaults_to_online_and_unconnected_or_unknown_accounts_are_offline() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let unknown = AccountId::generate();
    let page: Value = server
        .post(
            &owner,
            "/api/v1/presence",
            &json!({
                "accounts": [owner.account_id, unknown], "idle": false,
            }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(page["own"], "online");
    assert_eq!(page["accounts"].as_array().unwrap().len(), 2);
    assert!(
        page["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|account| account["presence"] == "offline")
    );
}

use crate::{stream_support::Frame, support::Member};
use junto_protocol::{
    events::{ClientMessage, Level, ServerMessage},
    presence::{Presence, PresenceChoice, PresencePage},
};

async fn read(
    server: &TestServer,
    caller: &Member,
    accounts: &[AccountId],
    idle: bool,
) -> PresencePage {
    server
        .post(
            caller,
            "/api/v1/presence",
            &json!({"accounts": accounts, "idle": idle}),
        )
        .await
        .ok(StatusCode::OK)
}

fn status(page: &PresencePage, account: AccountId) -> Presence {
    page.accounts
        .iter()
        .find(|row| row.account_id == account)
        .unwrap()
        .presence
}

#[tokio::test]
async fn devices_aggregate_idle_and_saved_choices_preserve_quiet_settings() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let person = server.join(&owner, "Member").await;
    let second = crate::devices::link_second_device(&server, &person).await;
    let _first_stream = server.subscribed(&person, Level::Full).await;
    let second_stream = server.subscribed(&second, Level::Background).await;
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Online
    );
    read(&server, &person, &[], true).await;
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Online
    );
    read(&server, &second, &[], true).await;
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Idle
    );
    read(&server, &second, &[], false).await;
    let mut changed = server.state.streams.presence_watch();
    drop(second_stream);
    tokio::time::timeout(std::time::Duration::from_secs(20), changed.changed())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Idle
    );
    let quiet = json!({"start_minute": 1320, "end_minute": 480, "time_zone": "UTC"});
    server
        .put(
            &person,
            "/api/v1/notification-settings/00000000-0000-0000-0000-000000000000",
            &json!({"level":"mentions", "daily_quiet":quiet}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    for (choice, expected) in [
        ("idle", Presence::Idle),
        ("do_not_disturb", Presence::DoNotDisturb),
        ("invisible", Presence::Offline),
        ("online", Presence::Idle),
    ] {
        server
            .put(
                &person,
                "/api/v1/account/presence",
                &json!({"choice": choice}),
            )
            .await
            .ok::<Value>(StatusCode::OK);
        let page = read(&server, &owner, &[person.account_id], false).await;
        assert_eq!(status(&page, person.account_id), expected, "{choice}");
        let settings: Value = server
            .get(
                &person,
                "/api/v1/notification-settings/00000000-0000-0000-0000-000000000000",
            )
            .await
            .ok(StatusCode::OK);
        assert_eq!(settings["daily_quiet"], quiet);
        assert_eq!(settings["do_not_disturb"], choice == "do_not_disturb");
        let own = read(&server, &person, &[person.account_id], true).await;
        if choice == "invisible" {
            assert_eq!(own.own, PresenceChoice::Invisible);
            assert_eq!(status(&own, person.account_id), Presence::Invisible);
        }
    }
}

#[tokio::test]
async fn presence_obeys_profile_visibility_and_cannot_disclose_a_hidden_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let workspace: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Other"}))
        .await
        .ok(StatusCode::CREATED);
    let hidden = server.join_into(&owner, "Separate", &[workspace.id]).await;
    let _stream = server.subscribed(&hidden, Level::Full).await;
    let unknown = AccountId::generate();
    let page = read(&server, &member, &[hidden.account_id, unknown], false).await;
    assert_eq!(status(&page, hidden.account_id), Presence::Offline);
    assert_eq!(status(&page, unknown), Presence::Offline);
    assert_eq!(
        status(
            &read(&server, &owner, &[hidden.account_id], false).await,
            hidden.account_id
        ),
        Presence::Online
    );
    server
        .put(
            &hidden,
            "/api/v1/account/presence",
            &json!({"choice":"invisible"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(
        status(
            &read(&server, &owner, &[hidden.account_id], false).await,
            hidden.account_id
        ),
        Presence::Offline
    );
    let excessive = vec![unknown; 101];
    server
        .post(
            &member,
            "/api/v1/presence",
            &json!({"accounts":excessive,"idle":false}),
        )
        .await
        .error(junto_protocol::error::ErrorCode::InvalidRequest);
    server
        .put(
            &member,
            "/api/v1/account/presence",
            &json!({"choice":"unknown"}),
        )
        .await
        .error(junto_protocol::error::ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn presence_invalidation_is_opt_in_and_moves_no_durable_cursor() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let mut legacy = server.subscribed(&owner, Level::Full).await;
    let mut stream = server.stream(&owner).await;
    stream
        .send(&ClientMessage::Subscribe {
            level: Level::Background,
            cursor: None,
            presence: true,
            appearance: false,
        })
        .await;
    assert!(matches!(
        stream.next().await,
        Frame::Message(ServerMessage::Subscribed { .. })
    ));
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::PresenceChanged {})
    );
    let cursor = server.snapshot(&owner).await.cursor;
    let _member_stream = server.subscribed(&member, Level::Full).await;
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::PresenceChanged {})
    );
    read(&server, &member, &[], true).await;
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::PresenceChanged {})
    );
    assert_eq!(server.snapshot(&owner).await.cursor, cursor);
    assert_eq!(
        legacy.within(std::time::Duration::from_millis(100)).await,
        None
    );
}

#[tokio::test]
async fn invisible_connections_and_input_do_not_emit_public_invalidations() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    server
        .put(
            &member,
            "/api/v1/account/presence",
            &json!({"choice":"invisible"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let mut observer = server.stream(&owner).await;
    observer
        .send(&ClientMessage::Subscribe {
            level: Level::Full,
            cursor: None,
            presence: true,
            appearance: false,
        })
        .await;
    assert!(matches!(
        observer.next().await,
        Frame::Message(ServerMessage::Subscribed { .. })
    ));
    assert_eq!(
        observer.next().await,
        Frame::Message(ServerMessage::PresenceChanged {})
    );
    let stream = server.subscribed(&member, Level::Full).await;
    read(&server, &member, &[], true).await;
    drop(stream);
    assert_eq!(
        observer.within(std::time::Duration::from_millis(100)).await,
        None
    );
}

#[tokio::test]
async fn expired_sessions_are_offline_before_cleanup_and_reconnect_keeps_manual_idle() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let person = server.join(&owner, "Member").await;
    let stream = server.subscribed(&person, Level::Full).await;
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Online
    );
    sqlx::query("update sessions set expires_at=$2 where device_id=$1")
        .bind(person.device_id)
        .bind(server.clock.now())
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Offline
    );
    let session = server.login(&person.key).await;
    let person = Member {
        token: session.token.expose().to_owned(),
        ..person
    };
    drop(stream);
    let stream = server.subscribed(&person, Level::Background).await;
    server
        .put(
            &person,
            "/api/v1/account/presence",
            &json!({"choice":"idle"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Idle
    );
    drop(stream);
    let _stream = server.subscribed(&person, Level::Full).await;
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Idle
    );
    server
        .delete(&person, "/api/v1/auth/session")
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        status(
            &read(&server, &owner, &[person.account_id], false).await,
            person.account_id
        ),
        Presence::Offline
    );
}

#[tokio::test]
async fn presence_visibility_changes_with_membership_and_administrator_role() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let observer = server.join(&owner, "Observer").await;
    let target = server.join(&owner, "Target").await;
    let _stream = server.subscribed(&target, Level::Background).await;
    let workspace = server.default_workspace(&owner).await;
    let separate: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Separate"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                separate.id, target.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        status(
            &read(&server, &observer, &[target.account_id], false).await,
            target.account_id
        ),
        Presence::Online
    );
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, target.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        status(
            &read(&server, &observer, &[target.account_id], false).await,
            target.account_id
        ),
        Presence::Offline
    );
    assert_eq!(
        status(
            &read(&server, &owner, &[target.account_id], false).await,
            target.account_id
        ),
        Presence::Online
    );
    let role = server
        .grant_server(
            &owner,
            &target,
            &[junto_protocol::ServerPermission::Administrator],
        )
        .await;
    assert_eq!(
        status(
            &read(&server, &observer, &[target.account_id], false).await,
            target.account_id
        ),
        Presence::Online
    );
    server
        .delete(
            &owner,
            &format!("/api/v1/accounts/{}/server-roles/{role}", target.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        status(
            &read(&server, &observer, &[target.account_id], false).await,
            target.account_id
        ),
        Presence::Offline
    );
}
