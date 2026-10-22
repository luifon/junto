use junto_protocol::account::{AccountProfile, Avatar, ImageType, OwnAccount};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::structure::{Workspace, WorkspaceMember};
use junto_protocol::{ServerPermission, WorkspacePermission};
use junto_server::image::samples;
use reqwest::StatusCode;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::support::{Member, TestServer};

const AVATAR: &str = "/api/v1/account/avatar";

fn profile_path(member: &Member) -> String {
    format!("/api/v1/accounts/{}/profile", member.account_id)
}

fn avatar_path(member: &Member) -> String {
    format!("/api/v1/accounts/{}/avatar", member.account_id)
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn an_account_sets_its_name_and_the_accounts_that_see_it_are_told() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let fellow = server.join(&owner, "Fellow").await;
    let mut to_fellow = server.subscribed(&fellow, Level::Full).await;

    let account: OwnAccount = server
        .patch(&guest, "/api/v1/account", &json!({ "name": "  Grace  " }))
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.name, "Grace");
    assert_eq!(account.avatar, None);
    let expected = AccountProfile {
        id: guest.account_id,
        name: "Grace".to_owned(),
        avatar: None,
        departure: None,
    };
    assert_eq!(
        to_fellow.event().await.1,
        Event::AccountProfileUpdated {
            profile: expected.clone()
        }
    );
    let profile: AccountProfile = server
        .get(&fellow, &profile_path(&guest))
        .await
        .ok(StatusCode::OK);
    assert_eq!(profile, expected);
    let workspace = server.default_workspace(&owner).await;
    let members: Vec<WorkspaceMember> = server
        .get(
            &fellow,
            &format!("/api/v1/workspaces/{}/members", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(members.iter().any(|member| member.name == "Grace"));

    server
        .patch(&guest, "/api/v1/account", &json!({ "name": "Grace" }))
        .await
        .ok::<OwnAccount>(StatusCode::OK);
    assert!(server.traffic(&fellow, &mut to_fellow).await.is_empty());

    for name in ["", "   ", &"x".repeat(65), "line\nbreak"] {
        server
            .patch(&guest, "/api/v1/account", &json!({ "name": name }))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    server
        .patch(&guest, "/api/v1/account", &json!({}))
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn an_avatar_is_stored_served_and_removed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let fellow = server.join(&owner, "Fellow").await;
    let mut to_fellow = server.subscribed(&fellow, Level::Full).await;

    server
        .get(&fellow, &avatar_path(&guest))
        .await
        .error(ErrorCode::NotFound);

    let cases = [
        (samples::png(64, 48), ImageType::Png, 64, 48),
        (samples::jpeg(640, 480), ImageType::Jpeg, 640, 480),
        (samples::webp_lossy(320, 200), ImageType::Webp, 320, 200),
        (
            samples::webp_lossless(1024, 1024),
            ImageType::Webp,
            1024,
            1024,
        ),
        (
            samples::webp_extended(300, 400, false),
            ImageType::Webp,
            300,
            400,
        ),
    ];
    for (image, media_type, width, height) in cases {
        let expected = Avatar {
            hash: hash(&image),
            media_type,
            bytes: u32::try_from(image.len()).unwrap(),
            width,
            height,
        };
        let stored: Avatar = server
            .put_bytes(&guest, AVATAR, Some("text/html"), image.clone())
            .await
            .ok(StatusCode::OK);
        assert_eq!(stored, expected);
        assert_eq!(
            to_fellow.event().await.1,
            Event::AccountProfileUpdated {
                profile: AccountProfile {
                    id: guest.account_id,
                    name: "Guest".to_owned(),
                    avatar: Some(expected.clone()),
                    departure: None,
                }
            }
        );

        let served = server.get(&fellow, &avatar_path(&guest)).await;
        assert_eq!(served.status, StatusCode::OK);
        assert_eq!(served.bytes(), image);
        assert_eq!(served.header("content-type"), Some(media_type.media_type()));
        assert_eq!(
            served.header("etag"),
            Some(format!("\"{}\"", expected.hash).as_str())
        );
        assert_eq!(served.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(served.header("cache-control"), Some("private, no-cache"));

        let own: OwnAccount = server
            .get(&guest, "/api/v1/account")
            .await
            .ok(StatusCode::OK);
        assert_eq!(own.avatar, Some(expected));
    }

    let again = samples::webp_extended(300, 400, false);
    server
        .put_bytes(&guest, AVATAR, None, again)
        .await
        .ok::<Avatar>(StatusCode::OK);
    assert!(server.traffic(&fellow, &mut to_fellow).await.is_empty());

    server
        .delete(&guest, AVATAR)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        to_fellow.event().await.1,
        Event::AccountProfileUpdated {
            profile: AccountProfile {
                id: guest.account_id,
                name: "Guest".to_owned(),
                avatar: None,
                departure: None,
            }
        }
    );
    server
        .get(&fellow, &avatar_path(&guest))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&guest, AVATAR)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.traffic(&fellow, &mut to_fellow).await.is_empty());
}

#[tokio::test]
async fn an_avatar_is_bounded_and_recognized_by_its_bytes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let mut largest = samples::png(64, 64);
    largest.resize(256 * 1024, 0);
    server
        .put_bytes(&owner, AVATAR, None, largest.clone())
        .await
        .ok::<Avatar>(StatusCode::OK);
    largest.push(0);
    server
        .put_bytes(&owner, AVATAR, Some("image/png"), largest)
        .await
        .error(ErrorCode::ImageTooLarge);

    let refused = [
        Vec::new(),
        b"GIF89a\x10\x00\x10\x00".to_vec(),
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec(),
        b"<!doctype html><script>alert(1)</script>".to_vec(),
        samples::png(1025, 8),
        samples::png(8, 1025),
        samples::jpeg(2000, 2000),
        samples::webp_lossless(1025, 1),
        samples::webp_extended(4096, 4096, false),
        samples::animated_png(8, 8),
        samples::webp_extended(8, 8, true),
        samples::png(0, 8),
        samples::png(8, 8)[..20].to_vec(),
    ];
    for bytes in refused {
        for declared in ["image/png", "image/jpeg", "image/webp"] {
            server
                .put_bytes(&owner, AVATAR, Some(declared), bytes.clone())
                .await
                .error(ErrorCode::ImageUnsupported);
        }
    }
    let own: OwnAccount = server
        .get(&owner, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(own.avatar.unwrap().bytes, 256 * 1024);

    let stored: i64 = sqlx::query_scalar("select count(*) from account_avatars")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored, 1);
    let too_large = sqlx::query("update account_avatars set content = content || '\\x00'::bytea")
        .execute(&server.db)
        .await;
    assert!(too_large.is_err());
    let too_wide = sqlx::query("update account_avatars set width = 1025")
        .execute(&server.db)
        .await;
    assert!(too_wide.is_err());
    let other_type = sqlx::query("update account_avatars set media_type = 'image/gif'")
        .execute(&server.db)
        .await;
    assert!(other_type.is_err());
}

#[tokio::test]
async fn a_profile_is_shown_only_to_the_accounts_that_may_see_the_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let fellow = server.join(&owner, "Fellow").await;
    let moderator = server.join(&owner, "Moderator").await;
    server
        .grant_server(&owner, &moderator, &[ServerPermission::BanMembers])
        .await;
    let inviter = server.join(&owner, "Inviter").await;
    server
        .grant_server(&owner, &inviter, &[ServerPermission::CreateInvites])
        .await;
    let workspace = server.default_workspace(&owner).await;
    server
        .grant_workspace(&owner, &inviter, workspace.id, WorkspacePermission::ALL)
        .await;
    let elsewhere: Workspace = server
        .post(
            &owner,
            "/api/v1/workspaces",
            &json!({ "name": "Elsewhere" }),
        )
        .await
        .ok(StatusCode::CREATED);
    let distant = server.join_into(&owner, "Distant", &[elsewhere.id]).await;
    server
        .put_bytes(&distant, AVATAR, None, samples::png(8, 8))
        .await
        .ok::<Avatar>(StatusCode::OK);

    let mut to_fellow = server.subscribed(&fellow, Level::Full).await;
    let mut to_inviter = server.subscribed(&inviter, Level::Full).await;
    let mut to_moderator = server.subscribed(&moderator, Level::Full).await;
    let mut to_owner = server.subscribed(&owner, Level::Full).await;
    let mut to_distant = server.subscribed(&distant, Level::Background).await;

    server
        .patch(&distant, "/api/v1/account", &json!({ "name": "Far" }))
        .await
        .ok::<OwnAccount>(StatusCode::OK);
    for stream in [&mut to_moderator, &mut to_owner] {
        assert!(matches!(
            stream.event().await.1,
            Event::AccountProfileUpdated { profile } if profile.name == "Far"
        ));
    }
    assert!(server.traffic(&fellow, &mut to_fellow).await.is_empty());
    assert!(server.traffic(&inviter, &mut to_inviter).await.is_empty());
    assert!(server.traffic(&distant, &mut to_distant).await.is_empty());

    let absent = format!("/api/v1/accounts/{}/profile", uuid::Uuid::now_v7());
    let absent_avatar = format!("/api/v1/accounts/{}/avatar", uuid::Uuid::now_v7());
    for stranger in [&fellow, &inviter] {
        let hidden = server.get(stranger, &profile_path(&distant)).await;
        hidden.error(ErrorCode::NotFound);
        assert_eq!(
            hidden.observable(),
            server.get(stranger, &absent).await.observable()
        );
        let hidden = server.get(stranger, &avatar_path(&distant)).await;
        hidden.error(ErrorCode::NotFound);
        assert_eq!(
            hidden.observable(),
            server.get(stranger, &absent_avatar).await.observable()
        );
    }
    for viewer in [&owner, &moderator, &distant] {
        let profile: AccountProfile = server
            .get(viewer, &profile_path(&distant))
            .await
            .ok(StatusCode::OK);
        assert_eq!(profile.name, "Far");
        assert_eq!(
            server.get(viewer, &avatar_path(&distant)).await.status,
            StatusCode::OK
        );
    }
    server
        .send::<serde_json::Value>(
            reqwest::Method::GET,
            &avatar_path(&distant),
            None,
            None,
            None,
        )
        .await
        .error(ErrorCode::Unauthenticated);

    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                elsewhere.id, fellow.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&fellow, &profile_path(&distant))
        .await
        .ok::<AccountProfile>(StatusCode::OK);
    server
        .get(&distant, &profile_path(&fellow))
        .await
        .ok::<AccountProfile>(StatusCode::OK);
    server
        .get(&distant, &profile_path(&inviter))
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn members_and_call_participants_carry_avatar_descriptions() {
    use crate::calls_support::Scene;
    use crate::discovery_support::events;
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let avatar: Avatar = server
        .put_bytes(ann, AVATAR, None, samples::png(8, 8))
        .await
        .ok(StatusCode::OK);
    let members: Vec<WorkspaceMember> = server
        .get(
            ben,
            &format!("/api/v1/workspaces/{}/members", scene.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        members
            .iter()
            .find(|member| member.account_id == ann.account_id)
            .unwrap()
            .avatar
            .as_ref(),
        Some(&avatar)
    );
    let before = server.snapshot(ben).await.cursor;
    server.enter_call(ann, scene.voice.id).await;
    let snapshot = serde_json::to_value(server.snapshot(ben).await).unwrap();
    let text = snapshot.to_string();
    assert!(text.contains(&avatar.hash));
    let sent = events(server, ben, before, Level::Full).await;
    assert!(sent.iter().any(|event| matches!(event, Event::CallParticipantJoined { participant, .. } if participant.account_id == ann.account_id && participant.avatar.as_ref() == Some(&avatar))));
    let own = server.own_call(ann).await.call.unwrap();
    assert_eq!(own.call.participants[0].avatar.as_ref(), Some(&avatar));
    server
        .post(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                scene.workspace_id, ann.account_id
            ),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let before = server.snapshot(ben).await.cursor;
    server
        .put_empty(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                scene.workspace_id, ann.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let sent = events(server, ben, before, Level::Full).await;
    assert!(sent.iter().any(|event| matches!(event, Event::WorkspaceMemberAdded { member, .. } if member.account_id == ann.account_id && member.avatar.as_ref() == Some(&avatar))));
    server
        .delete(ann, AVATAR)
        .await
        .empty(StatusCode::NO_CONTENT);
    let members: Vec<WorkspaceMember> = server
        .get(
            ben,
            &format!("/api/v1/workspaces/{}/members", scene.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(
        members
            .iter()
            .find(|member| member.account_id == ann.account_id)
            .unwrap()
            .avatar
            .is_none()
    );
}

#[tokio::test]
async fn profile_metadata_reads_follow_the_members() {
    use crate::calls_support::Scene;
    use crate::plan_support::{analyze, explained, index_names, visited_rows};
    let scene = Scene::start().await;
    let server = &scene.server;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let call = server.own_call(&scene.ann).await.call.unwrap().call.id;
    // Other accounts of the server, all with an avatar, whose identifiers sort before those
    // of the members: a merge join on the account reads all of them before the members,
    // and a hash join reads all of them.
    sqlx::query(
        "insert into accounts(id,name,status,created_at)
        select lpad(to_hex(i),32,'0')::uuid,'Other','active',now() from generate_series(1,10000) i",
    )
    .execute(&server.db)
    .await
    .unwrap();
    sqlx::query("insert into account_avatars(account_id,media_type,content,hash,width,height,updated_at)
        select id,'image/png',decode('00','hex'),decode(repeat('00',32),'hex'),1,1,now() from accounts")
        .execute(&server.db).await.unwrap();
    analyze(
        &server.db,
        &[
            "accounts",
            "account_avatars",
            "workspace_members",
            "call_members",
            "invites",
        ],
    )
    .await;
    // The rows that a list reads follow its members under each way of joining that the
    // planner can choose from the statistics of the tables.
    for settings in [
        "",
        "set local enable_nestloop = off; set local enable_hashjoin = off",
        "set local enable_nestloop = off; set local enable_mergejoin = off",
    ] {
        for (name, statement, scope, only) in [
            (
                "workspace members",
                junto_server::routes::workspaces::MEMBER_LIST,
                scene.workspace_id.as_uuid(),
                false,
            ),
            (
                "call participants",
                junto_server::calls::store::PARTICIPANTS,
                call.as_uuid(),
                true,
            ),
        ] {
            let mut transaction = server.db.begin().await.unwrap();
            if !settings.is_empty() {
                sqlx::raw_sql(settings)
                    .execute(&mut *transaction)
                    .await
                    .unwrap();
            }
            let mut query = sqlx::query_scalar(explained(statement)).bind(scope);
            if only {
                query = query.bind(None::<uuid::Uuid>);
            }
            let plan: serde_json::Value = query.fetch_one(&mut *transaction).await.unwrap();
            assert!(
                index_names(&plan)
                    .iter()
                    .any(|name| name == "account_avatars_pkey"),
                "{name} with {settings:?}: {plan}"
            );
            assert!(
                visited_rows(&plan[0]["Plan"]) < 100,
                "{name} with {settings:?}: {plan}"
            );
        }
    }
    let plan: serde_json::Value = sqlx::query_scalar(explained("select i.id,creator.name from invites i join accounts creator on creator.id=i.created_by order by i.id desc"))
        .fetch_one(&server.db).await.unwrap();
    assert!(
        index_names(&plan)
            .iter()
            .any(|name| name == "accounts_pkey"),
        "{plan}"
    );
    assert!(visited_rows(&plan[0]["Plan"]) < 100, "{plan}");
}
