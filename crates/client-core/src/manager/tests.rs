use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use junto_protocol::error::ErrorCode;
use junto_protocol::ids::ServerId;
use junto_protocol::secret::{InviteCode, LinkCode, OwnerCode};

use super::*;
use crate::address::ServerAddress;
use crate::api::testing::{FakeServer, Fault};
use crate::api::{ChallengeMismatch, RejectionCode, TransportErrorKind, TransportFuture};
use crate::api::{HttpRequest, TransportError};
use crate::flow::add_server::{AddServerAction, AddServerActionKind, AddServerState};
use crate::flow::approve_link::{
    ApproveLinkAction, ApproveLinkActionKind, ApproveLinkState, PendingApproval,
};
use crate::flow::link_all::{
    ApprovalProgress, ApproveDevicesAction, ApproveDevicesActionKind, ApproveDevicesFlow,
    ApproveDevicesState, LinkDeviceAction, LinkDeviceActionKind, LinkDeviceFlow, LinkDeviceState,
    LinkProgress, LinkRequest, LinkRequestText,
};
use crate::profile_sync::UploadFuture;

/// Several servers behind one transport, found by host name.
#[derive(Default)]
struct Network {
    hosts: Mutex<BTreeMap<String, Arc<FakeServer>>>,
    suspended: Mutex<BTreeMap<String, watch::Sender<bool>>>,
}

impl Network {
    fn host(&self, name: &str) -> Arc<FakeServer> {
        let server = FakeServer::new(name);
        self.plug(name, server.clone());
        server
    }

    fn plug(&self, name: &str, server: Arc<FakeServer>) {
        self.hosts
            .lock()
            .unwrap()
            .insert(format!("{name}.example.test"), server);
    }
}

impl Network {
    /// Makes every request to a host wait until the host is resumed.
    fn suspend(&self, name: &str) {
        self.suspended
            .lock()
            .unwrap()
            .insert(format!("{name}.example.test"), watch::Sender::new(true));
    }

    fn resume(&self, name: &str) {
        let host = format!("{name}.example.test");
        if let Some(gate) = self.suspended.lock().unwrap().remove(&host) {
            gate.send_replace(false);
        }
    }
}

impl Transport for Network {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        let host = request.url.host_str().unwrap_or_default().to_owned();
        let server = self.hosts.lock().unwrap().get(&host).cloned();
        let gate = self
            .suspended
            .lock()
            .unwrap()
            .get(&host)
            .map(watch::Sender::subscribe);
        match (server, gate) {
            (Some(server), Some(mut gate)) => Box::pin(async move {
                let _ = gate.wait_for(|suspended| !suspended).await;
                server.send(request).await
            }),
            (Some(server), None) => server.send(request),
            (None, _) => {
                Box::pin(async { Err(TransportError::new(TransportErrorKind::Connect, "no host")) })
            }
        }
    }
}

struct Device {
    directory: tempfile::TempDir,
    client: Client,
}

fn open(
    network: &Arc<Network>,
    directory: &std::path::Path,
    versions: SupportedVersions,
) -> Client {
    let mut options = ClientOptions::new(directory.join("junto"));
    options.transport = Some(network.clone());
    options.versions = versions;
    Client::open(options).unwrap()
}

fn device(network: &Arc<Network>) -> Device {
    let directory = tempfile::tempdir().unwrap();
    let client = open(network, directory.path(), SupportedVersions::THIS_BUILD);
    Device { directory, client }
}

impl Device {
    fn reopened(&self, network: &Arc<Network>) -> Client {
        open(
            network,
            self.directory.path(),
            SupportedVersions::THIS_BUILD,
        )
    }

    async fn add(&self, name: &str) -> ServerHandle {
        add(&self.client, name).await
    }
}

async fn add(client: &Client, name: &str) -> ServerHandle {
    let flow = client.add_server();
    flow.dispatch(AddServerAction::Fetch {
        address: format!("{name}.example.test"),
    })
    .await
    .unwrap();
    flow.dispatch(AddServerAction::Save {
        connection_mode: ConnectionMode::Always,
    })
    .await
    .unwrap();
    let AddServerState::Saved { server_id } = flow.view().state else {
        panic!("not saved");
    };
    client.server(server_id).unwrap()
}

fn claim() -> AccessAction {
    AccessAction::ClaimOwner {
        code: OwnerCode::new("OWNER-CODE"),
        name: "Ada".to_owned(),
        device_name: "Laptop".to_owned(),
    }
}

fn join(code: &str) -> AccessAction {
    AccessAction::Join {
        code: InviteCode::new(code),
        name: "Grace".to_owned(),
        device_name: "Desktop".to_owned(),
    }
}

fn refused(code: ErrorCode) -> FlowError {
    FlowError::Refused {
        code: RejectionCode::Known(code),
        category: crate::api::category(code),
    }
}

fn logged_in(handle: &ServerHandle) -> AccountId {
    match handle.access().state {
        AccessState::LoggedIn { account_id, .. } => account_id,
        other => panic!("not logged in: {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_server_is_previewed_before_any_account_or_key_exists() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let flow = device.client.add_server();
    assert_eq!(flow.view().state, AddServerState::EnterAddress);

    flow.dispatch(AddServerAction::Fetch {
        address: " acme.example.test ".to_owned(),
    })
    .await
    .unwrap();
    let AddServerState::Preview(preview) = flow.view().state else {
        panic!("no preview");
    };
    assert_eq!(preview.name, "acme");
    assert_eq!(preview.server_id, server.server_id());
    assert_eq!(preview.address.as_str(), "https://acme.example.test");
    assert_eq!(
        preview.logo_url.unwrap().as_str(),
        "https://acme.example.test/api/v1/logo"
    );
    assert_eq!(preview.version.unwrap().number(), 1);
    assert_eq!(server.seen(), ["Get /api/server"]);
    assert!(device.client.servers().is_empty());
    assert_eq!(
        std::fs::read_dir(device.client.data_dir().keys_dir())
            .unwrap()
            .count(),
        0
    );

    flow.dispatch(AddServerAction::Save {
        connection_mode: ConnectionMode::WhenSelected,
    })
    .await
    .unwrap();
    assert!(flow.view().actions.is_empty());
    let handle = &device.client.servers()[0];
    assert_eq!(handle.entry().connection_mode, ConnectionMode::WhenSelected);
    assert_eq!(handle.entry().name, "acme");
    assert_eq!(handle.reach(), Reach::Unknown);
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount { notice: None }
    );
    assert_eq!(device.reopened(&network).servers().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn adding_fails_without_a_change_for_a_bad_address_or_an_absent_server() {
    let network = Arc::new(Network::default());
    let device = device(&network);
    let flow = device.client.add_server();
    for address in ["", "http://acme.example.test", "https://192.0.2.1"] {
        let error = flow
            .dispatch(AddServerAction::Fetch {
                address: address.to_owned(),
            })
            .await
            .unwrap_err();
        assert!(matches!(error, FlowError::InvalidInput(_)), "{address}");
    }
    let error = flow
        .dispatch(AddServerAction::Fetch {
            address: "absent.example.test".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(matches!(error, FlowError::Unreachable(_)), "{error}");
    let view = flow.view();
    assert_eq!(view.state, AddServerState::EnterAddress);
    assert_eq!(view.error, Some(error));
    assert_eq!(
        flow.dispatch(AddServerAction::Save {
            connection_mode: ConnectionMode::Always
        })
        .await,
        Err(FlowError::NotAvailable)
    );
}

#[tokio::test(start_paused = true)]
async fn a_second_address_that_claims_a_saved_server_is_refused_and_gets_no_proof() {
    let network = Arc::new(Network::default());
    let real = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();

    let impostor = network.host("impostor");
    impostor.set_info(|info| info.server_id = real.server_id());
    let flow = device.client.add_server();
    let error = flow
        .dispatch(AddServerAction::Fetch {
            address: "impostor.example.test".to_owned(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        error,
        FlowError::AlreadySaved {
            server_id: real.server_id()
        }
    );
    assert_eq!(impostor.seen(), ["Get /api/server"]);
    assert_eq!(device.client.servers().len(), 1);

    let again = device.client.add_server();
    let error = again
        .dispatch(AddServerAction::Fetch {
            address: "https://acme.example.test/".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(matches!(error, FlowError::AlreadySaved { .. }));
}

#[tokio::test(start_paused = true)]
async fn text_of_a_hostile_server_is_cut_and_its_logo_cannot_point_elsewhere() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.set_info(|info| {
        info.name = format!("Ac\u{1b}[31mme{}", "x".repeat(4000));
        info.logo_path = Some("https://other.example.test/logo.png".to_owned());
        info.api_versions = (1..500).collect();
    });
    let device = device(&network);
    let handle = device.add("acme").await;
    let entry = handle.entry();
    assert_eq!(entry.name.chars().count(), 64);
    assert!(entry.name.starts_with("Ac[31mme"));
    assert_eq!(entry.logo_path, None);
    assert_eq!(entry.api_versions.len(), 16);
}

#[tokio::test(start_paused = true)]
async fn the_owner_claims_and_logs_in_after_a_restart_from_the_stored_key() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;

    let wrong = AccessAction::ClaimOwner {
        code: OwnerCode::new("WRONG"),
        name: "Ada".to_owned(),
        device_name: String::new(),
    };
    assert_eq!(
        handle.dispatch(wrong).await,
        Err(refused(ErrorCode::OwnerCodeInvalid))
    );
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount { notice: None }
    );

    handle.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&handle);
    let key = handle.device_public_key().unwrap();
    assert_eq!(server.account_of(key), Some(account_id));
    assert!(matches!(handle.reach(), Reach::Reachable { .. }));
    assert_eq!(handle.api().own_account().await.unwrap().name, "Ada");

    let restarted = device.reopened(&network);
    let handle = restarted.server(server.server_id()).unwrap();
    assert_eq!(handle.access().state, AccessState::LoggedOut { account_id });
    assert_eq!(handle.access().actions, [ActionKind::LogIn]);
    assert!(matches!(
        handle.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));
    handle.connect().await.unwrap();
    assert_eq!(logged_in(&handle), account_id);
    assert_eq!(handle.device_public_key().unwrap(), key);
    assert_eq!(handle.api().devices().await.unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn joining_with_a_code_keeps_the_key_when_the_code_is_refused() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("GOOD-CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;

    assert_eq!(
        handle.dispatch(join("BAD-CODE")).await,
        Err(refused(ErrorCode::InviteNotFound))
    );
    let view = handle.access();
    assert_eq!(view.state, AccessState::NoAccount { notice: None });
    assert_eq!(view.error, Some(refused(ErrorCode::InviteNotFound)));
    let key = handle.device_public_key().unwrap();

    handle.dispatch(join("GOOD-CODE")).await.unwrap();
    assert_eq!(handle.access().error, None);
    assert_eq!(server.account_of(key), Some(logged_in(&handle)));
    assert!(matches!(
        handle.entry().membership,
        Membership::Member { .. }
    ));

    let second = self::device(&network);
    let late = second.add("acme").await;
    assert_eq!(
        late.dispatch(join("GOOD-CODE")).await,
        Err(refused(ErrorCode::InviteExhausted))
    );
    let invalid = AccessAction::Join {
        code: InviteCode::new("GOOD-CODE"),
        name: " ".to_owned(),
        device_name: String::new(),
    };
    assert!(matches!(
        late.dispatch(invalid).await,
        Err(FlowError::InvalidInput(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn a_join_request_waits_across_a_restart_and_ends_in_each_possible_way() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    let request = AccessAction::RequestAccess {
        name: "Grace".to_owned(),
    };

    handle.dispatch(request.clone()).await.unwrap();
    let key = handle.device_public_key().unwrap();
    let AccessState::WaitingForApproval {
        join_request_id,
        name,
    } = handle.access().state
    else {
        panic!("not waiting");
    };
    assert_eq!(name, "Grace");
    assert!(join_request_id.is_some());
    assert!(!handle.access().actions.contains(&ActionKind::RequestAccess));
    handle.dispatch(AccessAction::Check).await.unwrap();
    assert!(matches!(
        handle.access().state,
        AccessState::WaitingForApproval { .. }
    ));

    let restarted = device.reopened(&network);
    let handle = restarted.server(server.server_id()).unwrap();
    assert_eq!(
        handle.access().state,
        AccessState::WaitingForApproval {
            join_request_id,
            name: "Grace".to_owned()
        }
    );
    server.decide_request(key, false);
    handle.connect().await.unwrap();
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount {
            notice: Some(Notice::RequestDenied)
        }
    );
    assert_eq!(handle.entry().membership, Membership::None);

    handle.dispatch(request.clone()).await.unwrap();
    server.expire_request(key);
    handle.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount {
            notice: Some(Notice::RequestExpired)
        }
    );

    handle.dispatch(request).await.unwrap();
    server.decide_request(key, true);
    handle.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(server.account_of(key), Some(logged_in(&handle)));
    assert_eq!(handle.device_public_key().unwrap(), key);
}

#[tokio::test(start_paused = true)]
async fn waiting_for_a_decision_asks_again_until_the_request_is_approved() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle
        .dispatch(AccessAction::RequestAccess {
            name: "Grace".to_owned(),
        })
        .await
        .unwrap();
    let key = handle.device_public_key().unwrap();

    let approver = {
        let server = server.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(35)).await;
            server.decide_request(key, true);
        })
    };
    let started = tokio::time::Instant::now();
    let state = handle
        .wait_for_decision(Duration::from_secs(10))
        .await
        .unwrap();
    approver.await.unwrap();
    assert!(matches!(state, AccessState::LoggedIn { .. }));
    assert_eq!(started.elapsed(), Duration::from_secs(40));
    assert_eq!(server.count("/join-requests/status"), 4);
}

#[tokio::test(start_paused = true)]
async fn a_request_that_the_server_already_holds_is_found_again() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    server.fail("/join-requests", [Fault::LoseAnswer]);
    handle
        .dispatch(AccessAction::RequestAccess {
            name: "Grace".to_owned(),
        })
        .await
        .unwrap();
    assert!(matches!(
        handle.access().state,
        AccessState::WaitingForApproval { .. }
    ));
    assert!(matches!(
        handle.entry().membership,
        Membership::JoinRequested { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn a_new_device_is_linked_after_the_other_device_compared_and_approved_its_key() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let first = device(&network);
    let owner = first.add("acme").await;
    owner.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&owner);

    let second = device(&network);
    let newcomer = second.add("acme").await;
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
    assert_ne!(public_key, owner.device_public_key().unwrap());
    newcomer.dispatch(AccessAction::Check).await.unwrap();
    assert!(matches!(
        newcomer.access().state,
        AccessState::WaitingForLink { .. }
    ));

    let approval = owner.approve_link();
    assert_eq!(
        approval
            .dispatch(ApproveLinkAction::LookUp {
                code: LinkCode::new("NO-SUCH-CODE")
            })
            .await,
        Err(refused(ErrorCode::LinkCodeInvalid))
    );
    assert_eq!(approval.view().state, ApproveLinkState::EnterCode);
    assert_eq!(
        approval.dispatch(ApproveLinkAction::Approve).await,
        Err(FlowError::NotAvailable)
    );
    approval
        .dispatch(ApproveLinkAction::LookUp { code: code.clone() })
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
    assert_eq!(shown, public_key);
    assert_eq!(device_name, "Phone");
    assert_eq!(server.account_of(public_key), None);
    approval.dispatch(ApproveLinkAction::Approve).await.unwrap();
    assert!(matches!(
        approval.view().state,
        ApproveLinkState::Approved { .. }
    ));
    assert!(approval.view().actions.is_empty());

    newcomer.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(logged_in(&newcomer), account_id);
    assert_eq!(newcomer.api().devices().await.unwrap().len(), 2);
    assert_eq!(server.account_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_link_code_that_was_replaced_sends_the_approver_back_to_the_code() {
    let network = Arc::new(Network::default());
    network.host("acme");
    let first = device(&network);
    let owner = first.add("acme").await;
    owner.dispatch(claim()).await.unwrap();
    let second = device(&network);
    let newcomer = second.add("acme").await;
    let link = AccessAction::RequestLink {
        device_name: String::new(),
    };
    newcomer.dispatch(link.clone()).await.unwrap();
    let AccessState::WaitingForLink { code, .. } = newcomer.access().state else {
        panic!("not waiting for a link");
    };
    let approval = owner.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    newcomer.dispatch(link).await.unwrap();
    assert_eq!(
        approval.dispatch(ApproveLinkAction::Approve).await,
        Err(refused(ErrorCode::LinkCodeInvalid))
    );
    let view = approval.view();
    assert_eq!(view.state, ApproveLinkState::EnterCode);
    assert_eq!(view.error, Some(refused(ErrorCode::LinkCodeInvalid)));
}

#[tokio::test(start_paused = true)]
async fn a_ban_is_found_at_the_next_request_and_an_unban_restores_the_account() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&handle);
    let key = handle.device_public_key().unwrap();

    server.ban(account_id);
    assert!(matches!(
        handle.api().own_account().await,
        Err(ApiError::AccessLost(AccessLoss::Banned))
    ));
    let view = handle.access();
    assert_eq!(
        view.state,
        AccessState::Banned {
            account_id: Some(account_id)
        }
    );
    assert_eq!(view.actions, [ActionKind::Check, ActionKind::ForgetAccount]);
    assert_eq!(view.in_progress, None);
    assert_eq!(view.error, Some(FlowError::AccessLost));
    assert!(matches!(
        handle.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));

    let restarted = device.reopened(&network);
    let handle = restarted.server(server.server_id()).unwrap();
    assert!(matches!(handle.access().state, AccessState::Banned { .. }));
    handle.connect().await.unwrap();
    assert!(matches!(handle.access().state, AccessState::Banned { .. }));
    assert_eq!(
        handle.dispatch(join("ANY")).await,
        Err(FlowError::NotAvailable)
    );

    server.unban(account_id);
    handle.dispatch(AccessAction::Check).await.unwrap();
    assert_eq!(logged_in(&handle), account_id);
    assert_eq!(handle.device_public_key().unwrap(), key);
}

#[tokio::test(start_paused = true)]
async fn a_banned_device_can_forget_its_key_and_join_as_a_new_account() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("NEW-CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let banned = logged_in(&handle);
    let old_key = handle.device_public_key().unwrap();
    server.ban(banned);
    handle.dispatch(AccessAction::Check).await.unwrap();

    handle.dispatch(AccessAction::ForgetAccount).await.unwrap();
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount { notice: None }
    );
    assert!(!device.client.data_dir().key_file(handle.id()).exists());
    handle.dispatch(join("NEW-CODE")).await.unwrap();
    assert_ne!(logged_in(&handle), banned);
    assert_ne!(handle.device_public_key().unwrap(), old_key);
}

#[tokio::test(start_paused = true)]
async fn a_removal_is_found_and_the_same_key_joins_again_as_a_new_account() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("BACK", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let removed = logged_in(&handle);
    let key = handle.device_public_key().unwrap();

    server.remove(removed);
    assert!(matches!(
        handle.api().devices().await,
        Err(ApiError::AccessLost(AccessLoss::Removed))
    ));
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount {
            notice: Some(Notice::Removed)
        }
    );
    assert_eq!(handle.entry().membership, Membership::Removed);
    assert_eq!(
        device
            .reopened(&network)
            .server(handle.id())
            .unwrap()
            .access()
            .state,
        AccessState::NoAccount {
            notice: Some(Notice::Removed)
        }
    );

    handle.dispatch(join("BACK")).await.unwrap();
    assert_ne!(logged_in(&handle), removed);
    assert_eq!(handle.device_public_key().unwrap(), key);
}

#[tokio::test(start_paused = true)]
async fn an_ended_session_is_renewed_once_without_an_action_of_the_user() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&handle);

    server.end_sessions();
    let api = handle.api();
    let (first, second) = tokio::join!(api.own_account(), api.devices());
    assert_eq!(first.unwrap().id, account_id);
    assert_eq!(second.unwrap().len(), 1);
    assert_eq!(logged_in(&handle), account_id);
    assert_eq!(server.count("/auth/sessions"), 1);
    assert_eq!(server.session_count(), 1);
    assert_eq!(handle.access().error, None);

    handle.dispatch(AccessAction::LogOut).await.unwrap();
    assert_eq!(handle.access().state, AccessState::LoggedOut { account_id });
    assert_eq!(server.session_count(), 0);
    assert!(matches!(
        handle.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));
    handle.dispatch(AccessAction::LogIn).await.unwrap();
    assert_eq!(logged_in(&handle), account_id);
}

#[tokio::test(start_paused = true)]
async fn a_busy_server_gets_the_same_signed_request_again() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    server.fail("/auth/challenges", [Fault::Busy]);
    server.fail("/join", [Fault::Busy, Fault::Busy]);

    let started = tokio::time::Instant::now();
    handle.dispatch(join("CODE")).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(3));
    logged_in(&handle);
    assert_eq!(server.count("/auth/challenges"), 2);
    assert_eq!(server.count("/join"), 3);
    assert_eq!(server.account_count(), 1);

    server.fail("/account", (0..10).map(|_| Fault::Busy));
    let error = handle.api().own_account().await.unwrap_err();
    assert_eq!(error.code(), Some(ErrorCode::Busy));
    assert_eq!(
        server.count("/account"),
        RetryPolicy::default().attempts as usize
    );
    logged_in(&handle);
    assert!(matches!(handle.reach(), Reach::Reachable { .. }));
}

#[tokio::test(start_paused = true)]
async fn a_lost_answer_of_a_join_does_not_create_two_accounts_or_lose_the_account() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 2);
    let device = device(&network);
    let handle = device.add("acme").await;

    server.fail("/join", [Fault::LoseAnswer]);
    handle.dispatch(join("CODE")).await.unwrap();
    let account_id = logged_in(&handle);
    assert_eq!(server.account_count(), 1);
    assert_eq!(server.count("/join"), 2);
    assert_eq!(
        server.account_of(handle.device_public_key().unwrap()),
        Some(account_id)
    );

    let other = self::device(&network);
    let handle = other.add("acme").await;
    server.fail("/join", [Fault::LoseRequest]);
    handle.dispatch(join("CODE")).await.unwrap();
    logged_in(&handle);
    assert_eq!(server.account_count(), 2);
}

#[tokio::test(start_paused = true)]
async fn an_account_that_the_book_does_not_know_is_found_by_a_check_or_a_second_join() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 2);
    for second_join in [false, true] {
        let device = device(&network);
        let handle = device.add("acme").await;
        handle.dispatch(join("CODE")).await.unwrap();
        let account_id = logged_in(&handle);
        device
            .client
            .inner
            .core
            .book
            .update(handle.entry().key(), |entry| {
                entry.membership = Membership::None
            })
            .unwrap();

        let restarted = device.reopened(&network);
        let handle = restarted.server(server.server_id()).unwrap();
        assert_eq!(
            handle.access().state,
            AccessState::NoAccount { notice: None }
        );
        if second_join {
            handle.dispatch(join("CODE")).await.unwrap();
        } else {
            handle.connect().await.unwrap();
        }
        assert_eq!(logged_in(&handle), account_id);
        assert!(matches!(
            handle.entry().membership,
            Membership::Member { .. }
        ));
    }
    assert_eq!(server.account_count(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_missing_key_of_a_member_is_reported_and_not_replaced_silently() {
    let network = Arc::new(Network::default());
    network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    std::fs::remove_file(device.client.data_dir().key_file(handle.id())).unwrap();

    let restarted = device.reopened(&network);
    let handle = restarted.servers().remove(0);
    handle.connect().await.unwrap();
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount {
            notice: Some(Notice::KeyMissing)
        }
    );
    assert_eq!(handle.entry().membership, Membership::None);
    assert!(!restarted.data_dir().key_file(handle.id()).exists());
}

#[tokio::test(start_paused = true)]
async fn one_unreachable_server_does_not_affect_another() {
    let network = Arc::new(Network::default());
    let acme = network.host("acme");
    let globex = network.host("globex");
    let device = device(&network);
    let first = device.add("acme").await;
    let second = device.add("globex").await;
    first.dispatch(claim()).await.unwrap();
    second.dispatch(claim()).await.unwrap();
    let (first_account, second_account) = (logged_in(&first), logged_in(&second));
    assert_ne!(
        first.device_public_key().unwrap(),
        second.device_public_key().unwrap()
    );

    globex.set_down(true);
    let restarted = device.reopened(&network);
    let results = restarted.connect_all().await;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], (acme.server_id(), Ok(())));
    assert_eq!(results[1].0, globex.server_id());
    let failure = results[1].1.clone().unwrap_err();
    assert!(matches!(failure, FlowError::Unreachable(_)), "{failure}");
    let first = restarted.server(acme.server_id()).unwrap();
    let second = restarted.server(globex.server_id()).unwrap();
    assert_eq!(logged_in(&first), first_account);
    assert_eq!(first.api().own_account().await.unwrap().id, first_account);
    assert_eq!(second.reach(), Reach::Unreachable);
    assert_eq!(
        second.access().state,
        AccessState::LoggedOut {
            account_id: second_account
        }
    );
    assert_eq!(second.access().error, Some(failure));
    assert!(matches!(
        second.entry().membership,
        Membership::Member { .. }
    ));

    globex.set_down(false);
    second.connect().await.unwrap();
    assert_eq!(logged_in(&second), second_account);
    assert!(matches!(second.reach(), Reach::Reachable { .. }));

    acme.set_down(true);
    assert!(matches!(
        first.api().own_account().await,
        Err(ApiError::Unreachable(_))
    ));
    assert_eq!(first.reach(), Reach::Unreachable);
    assert_eq!(logged_in(&first), first_account);
    assert_eq!(second.api().own_account().await.unwrap().id, second_account);

    acme.set_down(false);
    assert_eq!(first.api().own_account().await.unwrap().id, first_account);
    assert!(matches!(first.reach(), Reach::Reachable { .. }));
    assert_eq!(acme.count("/auth/sessions"), 1);
}

#[tokio::test(start_paused = true)]
async fn an_address_that_answers_as_another_server_gets_no_proof_and_no_token() {
    let network = Arc::new(Network::default());
    let real = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&handle);

    let other = FakeServer::new("acme");
    network.plug("acme", other.clone());
    assert_eq!(
        handle.dispatch(AccessAction::Check).await,
        Err(FlowError::OtherServer)
    );
    assert_eq!(
        handle.reach(),
        Reach::OtherServer {
            reported: other.server_id()
        }
    );
    assert_eq!(handle.access().state, AccessState::LoggedOut { account_id });
    assert!(matches!(
        handle.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));
    assert_eq!(
        handle.dispatch(AccessAction::LogIn).await,
        Err(FlowError::OtherServer)
    );
    assert_eq!(other.seen(), ["Get /api/server", "Get /api/server"]);

    network.plug("acme", real);
    handle.dispatch(AccessAction::LogIn).await.unwrap();
    assert_eq!(logged_in(&handle), account_id);
}

#[tokio::test(start_paused = true)]
async fn a_challenge_that_names_another_server_is_not_signed() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    let relayed = ServerId::generate();
    server.set_challenge_server_id(relayed);

    let error = handle.dispatch(join("CODE")).await.unwrap_err();
    assert_eq!(
        error,
        FlowError::Challenge(ChallengeMismatch::Server {
            expected: server.server_id(),
            got: relayed
        })
    );
    assert_eq!(server.count("/join"), 0);
    assert_eq!(server.account_count(), 0);
    assert_eq!(
        handle.access().state,
        AccessState::NoAccount { notice: None }
    );
}

#[tokio::test(start_paused = true)]
async fn each_server_gets_the_newest_api_version_that_both_sides_speak() {
    let network = Arc::new(Network::default());
    let newer = network.host("newer");
    newer.set_info(|info| info.api_versions = vec![1, 2]);
    let older = network.host("older");
    let future = network.host("future");
    future.set_info(|info| info.api_versions = vec![3, 4]);

    let directory = tempfile::tempdir().unwrap();
    let client = open(
        &network,
        directory.path(),
        SupportedVersions::new(2, Some(1)),
    );
    let on_newer = add(&client, "newer").await;
    let on_older = add(&client, "older").await;
    let on_future = add(&client, "future").await;

    on_newer.dispatch(claim()).await.unwrap();
    on_older.dispatch(claim()).await.unwrap();
    assert!(matches!(on_newer.reach(), Reach::Reachable { version } if version.number() == 2));
    assert!(matches!(on_older.reach(), Reach::Reachable { version } if version.number() == 1));
    assert!(
        newer
            .seen()
            .contains(&"Post /api/v2/owner/claim".to_owned())
    );
    assert!(
        older
            .seen()
            .contains(&"Post /api/v1/owner/claim".to_owned())
    );
    on_newer.api().own_account().await.unwrap();
    assert_eq!(newer.seen().last().unwrap(), "Get /api/v2/account");

    let incompatible = Incompatibility::ClientTooOld { server_oldest: 3 };
    assert_eq!(
        on_future.dispatch(claim()).await,
        Err(FlowError::Incompatible(incompatible))
    );
    assert_eq!(on_future.reach(), Reach::Incompatible(incompatible));
    assert_eq!(future.seen(), ["Get /api/server", "Get /api/server"]);
    logged_in(&on_newer);
    logged_in(&on_older);

    newer.set_info(|info| info.api_versions = vec![1]);
    on_newer.dispatch(AccessAction::Check).await.unwrap();
    assert!(matches!(on_newer.reach(), Reach::Reachable { version } if version.number() == 1));
    on_newer.api().own_account().await.unwrap();
    assert_eq!(newer.seen().last().unwrap(), "Get /api/v1/account");
    assert_eq!(on_newer.entry().api_versions, [1]);

    let old_client = open(&network, directory.path(), SupportedVersions::THIS_BUILD);
    let on_future = old_client.server(future.server_id()).unwrap();
    assert!(on_future.connect().await.is_err());
    assert_eq!(on_future.reach(), Reach::Incompatible(incompatible));
}

#[tokio::test(start_paused = true)]
async fn a_flow_runs_one_action_at_a_time_and_only_actions_of_its_state() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;

    assert_eq!(
        handle.dispatch(AccessAction::LogOut).await,
        Err(FlowError::NotAvailable)
    );
    assert_eq!(handle.access().error, None);

    server.fail("/join", [Fault::Busy]);
    let running = handle.dispatch(join("CODE"));
    tokio::task::yield_now().await;
    let during = handle.access();
    assert_eq!(during.in_progress, Some(ActionKind::Join));
    assert!(during.actions.is_empty());
    assert_eq!(during.state, AccessState::NoAccount { notice: None });
    assert_eq!(
        handle.dispatch(AccessAction::Check).await,
        Err(FlowError::Busy)
    );
    running.await.unwrap();
    let after = handle.access();
    assert_eq!(after.in_progress, None);
    assert_eq!(after.actions, [ActionKind::Check, ActionKind::LogOut]);
    assert_eq!(server.account_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_action_finishes_when_the_interface_drops_its_future() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    server.fail("/join", [Fault::Busy]);

    let mut changes = device.client.changes();
    drop(handle.dispatch(join("CODE")));
    while handle.access().in_progress.is_some()
        || !matches!(handle.access().state, AccessState::LoggedIn { .. })
    {
        assert!(changes.changed().await);
    }
    logged_in(&handle);
    assert!(matches!(
        handle.entry().membership,
        Membership::Member { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn a_request_that_races_with_a_logout_does_not_log_in_again() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&handle);

    server.fail("/account", [Fault::Busy]);
    let api = handle.api();
    let request = tokio::spawn(async move { api.own_account().await });
    while server.count("/account") == 0 {
        tokio::task::yield_now().await;
    }
    handle.dispatch(AccessAction::LogOut).await.unwrap();
    assert!(matches!(request.await.unwrap(), Err(ApiError::NotLoggedIn)));
    assert_eq!(server.count("/account"), 2);
    assert_eq!(handle.access().state, AccessState::LoggedOut { account_id });
    assert_eq!(server.session_count(), 0);
    assert_eq!(server.count("/auth/sessions"), 0);
}

#[tokio::test(start_paused = true)]
async fn a_server_is_not_removed_while_its_flow_runs_an_action() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    server.fail("/join", [Fault::Busy]);
    let running = handle.dispatch(join("CODE"));
    tokio::task::yield_now().await;
    assert!(matches!(
        device.client.remove_server(handle.id()),
        Err(RemoveError::Busy)
    ));
    running.await.unwrap();
    logged_in(&handle);
    device.client.remove_server(handle.id()).unwrap();
    assert!(device.client.servers().is_empty());
}

#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<(ServerId, ConnectionLevel, Option<StreamTarget>)>>,
}

impl EventStreams for Recorder {
    fn set_level(&self, server_id: ServerId, level: ConnectionLevel, target: Option<StreamTarget>) {
        self.calls.lock().unwrap().push((server_id, level, target));
    }
}

impl Recorder {
    fn levels(&self) -> Vec<ConnectionLevel> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, level, _)| *level)
            .collect()
    }
}

#[tokio::test(start_paused = true)]
async fn the_event_streams_learn_the_level_of_each_server_and_get_the_stored_cursor() {
    use ConnectionLevel::{Background, Off, Selected};
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    let recorder = Arc::new(Recorder::default());
    device.client.attach_event_streams(recorder.clone());
    assert!(recorder.levels().is_empty());
    assert_eq!(handle.connection_level(), Off);

    handle.dispatch(claim()).await.unwrap();
    assert_eq!(recorder.levels(), [Background]);
    device.client.select(Some(handle.id()));
    assert_eq!(device.client.selected(), Some(handle.id()));
    handle
        .set_connection_mode(ConnectionMode::WhenSelected)
        .unwrap();
    device.client.select(None);
    assert_eq!(recorder.levels(), [Background, Selected, Off]);
    handle.set_connection_mode(ConnectionMode::Always).unwrap();

    handle
        .store_cursor(EventCursor::new("cursor-17").unwrap())
        .unwrap();
    server.end_sessions();
    handle.api().own_account().await.unwrap();
    {
        let calls = recorder.calls.lock().unwrap();
        assert_eq!(calls.len(), 5);
        let (server_id, level, target) = &calls[4];
        let target = target.as_ref().unwrap();
        assert_eq!((*server_id, *level), (handle.id(), Background));
        assert_eq!(target.cursor.as_ref().unwrap().as_str(), "cursor-17");
        assert_eq!(target.api_version.number(), 1);
        assert_eq!(target.address.as_str(), "https://acme.example.test");
        let first_token = calls[0].2.as_ref().unwrap().token.expose().to_owned();
        assert_ne!(target.token.expose(), first_token);
    }

    handle.dispatch(AccessAction::LogOut).await.unwrap();
    assert_eq!(recorder.levels().last(), Some(&Off));
    assert_eq!(
        device
            .reopened(&network)
            .servers()
            .remove(0)
            .entry()
            .last_cursor
            .unwrap()
            .as_str(),
        "cursor-17"
    );
}

#[tokio::test(start_paused = true)]
async fn removing_a_server_deletes_its_entry_and_its_key_and_stops_its_flow() {
    let network = Arc::new(Network::default());
    network.host("acme");
    network.host("globex");
    let device = device(&network);
    let removed = device.add("acme").await;
    let kept = device.add("globex").await;
    removed.dispatch(claim()).await.unwrap();
    kept.dispatch(claim()).await.unwrap();
    device.client.select(Some(removed.id()));

    device.client.remove_server(removed.id()).unwrap();
    assert_eq!(device.client.selected(), None);
    assert!(device.client.server(removed.id()).is_none());
    assert!(!device.client.data_dir().key_file(removed.id()).exists());
    assert!(device.client.data_dir().key_file(kept.id()).exists());
    assert!(removed.access().actions.is_empty());
    assert_eq!(
        removed.dispatch(AccessAction::Check).await,
        Err(FlowError::NotAvailable)
    );
    assert!(matches!(
        removed.api().own_account().await,
        Err(ApiError::NotLoggedIn)
    ));
    assert!(matches!(
        device.client.remove_server(removed.id()),
        Err(RemoveError::Book(BookError::NotSaved { .. }))
    ));
    let reopened = device.reopened(&network);
    assert_eq!(reopened.servers().len(), 1);
    assert_eq!(reopened.servers()[0].id(), kept.id());
    kept.api().own_account().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_corrupted_book_stops_the_start_and_leaves_the_keys() {
    let network = Arc::new(Network::default());
    network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let data_dir = device.client.data_dir().clone();
    std::fs::write(
        data_dir.book_file(),
        b"{\"format_version\":1,\"servers\":[{",
    )
    .unwrap();

    let mut options = ClientOptions::new(data_dir.root());
    options.transport = Some(network.clone());
    assert!(matches!(
        Client::open(options),
        Err(OpenError::Storage(StorageError::Corrupted { .. }))
    ));
    assert!(data_dir.key_file(handle.id()).exists());

    ServerBook::new(data_dir.clone())
        .quarantine(1_800_000_000)
        .unwrap();
    let recovered = device.reopened(&network);
    assert!(recovered.servers().is_empty());
    let handle = add(&recovered, "acme").await;
    handle.connect().await.unwrap();
    logged_in(&handle);
}

#[tokio::test(start_paused = true)]
async fn a_key_without_a_book_entry_is_not_used_at_another_address() {
    let network = Arc::new(Network::default());
    let real = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&handle);
    let key = handle.device_public_key().unwrap();
    device
        .client
        .inner
        .core
        .book
        .remove(handle.entry().key(), || Ok::<(), BookError>(()))
        .unwrap();
    assert!(device.client.data_dir().key_file(handle.id()).exists());
    let logins_before = real.count("/auth/sessions");

    network.plug("relay", real.clone());
    let restarted = device.reopened(&network);
    let flow = restarted.add_server();
    let fetched = flow
        .dispatch(AddServerAction::Fetch {
            address: "relay.example.test".to_owned(),
        })
        .await;
    let acme = ServerAddress::parse("acme.example.test").unwrap();
    assert_eq!(
        fetched,
        Err(FlowError::KeyOfOtherAddress {
            stored: acme.clone()
        })
    );
    let view = flow.view();
    assert!(matches!(
        &view.state,
        AddServerState::KeyOfOtherAddress { key_address, .. } if *key_address == acme
    ));
    assert_eq!(
        view.actions,
        [AddServerActionKind::DiscardKey, AddServerActionKind::Reset]
    );
    assert_eq!(
        flow.dispatch(AddServerAction::Save {
            connection_mode: ConnectionMode::Always
        })
        .await,
        Err(FlowError::NotAvailable)
    );
    assert!(restarted.servers().is_empty());
    assert_eq!(real.count("/auth/sessions"), logins_before);
    assert_eq!(real.count("/auth/challenges"), 1);
    assert!(restarted.data_dir().key_file(real.server_id()).exists());

    let genuine = add(&restarted, "acme").await;
    genuine.connect().await.unwrap();
    assert_eq!(logged_in(&genuine), account_id);
    assert_eq!(genuine.device_public_key().unwrap(), key);
}

#[tokio::test(start_paused = true)]
async fn a_key_of_another_address_is_used_nowhere_until_the_user_discards_it() {
    let network = Arc::new(Network::default());
    let real = network.host("acme");
    real.add_invite("NEW", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let old_key = handle.device_public_key().unwrap();
    let key = handle.entry().key();
    let book = &device.client.inner.core.book;
    book.remove(key, || Ok::<(), BookError>(())).unwrap();

    network.plug("moved", real.clone());
    let restarted = device.reopened(&network);
    let flow = restarted.add_server();
    let fetch = AddServerAction::Fetch {
        address: "moved.example.test".to_owned(),
    };
    assert!(flow.dispatch(fetch).await.is_err());
    flow.dispatch(AddServerAction::DiscardKey).await.unwrap();
    assert!(matches!(flow.view().state, AddServerState::Preview(_)));
    assert!(!restarted.data_dir().key_file(real.server_id()).exists());
    flow.dispatch(AddServerAction::Save {
        connection_mode: ConnectionMode::Always,
    })
    .await
    .unwrap();

    let moved = restarted.server(real.server_id()).unwrap();
    moved.connect().await.unwrap();
    assert_eq!(
        moved.access().state,
        AccessState::NoAccount { notice: None }
    );
    moved.dispatch(join("NEW")).await.unwrap();
    assert_ne!(moved.device_public_key().unwrap(), old_key);
}

#[tokio::test(start_paused = true)]
async fn an_entry_whose_address_differs_from_the_origin_of_its_key_gets_no_proof() {
    let network = Arc::new(Network::default());
    let real = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let data_dir = device.client.data_dir().clone();
    let stored = std::fs::read_to_string(data_dir.book_file()).unwrap();
    std::fs::write(
        data_dir.book_file(),
        stored.replace("acme.example.test", "relay.example.test"),
    )
    .unwrap();

    network.plug("relay", real.clone());
    let challenges_before = real.count("/auth/challenges");
    let restarted = device.reopened(&network);
    let relayed = restarted.servers().remove(0);
    assert_eq!(
        relayed.entry().address.as_str(),
        "https://relay.example.test"
    );
    let error = relayed.connect().await.unwrap_err();
    assert!(
        matches!(error, FlowError::KeyOfOtherAddress { .. }),
        "{error}"
    );
    assert_eq!(real.count("/auth/challenges"), challenges_before);
    assert!(matches!(
        relayed.device_public_key(),
        Err(KeyStoreError::OtherOrigin { .. })
    ));
    assert!(matches!(
        relayed.access().state,
        AccessState::LoggedOut { .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn a_renewal_into_another_account_fails_the_request_and_does_not_repeat_it() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let first = device(&network);
    let moved = first.add("acme").await;
    moved.dispatch(claim()).await.unwrap();
    let account_a = logged_in(&moved);
    let second = device(&network);
    let other = second.add("acme").await;
    other.dispatch(join("CODE")).await.unwrap();
    let account_b = logged_in(&other);

    server.reassign(moved.device_public_key().unwrap(), account_b);
    let result = moved.api().devices().await;
    assert!(
        matches!(
            result,
            Err(ApiError::AccountChanged { was, now }) if was == account_a && now == account_b
        ),
        "the request ran for another account"
    );
    assert_eq!(server.count("/account/devices"), 1);
    let AccessState::LoggedIn {
        account_id,
        previous_account,
        ..
    } = moved.access().state
    else {
        panic!("not logged in");
    };
    assert_eq!((account_id, previous_account), (account_b, Some(account_a)));
    assert_eq!(moved.account(), Some(account_b));
    assert_eq!(moved.api().devices().await.unwrap().len(), 2);
    assert!(matches!(
        moved.api_for(account_a).devices().await,
        Err(ApiError::AccountChanged { .. })
    ));
    assert_eq!(server.count("/account/devices"), 2);
}

#[tokio::test(start_paused = true)]
async fn a_link_that_was_confirmed_for_one_account_is_not_approved_into_another() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let first = device(&network);
    let approver = first.add("acme").await;
    approver.dispatch(claim()).await.unwrap();
    let account_a = logged_in(&approver);
    let second = device(&network);
    let other = second.add("acme").await;
    other.dispatch(join("CODE")).await.unwrap();
    let account_b = logged_in(&other);
    let third = device(&network);
    let newcomer = third.add("acme").await;
    newcomer
        .dispatch(AccessAction::RequestLink {
            device_name: String::new(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForLink {
        code, public_key, ..
    } = newcomer.access().state
    else {
        panic!("not waiting for a link");
    };

    let approval = approver.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code: code.clone() })
        .await
        .unwrap();
    server.reassign(approver.device_public_key().unwrap(), account_b);
    assert!(matches!(
        approval.dispatch(ApproveLinkAction::Approve).await,
        Err(FlowError::AccountChanged { now, .. }) if now == account_b
    ));
    assert_eq!(server.account_of(public_key), None);
    assert_eq!(approval.view().state, ApproveLinkState::EnterCode);
    assert_eq!(server.count("/account/device-links/approve"), 1);

    let late = approver.approve_link();
    late.dispatch(ApproveLinkAction::LookUp { code: code.clone() })
        .await
        .unwrap();
    server.reassign(approver.device_public_key().unwrap(), account_a);
    approver.api().own_account().await.unwrap_err();
    assert_eq!(approver.account(), Some(account_a));
    assert!(matches!(
        late.dispatch(ApproveLinkAction::Approve).await,
        Err(FlowError::AccountChanged { .. })
    ));
    assert_eq!(server.account_of(public_key), None);
    assert_eq!(server.count("/account/device-links/approve"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_stale_client_does_not_remove_a_server_that_another_client_added_again() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("AGAIN", 1);
    let device = device(&network);
    let stale = device.add("acme").await;
    stale.dispatch(claim()).await.unwrap();

    let other = device.reopened(&network);
    other.remove_server(stale.id()).unwrap();
    let again = add(&other, "acme").await;
    again.dispatch(join("AGAIN")).await.unwrap();
    let key = again.device_public_key().unwrap();

    assert!(device.client.remove_server(stale.id()).is_err());
    assert!(other.data_dir().key_file(again.id()).exists());
    let reopened = device.reopened(&network);
    let kept = reopened
        .server(again.id())
        .expect("the entry of the other client");
    kept.connect().await.unwrap();
    logged_in(&kept);
    assert_eq!(kept.device_public_key().unwrap(), key);
}

#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_membership_that_could_not_be_written_is_written_at_the_next_contact() {
    use std::os::unix::fs::PermissionsExt;
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.device_public_key().unwrap();
    let root = device.client.data_dir().root().to_owned();

    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let joined = handle.dispatch(join("CODE")).await;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(joined, Err(FlowError::Storage(_))), "{joined:?}");
    let account_id = logged_in(&handle);

    handle.dispatch(AccessAction::Check).await.unwrap();
    let reopened = device.reopened(&network);
    assert_eq!(
        reopened.server(handle.id()).unwrap().access().state,
        AccessState::LoggedOut { account_id }
    );
}

#[tokio::test(start_paused = true)]
async fn a_link_approval_whose_answer_was_lost_is_found_in_the_device_list() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let first = device(&network);
    let owner = first.add("acme").await;
    owner.dispatch(claim()).await.unwrap();
    let second = device(&network);
    let newcomer = second.add("acme").await;
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

    let approval = owner.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    server.fail("/account/device-links/approve", [Fault::LoseAnswer]);
    approval.dispatch(ApproveLinkAction::Approve).await.unwrap();
    let ApproveLinkState::Approved { device } = approval.view().state else {
        panic!("not approved: {:?}", approval.view().state);
    };
    assert_eq!(device.public_key, public_key);
    assert_eq!(server.count("/account/device-links/approve"), 1);
}

#[tokio::test(start_paused = true)]
async fn a_join_request_that_the_book_does_not_know_is_found_by_a_check() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle
        .dispatch(AccessAction::RequestAccess {
            name: "Grace".to_owned(),
        })
        .await
        .unwrap();
    let key = handle.device_public_key().unwrap();
    device
        .client
        .inner
        .core
        .book
        .update(handle.entry().key(), |entry| {
            entry.membership = Membership::None
        })
        .unwrap();

    let restarted = device.reopened(&network);
    let handle = restarted.server(server.server_id()).unwrap();
    handle.connect().await.unwrap();
    assert!(
        matches!(
            handle.access().state,
            AccessState::WaitingForApproval { .. }
        ),
        "{:?}",
        handle.access().state
    );
    server.decide_request(key, true);
    handle.dispatch(AccessAction::Check).await.unwrap();
    logged_in(&handle);
}

/// Reads the server book at the moment a request for a join arrives.
struct BookAtRequest {
    network: Arc<Network>,
    book_file: std::path::PathBuf,
    seen: Mutex<Vec<String>>,
}

impl Transport for BookAtRequest {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        if request.url.path().ends_with("/join-requests") {
            let stored = std::fs::read_to_string(&self.book_file).unwrap_or_default();
            self.seen.lock().unwrap().push(stored);
        }
        self.network.send(request)
    }
}

#[tokio::test(start_paused = true)]
async fn the_intent_of_a_join_request_is_stored_before_the_request_is_sent() {
    let network = Arc::new(Network::default());
    network.host("acme");
    let directory = tempfile::tempdir().unwrap();
    let data_dir = directory.path().join("junto");
    let spy = Arc::new(BookAtRequest {
        network: network.clone(),
        book_file: data_dir.join("servers.json"),
        seen: Mutex::default(),
    });
    let mut options = ClientOptions::new(&data_dir);
    options.transport = Some(spy.clone());
    let client = Client::open(options).unwrap();
    let handle = add(&client, "acme").await;
    handle
        .dispatch(AccessAction::RequestAccess {
            name: "Grace".to_owned(),
        })
        .await
        .unwrap();
    let seen = spy.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("join_requested"), "{}", seen[0]);
    assert!(seen[0].contains("Grace"));
}

#[tokio::test(start_paused = true)]
async fn a_corrupted_profile_stops_the_start() {
    let network = Arc::new(Network::default());
    let device = device(&network);
    let data_dir = device.client.data_dir().clone();
    std::fs::write(data_dir.profile_file(), b"{\"format_version\":1,").unwrap();
    let mut options = ClientOptions::new(data_dir.root());
    options.transport = Some(network.clone());
    assert!(matches!(
        Client::open(options),
        Err(OpenError::Storage(StorageError::Corrupted { .. }))
    ));
}

#[tokio::test(start_paused = true)]
async fn a_link_approval_without_an_answer_and_without_a_device_list_is_an_unknown_outcome() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let first = device(&network);
    let owner = first.add("acme").await;
    owner.dispatch(claim()).await.unwrap();
    let second = device(&network);
    let newcomer = second.add("acme").await;
    let link = AccessAction::RequestLink {
        device_name: String::new(),
    };
    newcomer.dispatch(link.clone()).await.unwrap();
    let AccessState::WaitingForLink {
        code, public_key, ..
    } = newcomer.access().state
    else {
        panic!("not waiting for a link");
    };

    let approval = owner.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    server.fail("/account/device-links/approve", [Fault::LoseAnswer]);
    server.fail("/account/devices", [Fault::LoseRequest, Fault::LoseRequest]);
    assert!(approval.dispatch(ApproveLinkAction::Approve).await.is_err());
    let view = approval.view();
    assert!(matches!(view.state, ApproveLinkState::OutcomeUnknown(_)));
    assert_eq!(
        view.actions,
        [ApproveLinkActionKind::Check, ApproveLinkActionKind::Reset]
    );
    assert!(approval.dispatch(ApproveLinkAction::Check).await.is_err());
    assert!(matches!(
        approval.view().state,
        ApproveLinkState::OutcomeUnknown(_)
    ));
    approval.dispatch(ApproveLinkAction::Check).await.unwrap();
    let ApproveLinkState::Approved { device } = approval.view().state else {
        panic!("not approved");
    };
    assert_eq!(device.public_key, public_key);

    let third = self::device(&network);
    let other = third.add("acme").await;
    other.dispatch(link).await.unwrap();
    let AccessState::WaitingForLink { code, .. } = other.access().state else {
        panic!("not waiting for a link");
    };
    let lost_request = owner.approve_link();
    lost_request
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    server.fail("/account/device-links/approve", [Fault::LoseRequest]);
    assert!(
        lost_request
            .dispatch(ApproveLinkAction::Approve)
            .await
            .is_err()
    );
    assert!(matches!(
        lost_request.view().state,
        ApproveLinkState::Confirm(_)
    ));
    lost_request
        .dispatch(ApproveLinkAction::Approve)
        .await
        .unwrap();
    assert_eq!(owner.api().devices().await.unwrap().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_join_request_without_an_answer_waits_until_a_check_finds_it() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    let request = AccessAction::RequestAccess {
        name: "Grace".to_owned(),
    };
    server.fail(
        "/join-requests",
        [Fault::LoseAnswer, Fault::LoseRequest, Fault::LoseRequest],
    );
    server.fail("/auth/challenges", [Fault::Busy]);
    server.fail("/join-requests/status", [Fault::LoseRequest; 3]);
    assert!(handle.dispatch(request.clone()).await.is_err());
    assert_eq!(
        handle.access().state,
        AccessState::WaitingForApproval {
            join_request_id: None,
            name: "Grace".to_owned()
        }
    );
    let restarted = device.reopened(&network);
    let handle = restarted.servers().remove(0);
    assert!(matches!(
        handle.access().state,
        AccessState::WaitingForApproval {
            join_request_id: None,
            ..
        }
    ));
    handle.connect().await.unwrap();
    assert!(matches!(
        handle.access().state,
        AccessState::WaitingForApproval {
            join_request_id: Some(_),
            ..
        }
    ));

    let other = self::device(&network);
    let refused = other.add("acme").await;
    server.fail("/join-requests", [Fault::Busy; 5]);
    assert!(refused.dispatch(request).await.is_err());
    assert_eq!(
        refused.access().state,
        AccessState::NoAccount { notice: None }
    );
    assert_eq!(refused.entry().membership, Membership::None);
    assert_eq!(
        other
            .reopened(&network)
            .servers()
            .remove(0)
            .entry()
            .membership,
        Membership::None
    );
}

struct Uploads {
    results: Mutex<Vec<Result<(), ApiError>>>,
    seen: Mutex<Vec<ProfileUpdate>>,
}

impl ProfileUploader for Uploads {
    fn upload(&self, api: ServerApi, update: ProfileUpdate) -> UploadFuture {
        self.seen.lock().unwrap().push(update);
        let result = self.results.lock().unwrap().remove(0);
        Box::pin(async move {
            api.own_account().await?;
            result
        })
    }
}

const AVATAR: &[u8] = b"\x89PNG\r\n\x1a\n-image-";

#[tokio::test(start_paused = true)]
async fn the_preset_waits_for_the_server_operation_after_this_device_created_an_account() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 2);
    let first = device(&network);
    first.client.profile().set_name("Ada").unwrap();
    first.client.profile().set_avatar(AVATAR).unwrap();
    let owner = first.add("acme").await;
    assert_eq!(owner.profile_sync(), ProfileSync::NotNeeded);
    owner.dispatch(claim()).await.unwrap();
    assert_eq!(owner.profile_sync(), ProfileSync::Pending { attempts: 0 });
    assert!(matches!(
        owner.apply_profile().await,
        Err(ApiError::NotOffered { .. })
    ));
    assert_eq!(owner.profile_sync(), ProfileSync::Pending { attempts: 0 });
    let restarted = first.reopened(&network);
    let owner = restarted.servers().remove(0);
    assert_eq!(owner.profile_sync(), ProfileSync::Pending { attempts: 0 });
    owner.connect().await.unwrap();
    assert_eq!(owner.profile_sync(), ProfileSync::Pending { attempts: 0 });

    let linked = device(&network);
    linked.client.profile().set_avatar(AVATAR).unwrap();
    let newcomer = linked.add("acme").await;
    newcomer
        .dispatch(AccessAction::RequestLink {
            device_name: String::new(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForLink { code, .. } = newcomer.access().state else {
        panic!("not waiting for a link");
    };
    let approval = owner.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    approval.dispatch(ApproveLinkAction::Approve).await.unwrap();
    newcomer.dispatch(AccessAction::Check).await.unwrap();
    logged_in(&newcomer);
    assert_eq!(newcomer.profile_sync(), ProfileSync::NotNeeded);

    let plain = device(&network);
    let without_avatar = plain.add("acme").await;
    without_avatar.dispatch(join("CODE")).await.unwrap();
    assert_eq!(without_avatar.profile_sync(), ProfileSync::NotNeeded);
}

#[tokio::test(start_paused = true)]
async fn the_preset_is_sent_after_a_join_and_a_failed_try_is_counted() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let uploads = Arc::new(Uploads {
        results: Mutex::new(vec![
            Err(ApiError::OutcomeUnknown),
            Err(ApiError::OutcomeUnknown),
            Ok(()),
        ]),
        seen: Mutex::default(),
    });
    let directory = tempfile::tempdir().unwrap();
    let open = || {
        let mut options = ClientOptions::new(directory.path().join("junto"));
        options.transport = Some(network.clone());
        options.profile_uploader = Some(uploads.clone());
        Client::open(options).unwrap()
    };
    let client = open();
    client.profile().set_name("Grace").unwrap();
    client.profile().set_avatar(AVATAR).unwrap();
    let handle = add(&client, "acme").await;

    handle.dispatch(join("CODE")).await.unwrap();
    logged_in(&handle);
    assert_eq!(handle.profile_sync(), ProfileSync::Pending { attempts: 1 });
    {
        let seen = uploads.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].name, "Grace");
        assert_eq!(
            seen[0].avatar,
            Some((crate::profile::AvatarFormat::Png, AVATAR.to_vec()))
        );
    }

    let restarted = open();
    let handle = restarted.servers().remove(0);
    assert_eq!(handle.profile_sync(), ProfileSync::Pending { attempts: 1 });
    assert!(matches!(
        handle.apply_profile().await,
        Err(ApiError::NotLoggedIn)
    ));
    handle.connect().await.unwrap();
    assert_eq!(handle.profile_sync(), ProfileSync::Pending { attempts: 2 });
    assert_eq!(handle.apply_profile().await.unwrap(), ProfileSync::Applied);
    assert_eq!(uploads.seen.lock().unwrap().len(), 3);
    assert_eq!(handle.apply_profile().await.unwrap(), ProfileSync::Applied);
    assert_eq!(uploads.seen.lock().unwrap().len(), 3);
    assert_eq!(
        open().servers().remove(0).profile_sync(),
        ProfileSync::Applied
    );
}

#[tokio::test(start_paused = true)]
async fn a_join_request_that_is_approved_marks_the_preset_as_waiting() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    device.client.profile().set_avatar(AVATAR).unwrap();
    let handle = device.add("acme").await;
    handle
        .dispatch(AccessAction::RequestAccess {
            name: "Grace".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(handle.profile_sync(), ProfileSync::NotNeeded);
    server.decide_request(handle.device_public_key().unwrap(), true);
    handle.dispatch(AccessAction::Check).await.unwrap();
    logged_in(&handle);
    assert_eq!(handle.profile_sync(), ProfileSync::Pending { attempts: 0 });
}

async fn owner_of(network: &Arc<Network>, names: &[&str]) -> (Device, Vec<Arc<FakeServer>>) {
    let servers: Vec<_> = names.iter().map(|name| network.host(name)).collect();
    let first = device(network);
    for name in names {
        first.add(name).await.dispatch(claim()).await.unwrap();
    }
    (first, servers)
}

fn links(flow: &LinkDeviceFlow) -> (Vec<LinkProgress>, Option<LinkRequestText>, Option<String>) {
    let LinkDeviceState::Requested {
        servers,
        request,
        fingerprint,
        ..
    } = flow.view().state
    else {
        panic!("no links were requested");
    };
    (
        servers.into_iter().map(|server| server.progress).collect(),
        request,
        fingerprint,
    )
}

fn approvals(flow: &ApproveDevicesFlow) -> (Vec<ApprovalProgress>, String, String) {
    let ApproveDevicesState::Confirm {
        fingerprint,
        device_name,
        servers,
    } = flow.view().state
    else {
        panic!("nothing to confirm");
    };
    (
        servers.into_iter().map(|server| server.progress).collect(),
        fingerprint,
        device_name,
    )
}

#[tokio::test(start_paused = true)]
async fn a_new_device_is_linked_on_every_server_of_the_first_device_in_one_flow() {
    use ApprovalProgress::{Approved, Ready};
    use LinkProgress::{Linked, Waiting};
    let network = Arc::new(Network::default());
    let (first, servers) = owner_of(&network, &["acme", "globex", "initech"]).await;
    let offer = first.client.link_offer();
    assert_eq!(offer.servers.len(), 3);

    let second = device(&network);
    let linking = second.client.link_this_device();
    assert_eq!(linking.view().actions, [LinkDeviceActionKind::Start]);
    linking
        .dispatch(LinkDeviceAction::Start {
            offer: offer.to_text(),
            device_name: " Phone ".to_owned(),
        })
        .await
        .unwrap();
    let (progress, request, fingerprint) = links(&linking);
    assert_eq!(progress, [Waiting, Waiting, Waiting]);
    let (request, fingerprint) = (request.unwrap(), fingerprint.unwrap());
    assert_eq!(second.client.servers().len(), 3);
    let keys: Vec<_> = second
        .client
        .servers()
        .iter()
        .map(|handle| handle.device_public_key().unwrap())
        .collect();
    assert!(keys[0] != keys[1] && keys[1] != keys[2] && keys[0] != keys[2]);

    let approving = first.client.approve_devices();
    approving
        .dispatch(ApproveDevicesAction::LookUp { request })
        .await
        .unwrap();
    let (progress, shown, device_name) = approvals(&approving);
    assert_eq!(progress, [Ready, Ready, Ready]);
    assert_eq!(shown, fingerprint);
    assert_eq!(device_name, "Phone");
    let ApproveDevicesState::Confirm { servers: shown, .. } = approving.view().state else {
        panic!("nothing to confirm");
    };
    assert_eq!(shown[0].name, "acme");
    for (server, handle) in shown.iter().zip(first.client.servers()) {
        assert_eq!(server.account_id, Some(logged_in(&handle)));
    }
    let shown_keys: Vec<_> = shown.iter().map(|server| server.public_key).collect();
    assert_eq!(shown_keys, keys);
    for (server, key) in servers.iter().zip(&keys) {
        assert_eq!(server.account_of(*key), None);
    }

    approving
        .dispatch(ApproveDevicesAction::Approve)
        .await
        .unwrap();
    assert_eq!(approvals(&approving).0, [Approved, Approved, Approved]);
    linking.dispatch(LinkDeviceAction::Check).await.unwrap();
    let (progress, request, fingerprint) = links(&linking);
    assert_eq!(progress, [Linked, Linked, Linked]);
    assert_eq!((request, fingerprint), (None, None));
    for (old, new) in first.client.servers().iter().zip(second.client.servers()) {
        assert_eq!(logged_in(&new), logged_in(old));
        assert_eq!(new.api().devices().await.unwrap().len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn a_server_that_fails_during_a_link_of_all_servers_is_reported_and_resumed_alone() {
    use ApprovalProgress::{Approved, Ready};
    use LinkProgress::{Linked, Waiting};
    let network = Arc::new(Network::default());
    let (first, servers) = owner_of(&network, &["acme", "globex", "initech"]).await;
    let (acme, globex, initech) = (&servers[0], &servers[1], &servers[2]);
    let offer = first.client.link_offer().to_text();

    globex.set_down(true);
    let second = device(&network);
    let linking = second.client.link_this_device();
    let started = linking
        .dispatch(LinkDeviceAction::Start {
            offer,
            device_name: "Phone".to_owned(),
        })
        .await;
    assert_eq!(
        started,
        Err(FlowError::Incomplete {
            failed: 1,
            total: 3
        })
    );
    let (progress, request, _) = links(&linking);
    assert_eq!(progress[0], Waiting);
    assert!(matches!(
        progress[1],
        LinkProgress::Failed(FlowError::Unreachable(_))
    ));
    assert_eq!(progress[2], Waiting);
    assert_eq!(second.client.servers().len(), 2);

    let approving = first.client.approve_devices();
    approving
        .dispatch(ApproveDevicesAction::LookUp {
            request: request.unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(approvals(&approving).0, [Ready, Ready]);
    initech.fail("/account/device-links/approve", [Fault::LoseRequest]);
    assert_eq!(
        approving.dispatch(ApproveDevicesAction::Approve).await,
        Err(FlowError::Incomplete {
            failed: 1,
            total: 2
        })
    );
    let (progress, _, _) = approvals(&approving);
    assert_eq!(progress[0], Approved);
    assert!(matches!(progress[1], ApprovalProgress::Failed(_)));
    assert_eq!(acme.count("/account/device-links/approve"), 1);

    approving
        .dispatch(ApproveDevicesAction::Approve)
        .await
        .unwrap();
    assert_eq!(approvals(&approving).0, [Approved, Approved]);
    assert_eq!(acme.count("/account/device-links/approve"), 1);
    assert_eq!(initech.count("/account/device-links/approve"), 2);

    globex.set_down(false);
    linking.dispatch(LinkDeviceAction::Retry).await.unwrap();
    let (progress, request, _) = links(&linking);
    assert_eq!(progress, [Linked, Waiting, Linked]);
    let again = first.client.approve_devices();
    again
        .dispatch(ApproveDevicesAction::LookUp {
            request: request.unwrap(),
        })
        .await
        .unwrap();
    assert_eq!(approvals(&again).0, [Ready]);
    again.dispatch(ApproveDevicesAction::Approve).await.unwrap();
    linking.dispatch(LinkDeviceAction::Check).await.unwrap();
    assert_eq!(links(&linking).0, [Linked, Linked, Linked]);
    assert_eq!(globex.account_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_approval_whose_answer_was_lost_is_found_when_all_servers_are_resumed() {
    use ApprovalProgress::Approved;
    let network = Arc::new(Network::default());
    let (first, servers) = owner_of(&network, &["acme", "globex"]).await;
    let second = device(&network);
    let linking = second.client.link_this_device();
    linking
        .dispatch(LinkDeviceAction::Start {
            offer: first.client.link_offer().to_text(),
            device_name: String::new(),
        })
        .await
        .unwrap();
    let approving = first.client.approve_devices();
    approving
        .dispatch(ApproveDevicesAction::LookUp {
            request: links(&linking).1.unwrap(),
        })
        .await
        .unwrap();
    servers[1].fail("/account/device-links/approve", [Fault::LoseAnswer]);
    servers[1].fail("/account/devices", [Fault::LoseRequest]);
    assert!(
        approving
            .dispatch(ApproveDevicesAction::Approve)
            .await
            .is_err()
    );
    assert_eq!(
        approvals(&approving).0,
        [Approved, ApprovalProgress::Unknown]
    );
    approving
        .dispatch(ApproveDevicesAction::Approve)
        .await
        .unwrap();
    assert_eq!(approvals(&approving).0, [Approved, Approved]);
    assert_eq!(servers[1].count("/account/device-links/approve"), 2);
    linking.dispatch(LinkDeviceAction::Check).await.unwrap();
    assert!(
        second
            .client
            .servers()
            .iter()
            .all(|handle| { matches!(handle.access().state, AccessState::LoggedIn { .. }) })
    );
}

#[tokio::test(start_paused = true)]
async fn a_request_with_another_key_or_an_unknown_server_is_not_approved() {
    let network = Arc::new(Network::default());
    let (first, servers) = owner_of(&network, &["acme"]).await;
    let second = device(&network);
    let linking = second.client.link_this_device();
    linking
        .dispatch(LinkDeviceAction::Start {
            offer: first.client.link_offer().to_text(),
            device_name: String::new(),
        })
        .await
        .unwrap();
    let genuine = LinkRequest::parse(&links(&linking).1.unwrap()).unwrap();

    let mut swapped = genuine.clone();
    swapped.links[0].public_key = DeviceKey::generate().unwrap().public_key();
    let mut elsewhere = genuine.clone();
    elsewhere.links[0].server_id = ServerId::generate();
    assert_ne!(swapped.fingerprint(), genuine.fingerprint());
    for (request, key_differs) in [(swapped, true), (elsewhere, false)] {
        let approving = first.client.approve_devices();
        let looked_up = approving
            .dispatch(ApproveDevicesAction::LookUp {
                request: request.to_text().unwrap(),
            })
            .await;
        assert_eq!(
            looked_up,
            Err(FlowError::Incomplete {
                failed: 1,
                total: 1
            })
        );
        let expected = if key_differs {
            refused(ErrorCode::PublicKeyMismatch)
        } else {
            FlowError::UnknownServer
        };
        assert_eq!(
            approvals(&approving).0,
            [ApprovalProgress::Failed(expected)]
        );
        assert!(
            approving
                .dispatch(ApproveDevicesAction::Approve)
                .await
                .is_err()
        );
    }
    assert_eq!(servers[0].count("/account/device-links/approve"), 0);
    let owner = &first.client.servers()[0];
    assert_eq!(owner.api().devices().await.unwrap().len(), 1);

    let approving = first.client.approve_devices();
    assert_eq!(
        approving
            .dispatch(ApproveDevicesAction::LookUp {
                request: LinkRequestText::new("junto-link-request-1.e30".to_owned().into())
            })
            .await,
        Err(FlowError::InvalidBundle("link request"))
    );
    assert_eq!(approving.view().state, ApproveDevicesState::EnterRequest);
}

#[tokio::test(start_paused = true)]
async fn an_offer_whose_address_answers_as_another_server_adds_nothing() {
    let network = Arc::new(Network::default());
    let (first, _servers) = owner_of(&network, &["acme"]).await;
    network.host("impostor");
    let mut offer = first.client.link_offer();
    offer.servers[0].address = ServerAddress::parse("impostor.example.test").unwrap();

    let second = device(&network);
    let linking = second.client.link_this_device();
    let started = linking
        .dispatch(LinkDeviceAction::Start {
            offer: offer.to_text(),
            device_name: String::new(),
        })
        .await;
    assert!(matches!(started, Err(FlowError::Incomplete { .. })));
    assert_eq!(
        links(&linking).0,
        [LinkProgress::Failed(FlowError::OtherServer)]
    );
    assert!(second.client.servers().is_empty());
    assert_eq!(
        std::fs::read_dir(second.client.data_dir().keys_dir())
            .unwrap()
            .count(),
        0
    );
    let fresh = second.client.link_this_device();
    assert_eq!(
        fresh
            .dispatch(LinkDeviceAction::Start {
                offer: "not an offer".to_owned(),
                device_name: String::new()
            })
            .await,
        Err(FlowError::InvalidBundle("link offer"))
    );
    assert_eq!(fresh.view().state, LinkDeviceState::EnterOffer);
}

#[tokio::test(start_paused = true)]
async fn a_link_of_all_servers_is_not_approved_for_an_account_that_the_user_did_not_confirm() {
    let network = Arc::new(Network::default());
    let (first, servers) = owner_of(&network, &["acme"]).await;
    let server = &servers[0];
    server.add_invite("CODE", 1);
    let approver = first.client.servers().remove(0);
    let account_a = logged_in(&approver);
    let other = device(&network);
    let member = other.add("acme").await;
    member.dispatch(join("CODE")).await.unwrap();
    let account_b = logged_in(&member);

    let second = device(&network);
    let linking = second.client.link_this_device();
    linking
        .dispatch(LinkDeviceAction::Start {
            offer: first.client.link_offer().to_text(),
            device_name: String::new(),
        })
        .await
        .unwrap();
    let new_key = second.client.servers()[0].device_public_key().unwrap();
    let approving = first.client.approve_devices();
    approving
        .dispatch(ApproveDevicesAction::LookUp {
            request: links(&linking).1.unwrap(),
        })
        .await
        .unwrap();

    server.reassign(approver.device_public_key().unwrap(), account_b);
    for _ in 0..3 {
        assert!(
            approving
                .dispatch(ApproveDevicesAction::Approve)
                .await
                .is_err()
        );
        assert_eq!(server.account_of(new_key), None);
    }
    assert_eq!(server.count("/account/device-links/approve"), 1);
    let shown = |flow: &ApproveDevicesFlow| match flow.view().state {
        ApproveDevicesState::Confirm { mut servers, .. } => servers.remove(0),
        ApproveDevicesState::EnterRequest => panic!("nothing to confirm"),
    };
    assert_eq!(shown(&approving).account_id, Some(account_a));
    assert_eq!(
        shown(&approving).progress,
        ApprovalProgress::AccountChanged { now: account_b }
    );
    assert!(
        approving
            .view()
            .actions
            .contains(&ApproveDevicesActionKind::ConfirmAccount)
    );
    assert_eq!(
        approving
            .dispatch(ApproveDevicesAction::ConfirmAccount {
                server_id: ServerId::generate()
            })
            .await,
        Err(FlowError::NotAvailable)
    );

    approving
        .dispatch(ApproveDevicesAction::ConfirmAccount {
            server_id: server.server_id(),
        })
        .await
        .unwrap();
    assert_eq!(shown(&approving).account_id, Some(account_b));
    assert_eq!(shown(&approving).progress, ApprovalProgress::Ready);
    assert_eq!(server.account_of(new_key), None);
    assert_eq!(server.count("/account/device-links/approve"), 1);
    assert_eq!(
        approving
            .dispatch(ApproveDevicesAction::ConfirmAccount {
                server_id: server.server_id()
            })
            .await,
        Err(FlowError::NotAvailable)
    );
    approving
        .dispatch(ApproveDevicesAction::Approve)
        .await
        .unwrap();
    assert_eq!(server.account_of(new_key), Some(account_b));
}

#[tokio::test(start_paused = true)]
async fn a_server_whose_account_was_not_shown_is_looked_up_first_and_approved_after() {
    let network = Arc::new(Network::default());
    let (first, servers) = owner_of(&network, &["acme"]).await;
    let server = &servers[0];
    let second = device(&network);
    let linking = second.client.link_this_device();
    linking
        .dispatch(LinkDeviceAction::Start {
            offer: first.client.link_offer().to_text(),
            device_name: String::new(),
        })
        .await
        .unwrap();
    let new_key = second.client.servers()[0].device_public_key().unwrap();

    let restarted = first.reopened(&network);
    let approving = restarted.approve_devices();
    server.set_down(true);
    assert!(
        approving
            .dispatch(ApproveDevicesAction::LookUp {
                request: links(&linking).1.unwrap(),
            })
            .await
            .is_err()
    );
    let shown = |flow: &ApproveDevicesFlow| match flow.view().state {
        ApproveDevicesState::Confirm { mut servers, .. } => servers.remove(0),
        ApproveDevicesState::EnterRequest => panic!("nothing to confirm"),
    };
    assert_eq!(shown(&approving).account_id, None);
    assert!(matches!(
        shown(&approving).progress,
        ApprovalProgress::Failed(FlowError::Unreachable(_))
    ));

    server.set_down(false);
    assert_eq!(
        approving.dispatch(ApproveDevicesAction::Approve).await,
        Err(FlowError::Incomplete {
            failed: 1,
            total: 1
        })
    );
    assert_eq!(shown(&approving).progress, ApprovalProgress::Ready);
    assert_eq!(
        shown(&approving).account_id,
        restarted.servers()[0].account()
    );
    assert_eq!(server.account_of(new_key), None);
    approving
        .dispatch(ApproveDevicesAction::Approve)
        .await
        .unwrap();
    assert!(server.account_of(new_key).is_some());
}

async fn until<F: Fn() -> bool>(client: &Client, reached: F) {
    let mut changes = client.changes();
    let waiting = async {
        while !reached() {
            assert!(changes.changed().await);
        }
    };
    tokio::time::timeout(Duration::from_secs(300), waiting)
        .await
        .expect("the state was not reached while a server was suspended");
}

#[tokio::test(start_paused = true)]
async fn a_suspended_server_does_not_hold_up_the_other_servers_of_a_link() {
    let network = Arc::new(Network::default());
    let (first, _servers) = owner_of(&network, &["acme", "globex"]).await;
    let second = device(&network);
    let linking = second.client.link_this_device();

    network.suspend("acme");
    let starting = linking.dispatch(LinkDeviceAction::Start {
        offer: first.client.link_offer().to_text(),
        device_name: String::new(),
    });
    let progress_of = |flow: &LinkDeviceFlow| match flow.view().state {
        LinkDeviceState::Requested { servers, .. } => servers
            .into_iter()
            .map(|server| server.progress)
            .collect::<Vec<_>>(),
        LinkDeviceState::EnterOffer => Vec::new(),
    };
    until(&second.client, || {
        progress_of(&linking).get(1) == Some(&LinkProgress::Waiting)
    })
    .await;
    assert!(linking.view().in_progress.is_some());
    assert_ne!(progress_of(&linking)[0], LinkProgress::Waiting);
    network.resume("acme");
    starting.await.unwrap();
    let request = links(&linking).1.unwrap();

    let approving = first.client.approve_devices();
    network.suspend("acme");
    let looking = approving.dispatch(ApproveDevicesAction::LookUp { request });
    let approvals_of = |flow: &ApproveDevicesFlow| match flow.view().state {
        ApproveDevicesState::Confirm { servers, .. } => servers
            .into_iter()
            .map(|server| server.progress)
            .collect::<Vec<_>>(),
        ApproveDevicesState::EnterRequest => Vec::new(),
    };
    until(&first.client, || {
        approvals_of(&approving).get(1) == Some(&ApprovalProgress::Ready)
    })
    .await;
    assert_ne!(approvals_of(&approving)[0], ApprovalProgress::Ready);
    network.resume("acme");
    looking.await.unwrap();

    network.suspend("acme");
    let approval = approving.dispatch(ApproveDevicesAction::Approve);
    until(&first.client, || {
        approvals_of(&approving).get(1) == Some(&ApprovalProgress::Approved)
    })
    .await;
    assert_ne!(approvals_of(&approving)[0], ApprovalProgress::Approved);
    network.resume("acme");
    approval.await.unwrap();

    network.suspend("acme");
    let checking = linking.dispatch(LinkDeviceAction::Check);
    until(&second.client, || {
        progress_of(&linking).get(1) == Some(&LinkProgress::Linked)
    })
    .await;
    assert_ne!(progress_of(&linking)[0], LinkProgress::Linked);
    network.resume("acme");
    checking.await.unwrap();
    assert_eq!(
        progress_of(&linking),
        [LinkProgress::Linked, LinkProgress::Linked]
    );
}

#[tokio::test(start_paused = true)]
async fn a_server_that_returns_a_huge_link_code_fails_alone() {
    let network = Arc::new(Network::default());
    let (first, servers) = owner_of(&network, &["acme", "globex"]).await;
    servers[0].set_link_code(&"A".repeat(40 * 1024));
    let second = device(&network);
    let linking = second.client.link_this_device();
    let started = linking
        .dispatch(LinkDeviceAction::Start {
            offer: first.client.link_offer().to_text(),
            device_name: String::new(),
        })
        .await;
    assert_eq!(
        started,
        Err(FlowError::Incomplete {
            failed: 1,
            total: 2
        })
    );
    let (progress, request, _) = links(&linking);
    assert_eq!(
        progress,
        [
            LinkProgress::Failed(FlowError::InvalidResponse),
            LinkProgress::Waiting
        ]
    );
    let on_acme = second.client.server(servers[0].server_id()).unwrap();
    assert_eq!(
        on_acme.access().state,
        AccessState::NoAccount { notice: None }
    );
    let request = request.unwrap();
    assert!(request.expose().len() < 1024);

    let approving = first.client.approve_devices();
    approving
        .dispatch(ApproveDevicesAction::LookUp { request })
        .await
        .unwrap();
    approving
        .dispatch(ApproveDevicesAction::Approve)
        .await
        .unwrap();
    assert_eq!(approvals(&approving).0, [ApprovalProgress::Approved]);

    for code in ["", "abcd-efgh", "ABCD EFGH", "ABCD\nEFGH", &"A".repeat(33)] {
        servers[0].set_link_code(code);
        assert_eq!(
            on_acme
                .dispatch(AccessAction::RequestLink {
                    device_name: String::new()
                })
                .await,
            Err(FlowError::InvalidResponse),
            "{code:?}"
        );
    }
    servers[0].set_link_code("ABCD-2345");
    on_acme
        .dispatch(AccessAction::RequestLink {
            device_name: String::new(),
        })
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_change_that_the_server_made_and_answered_unreadably_is_found() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    server.add_invite("CODE", 1);
    let first = device(&network);
    let owner = first.add("acme").await;
    owner.dispatch(claim()).await.unwrap();

    let second = device(&network);
    let member = second.add("acme").await;
    server.fail("/join", [Fault::Garble]);
    member.dispatch(join("CODE")).await.unwrap();
    logged_in(&member);
    assert_eq!(server.account_count(), 2);

    let third = device(&network);
    let newcomer = third.add("acme").await;
    newcomer
        .dispatch(AccessAction::RequestLink {
            device_name: String::new(),
        })
        .await
        .unwrap();
    let AccessState::WaitingForLink {
        code, public_key, ..
    } = newcomer.access().state
    else {
        panic!("not waiting for a link");
    };
    let approval = owner.approve_link();
    approval
        .dispatch(ApproveLinkAction::LookUp { code })
        .await
        .unwrap();
    server.fail("/account/device-links/approve", [Fault::Garble]);
    approval.dispatch(ApproveLinkAction::Approve).await.unwrap();
    let ApproveLinkState::Approved { device } = approval.view().state else {
        panic!("not approved: {:?}", approval.view().state);
    };
    assert_eq!(device.public_key, public_key);
    assert_eq!(server.count("/account/device-links/approve"), 1);
}

#[test]
fn a_save_that_started_earlier_does_not_overwrite_a_later_one() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let network = Arc::new(Network::default());
    network.host("acme");
    let device = runtime.block_on(async {
        let device = device(&network);
        let handle = device.add("acme").await;
        handle.dispatch(claim()).await.unwrap();
        device
    });
    let handle = device.client.servers().remove(0);
    assert!(matches!(
        handle.entry().membership,
        Membership::Member { .. }
    ));

    let (entered, has_entered) = std::sync::mpsc::channel();
    let (resume, resumed) = std::sync::mpsc::channel::<()>();
    let resumed = Mutex::new(resumed);
    let first = AtomicBool::new(true);
    *device.client.inner.core.before_write.lock().unwrap() = Some(Arc::new(move || {
        if first.swap(false, Ordering::SeqCst) {
            entered.send(()).unwrap();
            resumed.lock().unwrap().recv().unwrap();
        }
    }));

    let earlier = {
        let server = handle.server.clone();
        std::thread::spawn(move || {
            server.record_sync(ProfileSync::Applied);
        })
    };
    has_entered.recv().unwrap();
    let later = {
        let server = handle.server.clone();
        std::thread::spawn(move || {
            let removed = Settled {
                state: AccessState::NoAccount {
                    notice: Some(Notice::Removed),
                },
                membership: Membership::Removed,
            };
            server.settle(removed, None).unwrap();
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    resume.send(()).unwrap();
    earlier.join().unwrap();
    later.join().unwrap();

    let stored = ServerBook::new(device.client.data_dir().clone())
        .entry(handle.id())
        .unwrap()
        .unwrap();
    assert_eq!(stored.membership, Membership::Removed);
    assert_eq!(stored.profile_sync, ProfileSync::Applied);
    assert_eq!(handle.entry().membership, Membership::Removed);
}

/// A key store that looks, while a key is deleted, whether another holder can take the
/// lock of the data directory.
struct LockProbe {
    inner: FileKeyStore,
    root: std::path::PathBuf,
    another_holder_got_the_lock: Mutex<Vec<bool>>,
}

impl KeyStore for LockProbe {
    fn load(
        &self,
        server_id: ServerId,
        origin: &ServerAddress,
    ) -> Result<Option<DeviceKey>, KeyStoreError> {
        self.inner.load(server_id, origin)
    }

    fn origin(&self, server_id: ServerId) -> Result<Option<ServerAddress>, KeyStoreError> {
        self.inner.origin(server_id)
    }

    fn insert(
        &self,
        server_id: ServerId,
        origin: &ServerAddress,
        key: &DeviceKey,
    ) -> Result<(), KeyStoreError> {
        self.inner.insert(server_id, origin, key)
    }

    fn remove(&self, server_id: ServerId) -> Result<(), KeyStoreError> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let root = self.root.clone();
        std::thread::spawn(move || {
            let other = DataDir::open(&root).unwrap();
            let _lock = other.lock().unwrap();
            let _ = sender.send(());
        });
        let got_it = receiver.recv_timeout(Duration::from_millis(300)).is_ok();
        self.another_holder_got_the_lock
            .lock()
            .unwrap()
            .push(got_it);
        self.inner.remove(server_id)
    }
}

#[tokio::test(start_paused = true)]
async fn the_lock_is_held_from_the_removal_of_an_entry_to_the_deletion_of_its_key() {
    let network = Arc::new(Network::default());
    network.host("acme");
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("junto");
    let probe = Arc::new(LockProbe {
        inner: FileKeyStore::new(DataDir::open(&root).unwrap()),
        root: root.clone(),
        another_holder_got_the_lock: Mutex::default(),
    });
    let mut options = ClientOptions::new(&root);
    options.transport = Some(network.clone());
    options.key_store = Some(probe.clone());
    let client = Client::open(options).unwrap();
    let handle = add(&client, "acme").await;
    handle.dispatch(claim()).await.unwrap();

    client.remove_server(handle.id()).unwrap();
    assert_eq!(*probe.another_holder_got_the_lock.lock().unwrap(), [false]);
    assert!(!client.data_dir().key_file(handle.id()).exists());
    assert!(client.servers().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_recovery_notice_is_kept_until_it_is_taken_also_across_later_logins() {
    let network = Arc::new(Network::default());
    let server = network.host("acme");
    let device = device(&network);
    let handle = device.add("acme").await;
    handle.dispatch(claim()).await.unwrap();
    let account_id = logged_in(&handle);
    assert!(handle.take_recoveries().is_empty());

    server.recover(account_id);
    handle.api().own_account().await.unwrap();
    let AccessState::LoggedIn { recoveries, .. } = handle.access().state else {
        panic!("not logged in");
    };
    assert_eq!(recoveries.len(), 1);

    server.end_sessions();
    handle.api().own_account().await.unwrap();
    handle.dispatch(AccessAction::LogOut).await.unwrap();
    handle.dispatch(AccessAction::LogIn).await.unwrap();
    let AccessState::LoggedIn {
        recoveries: latest, ..
    } = handle.access().state
    else {
        panic!("not logged in");
    };
    assert!(latest.is_empty());

    server.recover(account_id);
    handle.api().own_account().await.unwrap();
    let taken = handle.take_recoveries();
    assert_eq!(taken.len(), 2);
    assert_eq!(taken[0].id, recoveries[0].id);
    assert_ne!(taken[1].id, taken[0].id);
    assert!(handle.take_recoveries().is_empty());
}
