//! End-to-end tests: the client library and `junto-cli` against the real server binary of
//! this checkout and PostgreSQL. Run them through `tools/with-test-postgres.sh`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod admin;
mod calls_live;
mod departures;
#[allow(dead_code)]
mod limit;
mod limit_tests;
mod messages;
mod processes;
// The window tests use the same file, and parts of it that these tests do not use.
#[allow(dead_code)]
mod recording_server;
mod recordings;
mod remaining;
mod requests;
mod roles;
mod stream;
mod support;
mod threads;
mod tool_live;

use std::path::Path;
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, Instant};

use junto_client_core::api::{AccessLoss, ApiError};
use junto_client_core::book::{Membership, ProfileSync};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessAction, AccessState, ActionKind, Notice};
use junto_client_core::flow::add_server::{AddServerAction, AddServerState};
use junto_client_core::flow::approve_link::{ApproveLinkAction, ApproveLinkState, PendingApproval};
use junto_client_core::flow::link_all::{
    ApprovalProgress, ApproveDevicesAction, ApproveDevicesState, LinkDeviceAction, LinkDeviceState,
    LinkProgress,
};
use junto_client_core::profile::{AVATAR_MAX_BYTES, InvalidAvatar, ProfileError, avatar_hash};
use junto_client_core::protocol::account::{AccountStatus, ImageType, RecoverAccount};
use junto_client_core::protocol::audit::{AuditEvent, AuditFilterQuery};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::invite::{ApproveJoinRequest, CreateInvite};
use junto_client_core::protocol::secret::{InviteCode, OwnerCode};
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
use junto_client_core::version::{Incompatibility, SupportedVersions};
use junto_client_core::{Client, Reach};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;

use support::{Device, Fault, FaultProxy, TestServer, add, claim, join, logged_in};

async fn invite(owner: &junto_client_core::ServerHandle, uses: u32) -> InviteCode {
    owner
        .api()
        .create_invite(&CreateInvite {
            max_uses: Some(uses),
            ..CreateInvite::default()
        })
        .await
        .unwrap()
        .code
}

#[cfg(unix)]
fn files_under(directory: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_device_claims_a_new_server_with_the_printed_code() {
    let server = TestServer::start("Acme").await;
    let device = Device::new();

    let flow = device.client.add_server();
    flow.dispatch(AddServerAction::Fetch {
        address: server.address(),
    })
    .await
    .unwrap();
    let AddServerState::Preview(preview) = flow.view().state else {
        panic!("no preview");
    };
    assert_eq!(preview.name, "Acme");
    assert_eq!(preview.version.unwrap().number(), 1);
    assert!(device.client.servers().is_empty());
    flow.dispatch(AddServerAction::Save {
        connection_mode: Default::default(),
    })
    .await
    .unwrap();
    let handle = device.client.servers().remove(0);
    assert_eq!(handle.id(), preview.server_id);

    let wrong = AccessAction::ClaimOwner {
        code: OwnerCode::new("AAAA-AAAA-AAAA-AAAA-AAAA"),
        name: "Ada".to_owned(),
        device_name: String::new(),
    };
    let error = handle.dispatch(wrong).await.unwrap_err();
    assert!(matches!(
        error,
        FlowError::Refused {
            code: ErrorCode::OwnerCodeInvalid,
            ..
        }
    ));
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount { notice: None }
    );

    handle.dispatch(claim(&server, "Ada")).await.unwrap();
    let account_id = logged_in(&handle);
    let account = handle.api().own_account().await.unwrap();
    assert_eq!(account.id, account_id);
    assert_eq!(account.name, "Ada");
    assert!(account.is_owner);
    let devices = handle.api().devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].public_key, handle.device_public_key().unwrap());
    assert!(devices[0].current);

    let second_claim = Device::new();
    let late = second_claim.add(&server.address()).await;
    let error = late.dispatch(claim(&server, "Mallory")).await.unwrap_err();
    assert!(matches!(
        error,
        FlowError::Refused {
            code: ErrorCode::OwnerAlreadyClaimed,
            ..
        }
    ));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_key_file_is_private_on_the_disk_and_no_other_file_holds_a_secret() {
    use std::os::unix::fs::PermissionsExt;
    let server = TestServer::start("Acme").await;
    let device = Device::new();
    let handle = device.add(&server.address()).await;
    handle.dispatch(claim(&server, "Ada")).await.unwrap();
    handle.api().own_account().await.unwrap();

    let data_dir = device.client.data_dir();
    let key_file = data_dir.key_file(handle.id());
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode(&key_file), 0o600);
    assert_eq!(mode(&data_dir.keys_dir()), 0o700);
    assert_eq!(mode(data_dir.root()), 0o700);
    assert_eq!(mode(&data_dir.book_file()), 0o600);
    assert!(std::fs::symlink_metadata(&key_file).unwrap().is_file());

    let key = std::fs::read(&key_file).unwrap();
    let seed = &key[20..52];
    let files = files_under(data_dir.root());
    assert!(files.contains(&key_file));
    for path in files.iter().filter(|path| **path != key_file) {
        let content = std::fs::read(path).unwrap();
        assert!(
            !content.windows(seed.len()).any(|window| window == seed),
            "{}",
            path.display()
        );
        assert!(
            !content.windows(4).any(|window| window == b"jst_"),
            "{}",
            path.display()
        );
    }
    assert!(
        files_under(device.directory.path())
            .iter()
            .all(|path| path.starts_with(data_dir.root()))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_device_with_a_fresh_data_directory_joins_with_an_invite() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = invite(&owner, 1).await;

    let second = Device::new();
    let member = second.add(&server.address()).await;
    assert_eq!(
        member.access().state,
        AccessState::NoAccount { notice: None }
    );
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&member);
    assert_ne!(account_id, logged_in(&owner));
    assert_ne!(
        member.device_public_key().unwrap(),
        owner.device_public_key().unwrap()
    );

    let account = member.api().own_account().await.unwrap();
    assert_eq!(account.name, "Grace");
    assert!(!account.is_owner);
    let workspaces = member.api().workspaces().await.unwrap();
    assert_eq!(workspaces.len(), 1);
    assert!(workspaces[0].is_default);
    let channels = member.api().channels(workspaces[0].id).await.unwrap();
    assert_eq!(channels.channels.len(), 2);

    let error = member
        .api()
        .create_channel(
            workspaces[0].id,
            &CreateChannel {
                name: "plans".to_owned(),
                kind: ChannelKind::Text,
                category_id: None,
                position: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(ErrorCode::Forbidden));
    let created = owner
        .api()
        .create_channel(
            workspaces[0].id,
            &CreateChannel {
                name: "plans".to_owned(),
                kind: ChannelKind::Text,
                category_id: None,
                position: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(created.name, "plans");

    let log = owner
        .api()
        .server_audit_log(&AuditFilterQuery {
            limit: Some(10),
            ..AuditFilterQuery::default()
        })
        .await
        .unwrap();
    assert!(log.entries.iter().any(|entry| matches!(
        entry.event,
        AuditEvent::AccountJoined { account_id: joined, .. } if joined == account_id
    )));

    let third = Device::new();
    let late = third.add(&server.address()).await;
    let error = late.dispatch(join(&code, "Linus")).await.unwrap_err();
    assert!(matches!(
        error,
        FlowError::Refused {
            code: ErrorCode::InviteExhausted,
            ..
        }
    ));
    assert_eq!(late.access().state, AccessState::NoAccount { notice: None });
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_asks_for_access_and_collects_the_approval_and_the_denial() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();

    let second = Device::new();
    let asking = second.add(&server.address()).await;
    let request = AccessAction::RequestAccess {
        name: "Grace".to_owned(),
    };
    asking.dispatch(request.clone()).await.unwrap();
    let AccessState::WaitingForApproval {
        join_request_id: Some(join_request_id),
        ..
    } = asking.access().state
    else {
        panic!("not waiting");
    };
    asking.dispatch(AccessAction::Check).await.unwrap();
    assert!(matches!(
        asking.access().state,
        AccessState::WaitingForApproval { .. }
    ));

    let restarted = second.restarted();
    let asking = restarted.servers().remove(0);
    assert!(matches!(
        asking.access().state,
        AccessState::WaitingForApproval { .. }
    ));
    let waiting = owner.api().join_requests().await.unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(
        (waiting[0].id, waiting[0].name.as_str()),
        (join_request_id, "Grace")
    );

    let decided = {
        let asking = asking.clone();
        tokio::spawn(async move { asking.wait_for_decision(Duration::from_millis(300)).await })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    owner
        .api()
        .approve_join_request(join_request_id, &ApproveJoinRequest::default())
        .await
        .unwrap();
    let state = decided.await.unwrap().unwrap();
    assert!(matches!(state, AccessState::LoggedIn { .. }));
    assert_eq!(asking.api().own_account().await.unwrap().name, "Grace");
    assert!(matches!(
        asking.entry().membership,
        Membership::Member { .. }
    ));

    let third = Device::new();
    let refused = third.add(&server.address()).await;
    refused.dispatch(request).await.unwrap();
    let waiting = owner.api().join_requests().await.unwrap();
    owner.api().deny_join_request(waiting[0].id).await.unwrap();
    refused.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(
        refused.access().state,
        AccessState::NoAccount {
            notice: Some(Notice::RequestDenied)
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_client_logs_in_from_its_stored_key() {
    let mut server = TestServer::start("Acme").await;
    let device = Device::new();
    let handle = device.add(&server.address()).await;
    handle.dispatch(claim(&server, "Ada")).await.unwrap();
    let account_id = logged_in(&handle);
    let device_id = handle.api().devices().await.unwrap()[0].id;
    let public_key = handle.device_public_key().unwrap();
    drop(handle);

    let restarted = device.restarted();
    let handle = restarted.servers().remove(0);
    assert_eq!(handle.reach(), Reach::Unknown);
    assert_eq!(handle.access().state, AccessState::LoggedOut { account_id });
    assert_eq!(restarted.connect_all().await, [(handle.id(), Ok(()))]);
    assert_eq!(logged_in(&handle), account_id);
    let devices = handle.api().devices().await.unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(
        (devices[0].id, devices[0].public_key),
        (device_id, public_key)
    );
    assert!(devices[0].last_login_at.is_some());

    server.restart().await;
    assert_eq!(server.owner_code(), None);
    assert!(handle.api().own_account().await.unwrap().is_owner);

    handle.dispatch(AccessAction::LogOut).await.unwrap();
    assert!(matches!(
        handle.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));
    handle.dispatch(AccessAction::LogIn).await.unwrap();
    assert_eq!(logged_in(&handle), account_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_is_linked_from_another_data_directory_and_later_removed() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let account_id = logged_in(&owner);

    let second = Device::new();
    let newcomer = second.add(&server.address()).await;
    newcomer
        .dispatch(AccessAction::RequestLink {
            device_name: "Phone".to_owned(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForLink {
        code, public_key, ..
    } = newcomer.access().state
    else {
        panic!("not waiting for a link");
    };
    assert_eq!(public_key, newcomer.device_public_key().unwrap());
    newcomer.dispatch(AccessAction::Check).await.unwrap();
    assert!(matches!(
        newcomer.access().state,
        AccessState::WaitingForLink { .. }
    ));

    let approval = owner.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    let ApproveLinkState::Confirm(PendingApproval {
        public_key: shown,
        device_name,
        ..
    }) = approval.view().state
    else {
        panic!("nothing to confirm");
    };
    assert_eq!((shown, device_name.as_str()), (public_key, "Phone"));
    approval.dispatch(ApproveLinkAction::Approve).await.unwrap();
    let ApproveLinkState::Approved { device } = approval.view().state else {
        panic!("not approved");
    };

    newcomer.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(logged_in(&newcomer), account_id);
    assert!(newcomer.api().own_account().await.unwrap().is_owner);
    let devices = newcomer.api().devices().await.unwrap();
    assert_eq!(devices.len(), 2);
    assert!(
        devices
            .iter()
            .any(|listed| listed.id == device.id && listed.current)
    );

    owner.api().remove_device(device.id).await.unwrap();
    assert!(matches!(
        newcomer.api().own_account().await,
        Err(ApiError::AccessLost(AccessLoss::Removed))
    ));
    assert_eq!(
        newcomer.access().state,
        AccessState::NoAccount {
            notice: Some(Notice::Removed)
        }
    );
    assert_eq!(owner.api().devices().await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_banned_account_sees_its_state_and_returns_after_an_unban() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = invite(&owner, 2).await;
    let second = Device::new();
    let member = second.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&member);

    owner.api().ban_account(account_id).await.unwrap();
    assert!(matches!(
        member.api().workspaces().await,
        Err(ApiError::AccessLost(AccessLoss::Banned))
    ));
    let view = member.access();
    assert_eq!(
        view.state,
        AccessState::Banned {
            account_id: Some(account_id)
        }
    );
    assert_eq!(view.actions, [ActionKind::Check, ActionKind::ForgetAccount]);

    let restarted = second.restarted();
    let member = restarted.servers().remove(0);
    assert!(matches!(member.access().state, AccessState::Banned { .. }));
    member.connect().await.unwrap();
    assert!(matches!(member.access().state, AccessState::Banned { .. }));
    assert_eq!(
        member.dispatch(join(&code, "Grace")).await,
        Err(FlowError::NotAvailable)
    );
    let accounts = owner.api().accounts().await.unwrap();
    let listed = accounts.iter().find(|account| account.id == account_id);
    assert_eq!(listed.unwrap().status, AccountStatus::Banned);

    owner.api().unban_account(account_id).await.unwrap();
    member.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(logged_in(&member), account_id);
    member.api().workspaces().await.unwrap();

    owner.api().remove_account(account_id).await.unwrap();
    assert!(matches!(
        member.api().workspaces().await,
        Err(ApiError::AccessLost(AccessLoss::Removed))
    ));
    assert_eq!(
        member.access().state,
        AccessState::NoAccount {
            notice: Some(Notice::Removed)
        }
    );
    let key = member.device_public_key().unwrap();
    member.dispatch(join(&code, "Grace")).await.unwrap();
    assert_ne!(logged_in(&member), account_id);
    assert_eq!(member.device_public_key().unwrap(), key);
}

#[tokio::test(flavor = "multi_thread")]
async fn two_servers_are_held_at_once_and_a_stopped_one_does_not_affect_the_other() {
    let acme = TestServer::start("Acme").await;
    let mut globex = TestServer::start("Globex").await;
    let device = Device::new();
    let on_acme = device.add(&acme.address()).await;
    let on_globex = device.add(&globex.address()).await;
    on_acme.dispatch(claim(&acme, "Ada")).await.unwrap();
    on_globex.dispatch(claim(&globex, "Ada")).await.unwrap();
    let (acme_account, globex_account) = (logged_in(&on_acme), logged_in(&on_globex));
    assert_ne!(on_acme.id(), on_globex.id());
    assert_ne!(
        on_acme.device_public_key().unwrap(),
        on_globex.device_public_key().unwrap()
    );
    assert_eq!(on_acme.entry().name, "Acme");
    assert_eq!(on_globex.entry().name, "Globex");

    globex.stop();
    let restarted = device.restarted();
    let results = restarted.connect_all().await;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], (on_acme.id(), Ok(())));
    assert_eq!(results[1].0, on_globex.id());
    assert!(matches!(results[1].1, Err(FlowError::Unreachable(_))));
    let on_acme = restarted.server(on_acme.id()).unwrap();
    let on_globex = restarted.server(on_globex.id()).unwrap();
    assert_eq!(logged_in(&on_acme), acme_account);
    assert!(on_acme.api().own_account().await.unwrap().is_owner);
    assert_eq!(on_globex.reach(), Reach::Unreachable);
    assert_eq!(
        on_globex.access().state,
        AccessState::LoggedOut {
            account_id: globex_account
        }
    );
    assert!(matches!(
        on_globex.entry().membership,
        Membership::Member { .. }
    ));
    assert!(matches!(
        on_globex.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));

    globex.restart().await;
    on_globex.connect().await.unwrap();
    assert_eq!(logged_in(&on_globex), globex_account);
    assert!(matches!(on_globex.reach(), Reach::Reachable { .. }));
    assert_eq!(logged_in(&on_acme), acme_account);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_busy_answer_gets_the_same_signed_request_again_and_the_server_accepts_it() {
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let device = Device::new();
    let handle = device.add(&proxy.address()).await;

    proxy.fail("/api/v1/owner/claim", [Fault::Busy, Fault::Busy]);
    let started = Instant::now();
    handle.dispatch(claim(&server, "Ada")).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2));
    logged_in(&handle);
    assert!(handle.api().own_account().await.unwrap().is_owner);

    let claims = proxy.seen("/api/v1/owner/claim");
    assert_eq!(claims.len(), 3);
    assert!(claims.iter().all(|seen| seen.method == "POST"));
    assert!(!claims[0].body.is_empty());
    assert_eq!(claims[0].body, claims[1].body);
    assert_eq!(claims[1].body, claims[2].body);
    assert_eq!(proxy.seen("/api/v1/auth/challenges").len(), 1);

    proxy.fail("/api/v1/workspaces", [Fault::Busy]);
    assert_eq!(handle.api().workspaces().await.unwrap().len(), 1);
    assert_eq!(proxy.seen("/api/v1/workspaces").len(), 2);

    proxy.fail("/api/v1/workspaces", (0..5).map(|_| Fault::Busy));
    let error = handle.api().workspaces().await.unwrap_err();
    assert_eq!(error.code(), Some(ErrorCode::Busy));
    assert_eq!(proxy.seen("/api/v1/workspaces").len(), 7);
    logged_in(&handle);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_action_that_waits_for_the_flow_runs_when_the_running_action_has_ended() {
    const CLAIM: &str = "/api/v1/owner/claim";
    let server = TestServer::start("Acme").await;
    let proxy = FaultProxy::start(&server).await;
    let device = Device::new();
    let handle = device.add(&proxy.address()).await;

    // The claim waits at the proxy, so the access flow is busy until the test says so.
    proxy.hold(CLAIM);
    let claiming = tokio::spawn(handle.dispatch(claim(&server, "Ada")));
    let deadline = Instant::now() + Duration::from_secs(60);
    while proxy.seen(CLAIM).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the claim did not reach the proxy"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(matches!(
        handle.dispatch(AccessAction::LogOut).await,
        Err(FlowError::Busy)
    ));
    let logging_out = tokio::spawn(handle.dispatch_when_free(AccessAction::LogOut));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!claiming.is_finished() && !logging_out.is_finished());

    proxy.release();
    claiming.await.unwrap().unwrap();
    logging_out.await.unwrap().unwrap();
    assert!(matches!(
        handle.access().state,
        AccessState::LoggedOut { .. }
    ));
    assert_eq!(proxy.seen(CLAIM).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_join_whose_answer_was_lost_is_repeated_and_creates_one_account() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = invite(&owner, 5).await;

    let proxy = FaultProxy::start(&server).await;
    let second = Device::new();
    let member = second.add(&proxy.address()).await;
    proxy.fail("/api/v1/join", [Fault::LoseAnswer]);
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&member);
    assert_eq!(member.api().own_account().await.unwrap().name, "Grace");

    let joins = proxy.seen("/api/v1/join");
    assert_eq!(joins.len(), 2);
    assert_eq!(joins[0].body, joins[1].body);
    let accounts = owner.api().accounts().await.unwrap();
    assert_eq!(accounts.len(), 2);
    assert!(accounts.iter().any(|account| account.id == account_id));
    let invites = owner.api().invites().await.unwrap();
    assert_eq!(invites[0].uses, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_timed_out_on_a_database_lock_is_repeated_unchanged() {
    const CLAIM: &str = "/api/v1/owner/claim";
    let server = TestServer::start("Acme").await;
    let proxy = std::sync::Arc::new(FaultProxy::start(&server).await);
    let device = Device::new();
    let handle = device.add(&proxy.address()).await;

    let options = PgConnectOptions::from_str(&server.database_url).unwrap();
    let mut blocker = options.connect().await.unwrap();
    sqlx::query("begin").execute(&mut blocker).await.unwrap();
    sqlx::query("select 1 from server_state for update")
        .execute(&mut blocker)
        .await
        .unwrap();
    let release = {
        let proxy = proxy.clone();
        tokio::spawn(async move {
            let first_answer = loop {
                let answered = proxy
                    .seen(CLAIM)
                    .first()
                    .and_then(|seen| seen.answer.clone());
                match answered {
                    Some(answer) => break answer,
                    None => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            };
            sqlx::query("rollback").execute(&mut blocker).await.unwrap();
            first_answer
        })
    };

    handle.dispatch(claim(&server, "Ada")).await.unwrap();
    let (status, body) = release.await.unwrap();
    assert_eq!(status, 503);
    let refusal: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(refusal["error"]["code"], "timeout");

    let claims = proxy.seen(CLAIM);
    assert_eq!(claims.len(), 2);
    assert_eq!(claims[1].answer.as_ref().unwrap().0, 201);
    assert_eq!(claims[0].body, claims[1].body);
    let proofs: Vec<serde_json::Value> = claims
        .iter()
        .map(|seen| {
            serde_json::from_slice::<serde_json::Value>(&seen.body).unwrap()["proof"].clone()
        })
        .collect();
    assert!(proofs[0]["challenge_id"].is_string() && proofs[0]["signature"].is_string());
    assert_eq!(proofs[0], proofs[1]);

    let order: Vec<String> = proxy
        .all()
        .into_iter()
        .map(|seen| format!("{} {}", seen.method, seen.path))
        .collect();
    let first_claim = order.iter().position(|line| line.ends_with(CLAIM)).unwrap();
    assert_eq!(order[first_claim - 1], "POST /api/v1/auth/challenges");
    assert_eq!(order[first_claim + 1], format!("POST {CLAIM}"));
    let challenges = order
        .iter()
        .take(first_claim + 2)
        .filter(|line| line.ends_with("/auth/challenges"))
        .count();
    assert_eq!(challenges, 1);
    let challenge: serde_json::Value = serde_json::from_slice(
        &proxy.seen("/api/v1/auth/challenges")[0]
            .answer
            .as_ref()
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(challenge["id"], proofs[0]["challenge_id"]);

    assert!(handle.api().own_account().await.unwrap().is_owner);
    let accounts = handle.api().accounts().await.unwrap();
    assert_eq!(accounts.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_of_the_next_version_uses_the_previous_one_and_refuses_an_older_server() {
    let server = TestServer::start("Acme").await;
    let directory = tempfile::tempdir().unwrap();

    let next = Device::open(directory.path(), SupportedVersions::new(2, Some(1)));
    let handle = add(&next, &server.address()).await;
    handle.dispatch(claim(&server, "Ada")).await.unwrap();
    assert!(matches!(handle.reach(), Reach::Reachable { version } if version.number() == 1));
    assert!(handle.api().own_account().await.unwrap().is_owner);

    let too_new = Device::open(directory.path(), SupportedVersions::new(3, Some(2)));
    let handle = too_new.servers().remove(0);
    let incompatible = Incompatibility::ServerTooOld { server_newest: 1 };
    assert_eq!(
        handle.connect().await,
        Err(FlowError::Incompatible(incompatible))
    );
    assert_eq!(handle.reach(), Reach::Incompatible(incompatible));
    assert!(matches!(
        handle.access().state,
        AccessState::LoggedOut { .. }
    ));
    assert!(matches!(
        handle.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));

    let current: Client = Device::open(directory.path(), SupportedVersions::THIS_BUILD);
    let handle = current.servers().remove(0);
    handle.connect().await.unwrap();
    logged_in(&handle);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recovery_gives_a_new_device_the_account_and_every_device_a_notice() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = invite(&owner, 1).await;
    let second = Device::new();
    let old_device = second.add(&server.address()).await;
    old_device.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&old_device);

    let third = Device::new();
    let new_device = third.add(&server.address()).await;
    new_device.connect().await.unwrap();
    assert_eq!(
        new_device.access().state,
        AccessState::NoAccount { notice: None }
    );
    let notice = owner
        .api()
        .recover_account(
            account_id,
            &RecoverAccount {
                public_key: new_device.device_public_key().unwrap(),
                device_name: "Replacement".to_owned(),
            },
        )
        .await
        .unwrap();

    new_device.connect().await.unwrap();
    let AccessState::LoggedIn {
        account_id: recovered,
        recoveries,
        ..
    } = new_device.access().state
    else {
        panic!("the recovered device is not logged in");
    };
    assert_eq!(recovered, account_id);
    assert_eq!(recoveries.len(), 1);
    assert_eq!(recoveries[0].id, notice.id);

    assert_eq!(old_device.api().own_account().await.unwrap().name, "Grace");
    let AccessState::LoggedIn { recoveries, .. } = old_device.access().state else {
        panic!("the first device of the account is not logged in");
    };
    assert_eq!(recoveries.len(), 1);
    assert_eq!(recoveries[0].performed_by, Some(logged_in(&owner)));
    assert_eq!(old_device.api().recoveries().await.unwrap().len(), 1);
    assert_eq!(old_device.api().devices().await.unwrap().len(), 2);

    // A login does not remove the notice. The acknowledgement of one device does, for
    // that device only (ADR 0118).
    let shown = |handle: &junto_client_core::ServerHandle| match handle.access().state {
        AccessState::LoggedIn { recoveries, .. } => recoveries.len(),
        other => panic!("not logged in: {other:?}"),
    };
    for handle in [&new_device, &old_device] {
        handle.dispatch(AccessAction::LogOut).await.unwrap();
        handle.dispatch(AccessAction::LogIn).await.unwrap();
        assert_eq!(shown(handle), 1);
        assert_eq!(handle.api().recovery_notices().await.unwrap().len(), 1);
    }
    let restarted = third.restarted();
    let after_restart = restarted.servers().remove(0);
    after_restart.connect().await.unwrap();
    assert_eq!(shown(&after_restart), 1);

    new_device.acknowledge_recovery(notice.id).await.unwrap();
    assert_eq!(shown(&new_device), 0);
    assert!(
        new_device
            .api()
            .recovery_notices()
            .await
            .unwrap()
            .is_empty()
    );
    new_device.dispatch(AccessAction::LogOut).await.unwrap();
    new_device.dispatch(AccessAction::LogIn).await.unwrap();
    assert_eq!(shown(&new_device), 0);
    let again = new_device
        .acknowledge_recovery(notice.id)
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some(ErrorCode::NotFound));

    old_device.dispatch(AccessAction::LogOut).await.unwrap();
    old_device.dispatch(AccessAction::LogIn).await.unwrap();
    assert_eq!(shown(&old_device), 1);
    assert_eq!(old_device.api().recoveries().await.unwrap().len(), 1);
}

/// A still PNG image as far as the server reads it: the header chunk with the size, and
/// the start of the image data.
fn png(width: u32, height: u32, padding: usize) -> Vec<u8> {
    let mut image = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    image.extend_from_slice(&width.to_be_bytes());
    image.extend_from_slice(&height.to_be_bytes());
    image.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    image.extend_from_slice(&(padding as u32).to_be_bytes());
    image.extend_from_slice(b"IDAT");
    image.resize(image.len() + padding + 4, 0);
    image
}

#[tokio::test(flavor = "multi_thread")]
async fn a_proof_is_accepted_at_a_public_origin_of_the_server_and_refused_at_another_address() {
    let server = TestServer::start("Acme").await;
    let unlisted = FaultProxy::start_unlisted(&server).await;
    let device = Device::new();
    let handle = device.add(&unlisted.address()).await;
    assert_eq!(
        handle.dispatch(claim(&server, "Ada")).await,
        Err(FlowError::OriginRejected)
    );
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount { notice: None }
    );
    let signed_origin = |proxy: &FaultProxy, path: &str| {
        let sent = proxy.seen(path);
        assert_eq!(sent.len(), 1, "{path}");
        let body: serde_json::Value = serde_json::from_slice(&sent[0].body).unwrap();
        let (status, answer) = sent[0].answer.clone().unwrap();
        (
            body["proof"]["origin"].as_str().unwrap().to_owned(),
            status,
            String::from_utf8(answer).unwrap(),
        )
    };
    let (origin, status, answer) = signed_origin(&unlisted, "/api/v1/owner/claim");
    assert_eq!(origin, unlisted.address());
    assert_eq!(status, 403);
    assert!(answer.contains("origin_rejected"), "{answer}");

    // The owner code is unused: the same code claims the server at a listed address, and
    // the proof names that address.
    let listed = FaultProxy::start(&server).await;
    let through_proxy = Device::new();
    let owner = through_proxy.add(&listed.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    assert!(owner.api().own_account().await.unwrap().is_owner);
    let (origin, status, _) = signed_origin(&listed, "/api/v1/owner/claim");
    assert_eq!(origin, listed.address());
    assert_eq!(status, 201);

    // The address of the server itself is the other listed origin.
    let code = invite(&owner, 1).await;
    let direct = Device::new();
    let member = direct.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&member);
    member.dispatch(AccessAction::LogOut).await.unwrap();
    member.dispatch(AccessAction::LogIn).await.unwrap();
    assert_eq!(logged_in(&member), account_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_avatar_of_the_preset_reaches_a_new_account_and_other_accounts_read_it() {
    let server = TestServer::start("Acme").await;
    let image = png(64, 48, 100);
    let first = Device::new();
    first.client.profile().set_name("Preset name").unwrap();
    first.client.profile().set_avatar(&image).unwrap();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let owner_id = logged_in(&owner);
    assert_eq!(owner.profile_sync(), ProfileSync::Applied);
    let own = owner.api().own_account().await.unwrap();
    assert_eq!(own.name, "Ada");
    let stored = own.avatar.unwrap();
    assert_eq!(stored.hash, avatar_hash(&image));
    assert_eq!(stored.media_type, ImageType::Png);
    assert_eq!((stored.width, stored.height), (64, 48));
    assert_eq!(stored.bytes as usize, image.len());

    let code = invite(&owner, 2).await;
    let second = Device::new();
    let member = second.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let member_id = logged_in(&member);
    assert_eq!(member.profile_sync(), ProfileSync::NotNeeded);
    let profile = member.api().profile(owner_id).await.unwrap();
    assert_eq!(profile.name, "Ada");
    assert_eq!(profile.avatar, Some(stored.clone()));
    let read = member.api().avatar(owner_id).await.unwrap();
    assert_eq!(read.bytes, image);
    assert_eq!(read.hash, stored.hash);
    assert_eq!(read.media_type, ImageType::Png);

    let renamed = member.api().update_profile("Grace H.").await.unwrap();
    assert_eq!(renamed.name, "Grace H.");
    assert_eq!(renamed.avatar, None);
    assert_eq!(
        owner.api().profile(member_id).await.unwrap().name,
        "Grace H."
    );
    let jpeg_like = member.api().set_avatar(b"\xff\xd8\xff\xe0".to_vec()).await;
    assert_eq!(
        jpeg_like.unwrap_err().code(),
        Some(ErrorCode::ImageUnsupported)
    );
    let replaced = member.api().set_avatar(png(8, 8, 0)).await.unwrap();
    assert_eq!((replaced.width, replaced.height), (8, 8));
    member.api().remove_avatar().await.unwrap();
    assert_eq!(owner.api().profile(member_id).await.unwrap().avatar, None);
    let gone = owner.api().avatar(member_id).await.unwrap_err();
    assert_eq!(gone.code(), Some(ErrorCode::NotFound));

    // An image with more pixels than a server stores passes the check of the client, which
    // does not read the size in pixels. The server refuses it, and it is not sent again.
    let third = Device::new();
    third
        .client
        .profile()
        .set_avatar(&png(2000, 2000, 0))
        .unwrap();
    let wide = third.add(&server.address()).await;
    wide.dispatch(join(&code, "Linus")).await.unwrap();
    assert_eq!(wide.profile_sync(), ProfileSync::Refused);
    assert_eq!(wide.api().own_account().await.unwrap().avatar, None);
    wide.dispatch(AccessAction::LogOut).await.unwrap();
    wide.dispatch(AccessAction::LogIn).await.unwrap();
    assert_eq!(wide.profile_sync(), ProfileSync::Refused);

    // The preset takes no image that is larger than a server stores.
    let large = png(16, 16, AVATAR_MAX_BYTES);
    assert!(matches!(
        third.client.profile().set_avatar(&large),
        Err(ProfileError::Avatar(InvalidAvatar::TooLarge))
    ));
    assert!(matches!(
        wide.api().set_avatar(large).await,
        Err(ApiError::Avatar(InvalidAvatar::TooLarge))
    ));
    let largest = png(16, 16, AVATAR_MAX_BYTES - 45);
    assert_eq!(largest.len(), AVATAR_MAX_BYTES);
    let kept = wide.api().set_avatar(largest.clone()).await.unwrap();
    assert_eq!(kept.bytes as usize, AVATAR_MAX_BYTES);
    assert_eq!(
        wide.api().avatar(logged_in(&wide)).await.unwrap().bytes,
        largest
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_sets_and_shows_the_profile_of_an_account_and_saves_an_avatar() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let owner = Cli::in_directory(first.path());
    let member = Cli::in_directory(second.path());
    let image = png(32, 32, 10);
    let image_file = first.path().join("avatar-source.png");
    std::fs::write(&image_file, &image).unwrap();
    let image_path = image_file.to_str().unwrap();

    let preset = owner.ok(&["profile", "--name", "Ada", "--avatar", image_path]);
    assert_eq!(field(&preset, "avatar"), "image/png");
    owner.ok(&["add", &address]);
    let claimed = owner.ok_with_input(&["claim"], &format!("{}\n", server.owner_code().unwrap()));
    assert_eq!(field(&claimed, "state"), "logged in");
    let account = owner.ok(&["account"]);
    assert_eq!(field(&account, "name"), "Ada");
    assert_eq!(field(&account, "owner"), "true");
    let owner_id = field(&account, "account").to_owned();
    let avatar = field(&account, "avatar");
    assert!(avatar.starts_with("image/png  "), "{avatar}");
    assert!(avatar.contains("32x32"), "{avatar}");
    assert!(avatar.ends_with(&avatar_hash(&image)), "{avatar}");

    let invited = owner.ok(&["invite"]);
    member.ok(&["add", &address]);
    member.ok_with_input(
        &["join", "--name", "Grace"],
        &format!("{}\n", field(&invited, "code")),
    );
    let seen = member.ok(&["account", &owner_id]);
    assert_eq!(field(&seen, "name"), "Ada");
    assert!(!seen.contains("owner: "), "{seen}");
    let saved_file = second.path().join("saved.png");
    let saved_path = saved_file.to_str().unwrap();
    let saved = member.ok(&["account", &owner_id, "--save-avatar", saved_path]);
    assert!(field(&saved, "saved").contains(&avatar_hash(&image)));
    assert_eq!(std::fs::read(&saved_file).unwrap(), image);
    let (success, _, stderr) = member.run(&["account", &owner_id, "--save-avatar", saved_path]);
    assert!(!success && stderr.contains("saved.png"), "{stderr}");
    assert_eq!(std::fs::read(&saved_file).unwrap(), image);
    let (success, _, stderr) = member.run(&["account", &owner_id, "--name", "Mallory"]);
    assert!(
        !success && stderr.contains("only the account of this device"),
        "{stderr}"
    );

    let changed = member.ok(&["account", "--name", "Grace H.", "--avatar", image_path]);
    assert_eq!(field(&changed, "name"), "Grace H.");
    assert!(field(&changed, "avatar").contains("32x32"));
    let removed = member.ok(&["account", "--no-avatar"]);
    assert_eq!(field(&removed, "avatar"), "none");
    assert_eq!(field(&removed, "name"), "Grace H.");

    let not_an_image = first.path().join("notes.txt");
    std::fs::write(&not_an_image, b"not an image").unwrap();
    let (success, _, stderr) = member.run(&["account", "--avatar", not_an_image.to_str().unwrap()]);
    assert!(
        !success && stderr.contains("not a PNG, JPEG or WebP image"),
        "{stderr}"
    );
    assert_eq!(
        field(&owner.ok(&["profile", "--no-avatar"]), "avatar"),
        "none"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_code_of_the_host_command_returns_the_owner_on_a_new_device() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let account_id = logged_in(&owner);

    let issued = support::code_in(&server.host_command(&["issue-owner-code"])).unwrap();
    let second = Device::new();
    second.client.profile().set_avatar(&png(8, 8, 0)).unwrap();
    let returning = second.add(&server.address()).await;
    returning
        .dispatch(AccessAction::ClaimOwner {
            code: OwnerCode::new(issued),
            name: "Ada".to_owned(),
            device_name: "New laptop".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(logged_in(&returning), account_id);
    let account = returning.api().own_account().await.unwrap();
    assert!(account.is_owner);
    // The account existed before, so the avatar of the preset of this device is not sent.
    assert_eq!(returning.profile_sync(), ProfileSync::NotNeeded);
    assert_eq!(account.avatar, None);

    assert!(owner.api().own_account().await.unwrap().is_owner);
    let AccessState::LoggedIn { recoveries, .. } = owner.access().state else {
        panic!("the first device of the owner is not logged in");
    };
    assert_eq!(recoveries.len(), 1);
    assert_eq!(recoveries[0].performed_by, None);
}

struct Cli {
    data_dir: std::path::PathBuf,
}

impl Cli {
    /// A tool whose data directory is a new directory inside `directory`.
    fn in_directory(directory: &Path) -> Self {
        Self {
            data_dir: directory.join("junto"),
        }
    }

    fn run(&self, arguments: &[&str]) -> (bool, String, String) {
        self.run_with_input(arguments, "")
    }

    /// Runs the tool with `input` on its standard input, which is not a terminal.
    fn run_with_input(&self, arguments: &[&str], input: &str) -> (bool, String, String) {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_junto-cli"))
            .arg("--data-dir")
            .arg(&self.data_dir)
            .args(arguments)
            .env_remove("JUNTO_DATA_DIR")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(input.as_bytes());
        drop(stdin);
        let output = child.wait_with_output().unwrap();
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }

    #[track_caller]
    fn ok_with_input(&self, arguments: &[&str], input: &str) -> String {
        let (success, stdout, stderr) = self.run_with_input(arguments, input);
        assert!(success, "{arguments:?}\n{stdout}\n{stderr}");
        stdout
    }

    #[track_caller]
    fn ok(&self, arguments: &[&str]) -> String {
        let (success, stdout, stderr) = self.run(arguments);
        assert!(success, "{arguments:?}\n{stdout}\n{stderr}");
        stdout
    }
}

fn field<'a>(output: &'a str, name: &str) -> &'a str {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("no {name} in:\n{output}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_command_line_tool_operates_a_server_from_claim_to_approval() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let owner_code = server.owner_code().unwrap();
    let (first, second, third) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let owner = Cli::in_directory(first.path());
    let member = Cli::in_directory(second.path());
    let asking = Cli::in_directory(third.path());

    let (success, _, stderr) = owner.run(&[]);
    assert!(!success && stderr.contains("usage: junto-cli"));
    let (success, _, stderr) = owner.run(&["status"]);
    assert!(
        !success && stderr.contains("no server is saved"),
        "{stderr}"
    );

    let added = owner.ok(&["add", &address]);
    assert_eq!(field(&added, "name"), "Acme");
    assert!(owner.ok(&["servers"]).contains("no account"));
    let claimed = owner.ok_with_input(&["claim", "--name", "Ada"], &format!("{owner_code}\n"));
    assert_eq!(field(&claimed, "state"), "logged in");
    assert!(!claimed.contains(&owner_code));
    assert!(owner.ok(&["servers"]).contains("member"));

    let login = owner.ok(&["login"]);
    assert_eq!(field(&login, "name"), "Ada");
    assert_eq!(field(&login, "owner"), "true");
    let devices = owner.ok(&["devices"]);
    assert_eq!(devices.lines().count(), 1);
    assert!(devices.contains("(this device)"));

    let invited = owner.ok(&["invite", "--max-uses", "2", "--expires-in-hours", "1"]);
    assert_eq!(field(&invited, "uses"), "2");
    let invite_code = field(&invited, "code").to_owned();

    member.ok(&["add", &address]);
    let (success, _, stderr) =
        member.run_with_input(&["join", "--name", "Grace"], "AAAA-AAAA-AAAA-AAAA\n");
    assert!(!success && stderr.contains("invite_not_found"), "{stderr}");
    member.ok(&["profile", "--name", "Grace"]);
    let joined = member.ok_with_input(&["join"], &format!("{invite_code}\n"));
    assert_eq!(field(&joined, "state"), "logged in");
    assert_eq!(field(&member.ok(&["login"]), "name"), "Grace");
    let (success, _, stderr) = member.run(&["invite"]);
    assert!(!success && stderr.contains("forbidden"), "{stderr}");

    asking.ok(&["add", &address, "--when-selected"]);
    let requested = asking.ok(&["request", "--name", "Linus"]);
    assert_eq!(field(&requested, "state"), "waiting for approval");
    let request_id = field(&requested, "request").to_owned();
    assert_eq!(
        field(&asking.ok(&["status"]), "state"),
        "waiting for approval"
    );
    let requests = owner.ok(&["requests"]);
    assert!(requests.contains(&request_id) && requests.contains("Linus"));
    owner.ok(&["approve-request", &request_id]);
    assert_eq!(field(&asking.ok(&["status"]), "state"), "logged in");
    assert_eq!(
        field(&asking.ok(&["login", "--server", "Acme"]), "name"),
        "Linus"
    );

    asking.ok(&["remove"]);
    assert_eq!(asking.ok(&["servers"]), "");
    assert_eq!(
        std::fs::read_dir(third.path().join("junto").join("keys"))
            .unwrap()
            .count(),
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_takes_a_code_from_standard_input_and_never_from_an_argument() {
    let server = TestServer::start("Acme").await;
    let owner_code = server.owner_code().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let owner = Cli::in_directory(directory.path());
    owner.ok(&["add", &server.address()]);

    let (success, stdout, stderr) = owner.run(&["claim", "--name", "Ada", "--code", &owner_code]);
    assert!(!success, "{stdout}");
    assert!(stderr.contains("standard input"), "{stderr}");
    assert!(owner.ok(&["servers"]).contains("no account"));

    let claimed = owner.ok_with_input(&["claim", "--name", "Ada"], &format!("{owner_code}\n"));
    assert_eq!(field(&claimed, "state"), "logged in");
    assert!(!claimed.contains(&owner_code));

    let (success, _, stderr) = owner.run(&["claim", "--name", "Ada"]);
    assert!(!success && stderr.contains("no owner code"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_reads_a_bounded_line_and_a_line_without_a_line_end() {
    let server = TestServer::start("Acme").await;
    let owner_code = server.owner_code().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let owner = Cli::in_directory(directory.path());
    owner.ok(&["add", &server.address()]);

    let endless = "A".repeat(4 * 1024 * 1024);
    let started = Instant::now();
    let (success, stdout, stderr) = owner.run_with_input(&["claim", "--name", "Ada"], &endless);
    assert!(!success, "{stdout}");
    assert!(stderr.contains("longer than 256 bytes"), "{stderr}");
    assert!(!stderr.contains("AAAA"), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(20));
    let (success, _, stderr) = owner.run_with_input(&["link-all"], &endless);
    assert!(!success && stderr.contains("longer than"), "{stderr}");
    assert!(owner.ok(&["servers"]).contains("no account"));

    let claimed = owner.ok_with_input(&["claim", "--name", "Ada"], &format!("  {owner_code}"));
    assert_eq!(field(&claimed, "state"), "logged in");
}

/// A terminal for a child process: the test holds one end and the child reads the other.
#[cfg(unix)]
mod terminal {
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;

    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    use rustix::termios::{LocalModes, tcgetattr};

    pub struct Terminal {
        test_end: File,
        child_end: OwnedFd,
    }

    impl Terminal {
        pub fn open() -> Self {
            let test_end = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
            grantpt(&test_end).unwrap();
            unlockpt(&test_end).unwrap();
            let name = ptsname(&test_end, Vec::new()).unwrap();
            let flags = OFlags::RDWR | OFlags::NOCTTY;
            let child_end = rustix::fs::open(name.as_c_str(), flags, Mode::empty()).unwrap();
            Self {
                test_end: File::from(test_end),
                child_end,
            }
        }

        pub fn for_child(&self) -> std::process::Stdio {
            self.child_end.try_clone().unwrap().into()
        }

        pub fn echoes(&self) -> bool {
            tcgetattr(&self.child_end)
                .unwrap()
                .local_modes
                .contains(LocalModes::ECHO)
        }

        pub fn type_line(&mut self, line: &str) {
            self.test_end.write_all(line.as_bytes()).unwrap();
            self.test_end.write_all(b"\n").unwrap();
        }

        /// What the terminal showed and nobody has read yet.
        pub fn shown(&mut self) -> String {
            let flags = rustix::fs::fcntl_getfl(&self.test_end).unwrap();
            rustix::fs::fcntl_setfl(&self.test_end, flags | OFlags::NONBLOCK).unwrap();
            let mut shown = Vec::new();
            let mut buffer = [0u8; 1024];
            while let Ok(count @ 1..) = self.test_end.read(&mut buffer) {
                shown.extend_from_slice(&buffer[..count]);
            }
            rustix::fs::fcntl_setfl(&self.test_end, flags).unwrap();
            String::from_utf8_lossy(&shown).into_owned()
        }
    }

    /// A pipe that is full, so that the next write to it waits until somebody reads.
    pub fn full_pipe() -> (std::io::PipeReader, std::io::PipeWriter) {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let flags = rustix::fs::fcntl_getfl(&writer).unwrap();
        rustix::fs::fcntl_setfl(&writer, flags | OFlags::NONBLOCK).unwrap();
        while writer.write(&[b'.'; 1024]).is_ok() {}
        while writer.write(b".").is_ok() {}
        rustix::fs::fcntl_setfl(&writer, flags).unwrap();
        (reader, writer)
    }
}

#[cfg(unix)]
#[test]
fn the_prompt_for_a_secret_is_shown_only_after_the_echo_of_the_terminal_is_off() {
    use std::io::Read;
    let directory = tempfile::tempdir().unwrap();
    let mut terminal = terminal::Terminal::open();
    assert!(terminal.echoes());

    let (mut messages, full) = terminal::full_pipe();
    let mut child = Command::new(env!("CARGO_BIN_EXE_junto-cli"))
        .arg("--data-dir")
        .arg(directory.path().join("junto"))
        .arg("link-all")
        .stdin(terminal.for_child())
        .stdout(std::process::Stdio::null())
        .stderr(full)
        .spawn()
        .unwrap();

    // The tool cannot show its prompt yet: the pipe for its messages is full. What it
    // did before the prompt is all that has happened.
    let waited = Instant::now();
    while terminal.echoes() && waited.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !terminal.echoes(),
        "the tool waits to show its prompt while the terminal still echoes"
    );

    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        messages.read_to_end(&mut text).unwrap();
        String::from_utf8_lossy(&text).into_owned()
    });
    terminal.type_line("SECRET-OFFER-TEXT");
    let status = child.wait().unwrap();
    let messages = reader.join().unwrap();
    assert!(!status.success());
    assert!(messages.contains("link offer: "), "{messages}");
    assert!(messages.contains("not a valid link offer"), "{messages}");
    assert!(!messages.contains("SECRET-OFFER-TEXT"));
    let shown = terminal.shown();
    assert!(!shown.contains("SECRET-OFFER-TEXT"), "{shown}");
    assert!(terminal.echoes(), "the echo was not restored");
}

#[tokio::test(flavor = "multi_thread")]
async fn every_command_of_the_tool_shows_a_recovery_before_its_own_output() {
    let server = TestServer::start("Acme").await;
    let address = server.address();
    let directory = tempfile::tempdir().unwrap();
    let owner = Cli::in_directory(directory.path());
    owner.ok(&["add", &address]);
    owner.ok_with_input(
        &["claim", "--name", "Ada"],
        &format!("{}\n", server.owner_code().unwrap()),
    );
    let offer = field(&owner.ok(&["link-offer"]), "offer").to_owned();

    let new_device = Device::new();
    let linking = new_device.client.link_this_device();
    linking
        .dispatch(LinkDeviceAction::Start {
            offer: offer.clone(),
            device_name: "Phone".to_owned(),
        })
        .await
        .unwrap();
    let LinkDeviceState::Requested {
        request: Some(request),
        ..
    } = linking.view().state
    else {
        panic!("no request");
    };
    let request = format!("{}\n", request.expose());
    let offer = format!("{offer}\n");
    let unknown_request = uuid::Uuid::now_v7().to_string();

    let commands: [(&[&str], &str); 9] = [
        (&["devices"], ""),
        (&["invite"], ""),
        (&["requests"], ""),
        (&["login"], ""),
        (&["status"], ""),
        (&["approve-request", &unknown_request], ""),
        (&["approve-link", "--yes"], "ABCD-EFGH\n"),
        (&["link-all"], &offer),
        (&["approve-all", "--yes"], &request),
    ];
    for (arguments, input) in commands {
        let issued = support::code_in(&server.host_command(&["issue-owner-code"])).unwrap();
        let returning = Device::new();
        let handle = returning.add(&address).await;
        handle
            .dispatch(AccessAction::ClaimOwner {
                code: OwnerCode::new(issued),
                name: "Ada".to_owned(),
                device_name: String::new(),
            })
            .await
            .unwrap();

        let (_, stdout, stderr) = owner.run_with_input(arguments, input);
        let lines: Vec<&str> = stdout.lines().collect();
        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with("recovery ")),
            "{arguments:?}\n{stdout}\n{stderr}"
        );
        assert!(lines[0].contains("a command on the host of the server"));
        let notices = lines.iter().filter(|line| line.starts_with("recovery "));
        assert_eq!(notices.count(), 1, "{arguments:?}\n{stdout}");
        let recovery_id = lines[0]
            .strip_prefix("recovery ")
            .and_then(|rest| rest.split(':').next())
            .unwrap()
            .to_owned();

        // The tool shows the recovery at every command until it is acknowledged.
        let again = owner.ok(&["devices"]);
        assert!(
            again.starts_with(&format!("recovery {recovery_id}: ")),
            "{arguments:?}\n{again}"
        );
        let listed = owner.ok(&["recoveries"]);
        assert_eq!(listed.lines().count(), 1, "{listed}");
        assert!(listed.contains(&recovery_id), "{listed}");
        let acknowledged = owner.ok(&["acknowledge-recovery", &recovery_id]);
        assert_eq!(
            acknowledged.lines().last(),
            Some(format!("acknowledged recovery {recovery_id}").as_str())
        );
        let after = owner.ok(&["devices"]);
        assert!(!after.contains("recovery "), "{arguments:?}\n{after}");
        assert_eq!(
            owner.ok(&["recoveries"]),
            "no recovery waits for an acknowledgement\n"
        );
        let (success, _, stderr) = owner.run(&["acknowledge-recovery", &recovery_id]);
        assert!(!success && stderr.contains("not_found"), "{stderr}");
    }
    let devices = owner.ok(&["devices"]);
    assert!(devices.contains("Phone"), "{devices}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_device_is_linked_on_two_servers_in_one_flow() {
    let acme = TestServer::start("Acme").await;
    let globex = TestServer::start("Globex").await;
    let first = Device::new();
    let on_acme = first.add(&acme.address()).await;
    let on_globex = first.add(&globex.address()).await;
    on_acme.dispatch(claim(&acme, "Ada")).await.unwrap();
    on_globex.dispatch(claim(&globex, "Ada")).await.unwrap();

    let second = Device::new();
    let linking = second.client.link_this_device();
    linking
        .dispatch(LinkDeviceAction::Start {
            offer: first.client.link_offer().to_text(),
            device_name: "Phone".to_owned(),
        })
        .await
        .unwrap();
    let LinkDeviceState::Requested {
        request: Some(request),
        fingerprint: Some(fingerprint),
        ..
    } = linking.view().state
    else {
        panic!("no request");
    };

    let approving = first.client.approve_devices();
    approving
        .dispatch(ApproveDevicesAction::LookUp { request })
        .await
        .unwrap();
    let ApproveDevicesState::Confirm {
        fingerprint: shown,
        device_name,
        servers,
    } = approving.view().state
    else {
        panic!("nothing to confirm");
    };
    assert_eq!((shown, device_name.as_str()), (fingerprint, "Phone"));
    assert_eq!(servers.len(), 2);
    assert!(
        servers
            .iter()
            .all(|server| server.progress == ApprovalProgress::Ready)
    );
    assert_eq!(on_acme.api().devices().await.unwrap().len(), 1);
    approving
        .dispatch(ApproveDevicesAction::Approve)
        .await
        .unwrap();

    linking.dispatch(LinkDeviceAction::Check).await.unwrap();
    let LinkDeviceState::Requested {
        servers, request, ..
    } = linking.view().state
    else {
        panic!("no links");
    };
    assert!(request.is_none());
    assert!(
        servers
            .iter()
            .all(|server| server.progress == LinkProgress::Linked)
    );
    // The new device saved the servers side by side, so their order in its book is the
    // order in which they answered.
    assert_eq!(second.client.servers().len(), 2);
    for old in first.client.servers() {
        let new = second.client.server(old.id()).unwrap();
        assert_eq!(logged_in(&new), logged_in(&old));
        assert!(new.api().own_account().await.unwrap().is_owner);
        let devices = new.api().devices().await.unwrap();
        assert_eq!(devices.len(), 2);
        assert!(
            devices
                .iter()
                .any(|device| device.current && device.name == "Phone")
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_links_a_new_device_on_every_server_of_the_first_device() {
    let acme = TestServer::start("Acme").await;
    let globex = TestServer::start("Globex").await;
    let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let owner = Cli::in_directory(first.path());
    let phone = Cli::in_directory(second.path());
    for server in [&acme, &globex] {
        let address = server.address();
        owner.ok(&["add", &address]);
        owner.ok_with_input(
            &["claim", "--server", &address, "--name", "Ada"],
            &format!("{}\n", server.owner_code().unwrap()),
        );
    }

    let offered = owner.ok(&["link-offer"]);
    let offer = field(&offered, "offer");
    let requested = phone.ok_with_input(
        &["link-all", "--device-name", "Phone"],
        &format!("{offer}\n"),
    );
    assert_eq!(requested.matches("waiting for approval").count(), 2);
    let (request, fingerprint) = (
        field(&requested, "request"),
        field(&requested, "fingerprint"),
    );

    let (success, stdout, stderr) =
        owner.run_with_input(&["approve-all"], &format!("{request}\nn\n"));
    assert!(
        !success && stderr.contains("not approved"),
        "{stdout}{stderr}"
    );
    assert_eq!(stdout.matches("  ready").count(), 2);

    let approved = owner.ok_with_input(&["approve-all", "--yes"], &format!("{request}\n"));
    assert_eq!(field(&approved, "fingerprint"), fingerprint);
    assert_eq!(field(&approved, "device name"), "Phone");
    assert_eq!(approved.matches("  approved").count(), 2);

    for server in [&acme, &globex] {
        let address = server.address();
        let login = phone.ok(&["login", "--server", &address]);
        assert_eq!(field(&login, "name"), "Ada");
        assert_eq!(field(&login, "owner"), "true");
        let devices = owner.ok(&["devices", "--server", &address]);
        assert_eq!(devices.lines().count(), 2);
    }
    let (success, _, stderr) = owner.run(&["approve-all", "--code", request]);
    assert!(!success && stderr.contains("standard input"), "{stderr}");
}

/// ADR 0715: an operation that uses the network starts its work on the runtime of the
/// client and returns a future that only waits for it. The thread of this test has no
/// tokio runtime, as the thread of a window, and polls those futures with an executor of
/// its own. A future that did network work itself would need the tokio reactor here.
#[test]
fn an_executor_without_tokio_polls_the_futures_of_network_operations() {
    let server_runtime = tokio::runtime::Runtime::new().unwrap();
    let server = server_runtime.block_on(TestServer::start("Acme Studio"));
    assert!(tokio::runtime::Handle::try_current().is_err());
    let device = Device::new();

    futures_executor::block_on(async {
        let handle = add(&device.client, &server.address()).await;
        handle.dispatch(claim(&server, "Ada")).await.unwrap();
        let account_id = logged_in(&handle);
        let api = handle.api();
        assert_eq!(api.own_account().await.unwrap().id, account_id);
        let created = CreateChannel {
            name: "plans".to_owned(),
            kind: ChannelKind::Text,
            category_id: None,
            position: None,
        };
        let workspaces = api.workspaces().await.unwrap();
        let channel = api
            .create_channel(workspaces[0].id, &created)
            .await
            .unwrap();
        assert_eq!(channel.name, "plans");
        handle.logo().await.unwrap();
    });
    drop(server);
}
