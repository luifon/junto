//! Leaving a workspace, leaving the server and deleting the account through the client
//! library, against the real server (ADR 0121).

use junto_client_core::ServerHandle;
use junto_client_core::api::{AccessLoss, ApiError};
use junto_client_core::book::Membership;
use junto_client_core::flow::access::{AccessAction, AccessState, ActionKind};
use junto_client_core::protocol::account::AccountStatus;

use crate::stream::{PATIENCE, live};
use crate::support::{Device, TestServer, claim, join, logged_in};

/// Waits until the access state of a server fulfils a condition.
async fn access_when(
    device: &Device,
    handle: &ServerHandle,
    what: &str,
    holds: impl Fn(&AccessState) -> bool,
) {
    let mut changes = device.client.changes();
    let reached = tokio::time::timeout(PATIENCE, async {
        while !holds(&handle.access().state) {
            changes.changed().await;
        }
    })
    .await;
    assert!(
        reached.is_ok(),
        "after {PATIENCE:?} this did not hold: {what}; the state is {:?}",
        handle.access().state
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_member_leaves_the_server_returns_to_the_same_account_and_then_deletes_it() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = crate::invite(&owner, 3).await;
    let second = Device::new();
    let member = second.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&member);
    let key = member.device_public_key().unwrap();
    let workspaces = member.api().own_account().await.unwrap().workspace_ids;
    assert_eq!(workspaces.len(), 1);

    // The owner cannot leave its only workspace or delete its account.
    let owner_workspaces = owner.api().own_account().await.unwrap().workspace_ids;
    assert!(
        owner
            .api()
            .leave_workspace(owner_workspaces[0])
            .await
            .is_err()
    );
    assert!(owner.api().delete_account().await.is_err());

    // The last workspace: the account is inactive, the book keeps the key.
    member.api().leave_workspace(workspaces[0]).await.unwrap();
    member.dispatch(AccessAction::Check).await.unwrap();
    let view = member.access();
    assert_eq!(
        view.state,
        AccessState::Inactive {
            account_id: Some(account_id)
        }
    );
    assert_eq!(
        view.actions,
        [
            ActionKind::Check,
            ActionKind::Join,
            ActionKind::RequestAccess,
            ActionKind::ForgetAccount
        ]
    );
    assert_eq!(
        member.entry().membership,
        Membership::Inactive { account_id }
    );
    assert!(matches!(
        member.api().workspaces().await,
        Err(ApiError::NotLoggedIn | ApiError::AccessLost(AccessLoss::Inactive))
    ));

    // A restart keeps the state.
    let restarted = second.restarted();
    let member = restarted.servers().remove(0);
    assert!(matches!(
        member.access().state,
        AccessState::Inactive { .. }
    ));
    member.connect().await.unwrap();
    assert!(matches!(
        member.access().state,
        AccessState::Inactive { .. }
    ));

    // A join with a code returns the same account, with its name and the same key.
    member.dispatch(join(&code, "Another name")).await.unwrap();
    assert_eq!(logged_in(&member), account_id);
    assert_eq!(member.device_public_key().unwrap(), key);
    assert_eq!(member.api().own_account().await.unwrap().name, "Grace");
    let accounts = owner.api().accounts().await.unwrap();
    let listed = accounts.iter().find(|account| account.id == account_id);
    assert_eq!(listed.unwrap().status, AccountStatus::Active);

    // Deletion: the server leaves the book of this device with its key.
    member.api().delete_account().await.unwrap();
    restarted.remove_server(member.id()).unwrap();
    assert!(restarted.servers().is_empty());
    let accounts = owner.api().accounts().await.unwrap();
    let listed = accounts.iter().find(|account| account.id == account_id);
    assert_eq!(listed.unwrap().status, AccountStatus::Deleted);
    assert_eq!(listed.unwrap().name, "Grace");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_device_of_a_deleted_account_states_the_deletion_and_not_a_removal() {
    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = crate::invite(&owner, 1).await;
    let laptop = Device::new();
    let on_laptop = laptop.add(&server.address()).await;
    on_laptop.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&on_laptop);

    // A second device of the account, linked by the first.
    let phone = Device::new();
    let on_phone = phone.add(&server.address()).await;
    on_phone
        .dispatch(AccessAction::RequestLink {
            device_name: "Phone".to_owned(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForLink {
        code: link_code,
        public_key,
        ..
    } = on_phone.access().state
    else {
        panic!("the second device does not wait for a link");
    };
    on_laptop
        .api()
        .approve_device_link(link_code, public_key)
        .await
        .unwrap();
    on_phone.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(logged_in(&on_phone), account_id);
    live(&phone, &on_phone).await;

    on_laptop.api().delete_account().await.unwrap();
    access_when(
        &phone,
        &on_phone,
        "the phone states the deletion",
        |state| matches!(state, AccessState::Deleted { .. }),
    )
    .await;
    // The state can be visible while renewal still owns the flow. Queue the later
    // check behind it; the now unknown key must keep the deletion.
    on_phone
        .dispatch_when_free(AccessAction::Check)
        .await
        .unwrap();
    assert_eq!(
        on_phone.access().state,
        AccessState::Deleted {
            account_id: Some(account_id)
        }
    );
    assert_eq!(
        on_phone.entry().membership,
        Membership::Deleted {
            account_id: Some(account_id)
        }
    );
    assert_eq!(
        on_phone.access().actions,
        [ActionKind::Check, ActionKind::ForgetAccount]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_return_request_keeps_the_inactive_account() {
    ended_return_request_keeps_the_account(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_return_request_keeps_the_inactive_account() {
    ended_return_request_keeps_the_account(true).await;
}

async fn ended_return_request_keeps_the_account(expired: bool) {
    use junto_client_core::protocol::invite::ApproveJoinRequest;
    use sqlx::ConnectOptions;
    use sqlx::postgres::PgConnectOptions;

    let server = TestServer::start("Acme").await;
    let first = Device::new();
    let owner = first.add(&server.address()).await;
    owner.dispatch(claim(&server, "Ada")).await.unwrap();
    let code = crate::invite(&owner, 2).await;
    let second = Device::new();
    let member = second.add(&server.address()).await;
    member.dispatch(join(&code, "Grace")).await.unwrap();
    let account_id = logged_in(&member);
    let key = member.device_public_key().unwrap();
    let workspace = member.api().own_account().await.unwrap().workspace_ids[0];
    member.api().leave_workspace(workspace).await.unwrap();
    member
        .dispatch_when_free(AccessAction::Check)
        .await
        .unwrap();
    member
        .dispatch_when_free(AccessAction::RequestAccess {
            name: "Another name".to_owned(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForApproval {
        join_request_id: Some(id),
        ..
    } = member.access().state
    else {
        panic!("the return request is not pending");
    };
    if expired {
        let options: PgConnectOptions = server.database_url.parse().unwrap();
        let mut database = options.connect().await.unwrap();
        sqlx::query(
            "update join_requests set expires_at = now() - interval '1 hour' where id = $1",
        )
        .bind(id.as_uuid())
        .execute(&mut database)
        .await
        .unwrap();
    } else {
        owner.api().deny_join_request(id).await.unwrap();
    }
    member
        .dispatch_when_free(AccessAction::Check)
        .await
        .unwrap();
    assert_eq!(
        member.entry().membership,
        Membership::Inactive { account_id }
    );
    assert_eq!(
        member.access().state,
        AccessState::Inactive {
            account_id: Some(account_id)
        }
    );
    assert_eq!(member.device_public_key().unwrap(), key);

    let restarted = second.restarted();
    let member = restarted.servers().remove(0);
    member.connect_when_free().await.unwrap();
    assert_eq!(
        member.entry().membership,
        Membership::Inactive { account_id }
    );
    member
        .dispatch_when_free(AccessAction::RequestAccess {
            name: "Another name".to_owned(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForApproval {
        join_request_id: Some(next),
        ..
    } = member.access().state
    else {
        panic!("the next return request is not pending");
    };
    assert_ne!(id, next);
    let restarted = second.restarted();
    let member = restarted.servers().remove(0);
    member.connect_when_free().await.unwrap();
    assert!(
        matches!(member.access().state, AccessState::WaitingForApproval { join_request_id: Some(found), .. } if found == next)
    );
    assert_eq!(
        member.entry().membership,
        Membership::Inactive { account_id }
    );
    owner
        .api()
        .approve_join_request(next, &ApproveJoinRequest::default())
        .await
        .unwrap();
    member
        .dispatch_when_free(AccessAction::Check)
        .await
        .unwrap();
    assert_eq!(logged_in(&member), account_id);
    assert_eq!(member.device_public_key().unwrap(), key);
    assert_eq!(member.api().own_account().await.unwrap().name, "Grace");
}
