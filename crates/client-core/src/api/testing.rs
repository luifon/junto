//! A server in memory for the tests of the flows. It follows the rules of the real server
//! for challenges, signatures, sessions, joining, join requests, device links, bans and
//! removals, and it can be told to fail.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use junto_protocol::account::{
    ApproveDeviceLink, Device, DeviceLinkCreated, DeviceOrigin, LookUpDeviceLink, OwnAccount,
    PendingDeviceLink, RequestDeviceLink,
};
use junto_protocol::auth::{
    Challenge, ChallengeOperation, ClaimOwner, CreateChallenge, Login, Proof, SessionCreated,
};
use junto_protocol::ids::{AccountId, ChallengeId, DeviceId, JoinRequestId, ServerId};
use junto_protocol::invite::{
    Join, JoinRequest, JoinRequestState, JoinRequestStatus, JoinRequestStatusQuery, RequestToJoin,
};
use junto_protocol::keys::{Nonce, PublicKey};
use junto_protocol::secret::{LinkCode, SessionToken};
use junto_protocol::server::ServerInfo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use time::OffsetDateTime;

use super::transport::{
    HttpRequest, HttpResponse, Method, Transport, TransportError, TransportErrorKind,
    TransportFuture,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Answers `busy` without doing the work.
    Busy,
    /// Does the work and loses the answer.
    LoseAnswer,
    /// Loses the request before any work.
    LoseRequest,
    /// Does the work and answers with the right status and a body that is not JSON.
    Garble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Active,
    Banned,
    Removed,
}

struct Account {
    name: String,
    status: Status,
}

struct DeviceRow {
    id: DeviceId,
    account_id: AccountId,
    public_key: PublicKey,
    name: String,
    origin: DeviceOrigin,
}

struct Request {
    id: JoinRequestId,
    public_key: PublicKey,
    name: String,
    state: JoinRequestState,
    account_id: Option<AccountId>,
}

struct World {
    info: ServerInfo,
    owner_code: Option<String>,
    challenges: BTreeMap<ChallengeId, Challenge>,
    accounts: BTreeMap<AccountId, Account>,
    devices: Vec<DeviceRow>,
    sessions: BTreeMap<String, DeviceId>,
    invites: BTreeMap<String, u32>,
    requests: Vec<Request>,
    links: BTreeMap<String, (PublicKey, String)>,
    faults: Vec<(String, VecDeque<Fault>)>,
    down: bool,
    seen: Vec<String>,
    next_token: u32,
    challenge_server_id: Option<ServerId>,
    link_code: Option<String>,
    notices: Vec<(DeviceId, junto_protocol::account::RecoveryNotice)>,
}

type Reply = Result<(u16, Vec<u8>), &'static str>;

/// A refusal, with the challenge that it puts back.
type Refusal = (Option<Box<Challenge>>, &'static str);

pub(crate) struct FakeServer {
    world: Mutex<World>,
}

impl FakeServer {
    pub(crate) fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            world: Mutex::new(World {
                info: ServerInfo {
                    server_id: ServerId::generate(),
                    name: name.to_owned(),
                    logo_path: Some("/api/v1/logo".to_owned()),
                    api_versions: vec![1],
                },
                owner_code: Some("OWNER-CODE".to_owned()),
                challenges: BTreeMap::new(),
                accounts: BTreeMap::new(),
                devices: Vec::new(),
                sessions: BTreeMap::new(),
                invites: BTreeMap::new(),
                requests: Vec::new(),
                links: BTreeMap::new(),
                faults: Vec::new(),
                down: false,
                seen: Vec::new(),
                next_token: 0,
                challenge_server_id: None,
                link_code: None,
                notices: Vec::new(),
            }),
        })
    }

    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    pub(crate) fn server_id(&self) -> ServerId {
        self.world().info.server_id
    }

    /// Makes the challenges name another server than the server information does.
    pub(crate) fn set_challenge_server_id(&self, server_id: ServerId) {
        self.world().challenge_server_id = Some(server_id);
    }

    /// Makes the server answer every link request with this text as the code.
    pub(crate) fn set_link_code(&self, code: &str) {
        self.world().link_code = Some(code.to_owned());
    }

    pub(crate) fn set_info(&self, change: impl FnOnce(&mut ServerInfo)) {
        change(&mut self.world().info);
    }

    pub(crate) fn set_down(&self, down: bool) {
        self.world().down = down;
    }

    /// The next requests whose path ends with `path` fail in the given ways, in order.
    pub(crate) fn fail(&self, path: &str, faults: impl IntoIterator<Item = Fault>) {
        self.world()
            .faults
            .push((path.to_owned(), faults.into_iter().collect()));
    }

    pub(crate) fn add_invite(&self, code: &str, uses: u32) {
        self.world().invites.insert(code.to_owned(), uses);
    }

    pub(crate) fn ban(&self, account_id: AccountId) {
        let mut world = self.world();
        world.accounts.get_mut(&account_id).unwrap().status = Status::Banned;
        world.end_sessions_of(account_id);
    }

    pub(crate) fn unban(&self, account_id: AccountId) {
        self.world().accounts.get_mut(&account_id).unwrap().status = Status::Active;
    }

    pub(crate) fn remove(&self, account_id: AccountId) {
        let mut world = self.world();
        world.accounts.get_mut(&account_id).unwrap().status = Status::Removed;
        world.end_sessions_of(account_id);
        world
            .devices
            .retain(|device| device.account_id != account_id);
    }

    /// Takes the device with `public_key` out of its account and gives it to `account_id`,
    /// as a removal of the device followed by a recovery into another account does.
    pub(crate) fn reassign(&self, public_key: PublicKey, account_id: AccountId) {
        let mut world = self.world();
        let moved: Vec<DeviceId> = world
            .devices
            .iter()
            .filter(|device| device.public_key == public_key)
            .map(|device| device.id)
            .collect();
        world
            .sessions
            .retain(|_, device_id| !moved.contains(device_id));
        world
            .devices
            .retain(|device| device.public_key != public_key);
        world.add_device(account_id, public_key, "", DeviceOrigin::Recovery);
    }

    /// Ends every session of the account and leaves one recovery notice for each of its
    /// devices, as a recovery of the account does.
    pub(crate) fn recover(&self, account_id: AccountId) {
        let mut world = self.world();
        world.end_sessions_of(account_id);
        let notice = junto_protocol::account::RecoveryNotice {
            id: junto_protocol::ids::RecoveryId::generate(),
            device_id: None,
            performed_by: None,
            performed_at: OffsetDateTime::now_utc().replace_nanosecond(0).unwrap(),
        };
        let devices: Vec<DeviceId> = world
            .devices
            .iter()
            .filter(|device| device.account_id == account_id)
            .map(|device| device.id)
            .collect();
        for device_id in devices {
            world.notices.push((device_id, notice.clone()));
        }
    }

    pub(crate) fn end_sessions(&self) {
        self.world().sessions.clear();
    }

    pub(crate) fn decide_request(&self, public_key: PublicKey, approve: bool) {
        let mut world = self.world();
        let index = world
            .requests
            .iter()
            .rposition(|request| request.public_key == public_key)
            .unwrap();
        if approve {
            let name = world.requests[index].name.clone();
            let account_id = world.create_account(&name, public_key, "", DeviceOrigin::JoinRequest);
            world.requests[index].state = JoinRequestState::Approved;
            world.requests[index].account_id = Some(account_id);
        } else {
            world.requests[index].state = JoinRequestState::Denied;
        }
    }

    pub(crate) fn expire_request(&self, public_key: PublicKey) {
        let mut world = self.world();
        let request = world
            .requests
            .iter_mut()
            .rfind(|request| request.public_key == public_key)
            .unwrap();
        request.state = JoinRequestState::Expired;
    }

    pub(crate) fn account_of(&self, public_key: PublicKey) -> Option<AccountId> {
        self.world()
            .devices
            .iter()
            .find(|device| device.public_key == public_key)
            .map(|device| device.account_id)
    }

    pub(crate) fn account_count(&self) -> usize {
        self.world().accounts.len()
    }

    pub(crate) fn session_count(&self) -> usize {
        self.world().sessions.len()
    }

    /// The method and path of every request that arrived, in order.
    pub(crate) fn seen(&self) -> Vec<String> {
        self.world().seen.clone()
    }

    pub(crate) fn count(&self, path_end: &str) -> usize {
        self.seen()
            .iter()
            .filter(|line| line.ends_with(path_end))
            .count()
    }
}

fn json<T: Serialize>(status: u16, body: &T) -> Reply {
    Ok((status, serde_json::to_vec(body).unwrap()))
}

fn read<T: DeserializeOwned>(request: &HttpRequest) -> Result<T, &'static str> {
    serde_json::from_slice(
        request
            .body
            .as_ref()
            .map(|body| body.as_slice())
            .unwrap_or_default(),
    )
    .map_err(|_| "invalid_request")
}

fn status_of(code: &str) -> u16 {
    let parsed: junto_protocol::error::ErrorCode =
        serde_json::from_value(code.into()).expect("a known error code");
    parsed.http_status()
}

fn refusal(code: &str) -> HttpResponse {
    HttpResponse {
        status: status_of(code),
        retry_after: matches!(code, "busy" | "timeout").then_some(Duration::from_secs(1)),
        body: format!(r#"{{"error":{{"code":"{code}","message":"a sentence"}}}}"#)
            .into_bytes()
            .into(),
    }
}

impl World {
    fn end_sessions_of(&mut self, account_id: AccountId) {
        let devices: Vec<DeviceId> = self
            .devices
            .iter()
            .filter(|device| device.account_id == account_id)
            .map(|device| device.id)
            .collect();
        self.sessions
            .retain(|_, device_id| !devices.contains(device_id));
    }

    fn create_account(
        &mut self,
        name: &str,
        public_key: PublicKey,
        device_name: &str,
        origin: DeviceOrigin,
    ) -> AccountId {
        let account_id = AccountId::generate();
        self.accounts.insert(
            account_id,
            Account {
                name: name.to_owned(),
                status: Status::Active,
            },
        );
        self.add_device(account_id, public_key, device_name, origin);
        account_id
    }

    fn add_device(
        &mut self,
        account_id: AccountId,
        public_key: PublicKey,
        name: &str,
        origin: DeviceOrigin,
    ) -> DeviceId {
        let id = DeviceId::generate();
        self.devices.push(DeviceRow {
            id,
            account_id,
            public_key,
            name: name.to_owned(),
            origin,
        });
        id
    }

    fn device_of_key(&self, public_key: PublicKey) -> Option<&DeviceRow> {
        self.devices
            .iter()
            .find(|device| device.public_key == public_key)
    }

    fn start_session(&mut self, public_key: PublicKey) -> SessionCreated {
        let device = self.device_of_key(public_key).unwrap();
        let (device_id, account_id) = (device.id, device.account_id);
        self.next_token += 1;
        let token = format!("jst_fake-{}", self.next_token);
        self.sessions.insert(token.clone(), device_id);
        let mut recoveries = Vec::new();
        self.notices.retain(|(device, notice)| {
            if *device == device_id {
                recoveries.push(notice.clone());
            }
            *device != device_id
        });
        SessionCreated {
            token: SessionToken::new(token),
            expires_at: OffsetDateTime::now_utc() + time::Duration::days(30),
            account_id,
            device_id,
            recoveries,
        }
    }

    /// Checks a proof like the real server and uses the challenge up. An operation that
    /// then refuses puts the challenge back, as a transaction that does not commit does.
    fn consume(
        &mut self,
        proof: &Proof,
        operation: ChallengeOperation,
    ) -> Result<Challenge, &'static str> {
        let challenge = self
            .challenges
            .get(&proof.challenge_id)
            .cloned()
            .ok_or("challenge_invalid")?;
        if challenge.operation != operation {
            return Err("challenge_invalid");
        }
        let key = VerifyingKey::from_bytes(challenge.public_key.as_bytes())
            .map_err(|_| "invalid_signature")?;
        let signature = ed25519_dalek::Signature::from_bytes(proof.signature.as_bytes());
        key.verify_strict(&challenge.signing_input(), &signature)
            .map_err(|_| "invalid_signature")?;
        self.challenges.remove(&proof.challenge_id);
        Ok(challenge)
    }

    fn session_device(&self, request: &HttpRequest) -> Result<&DeviceRow, &'static str> {
        let token = request.bearer.as_ref().ok_or("unauthenticated")?;
        let device_id = self.sessions.get(token.expose()).ok_or("unauthenticated")?;
        let device = self
            .devices
            .iter()
            .find(|device| device.id == *device_id)
            .ok_or("unauthenticated")?;
        match self.accounts[&device.account_id].status {
            Status::Active => Ok(device),
            Status::Banned | Status::Removed => Err("unauthenticated"),
        }
    }

    fn handle(&mut self, request: &HttpRequest) -> Reply {
        let path = request.url.path().to_owned();
        if (request.method, path.as_str()) == (Method::Get, "/api/server") {
            return json(200, &self.info);
        }
        let route = self
            .info
            .api_versions
            .iter()
            .find_map(|version| path.strip_prefix(&format!("/api/v{version}")))
            .ok_or("not_found")?
            .to_owned();
        let outcome = self.route(request, &route);
        if let Err((Some(challenge), _)) = &outcome {
            self.challenges
                .insert(challenge.id, challenge.as_ref().clone());
        }
        outcome.map_err(|(_, code)| code)
    }

    fn route(&mut self, request: &HttpRequest, route: &str) -> Result<(u16, Vec<u8>), Refusal> {
        let plain = |code: &'static str| (None, code);
        match (request.method, route) {
            (Method::Post, "/auth/challenges") => {
                let body: CreateChallenge = read(request).map_err(plain)?;
                let account_id = match body.operation {
                    ChallengeOperation::Login => {
                        let device = self
                            .device_of_key(body.public_key)
                            .ok_or(plain("unknown_device"))?;
                        match self.accounts[&device.account_id].status {
                            Status::Active => Some(device.account_id),
                            Status::Banned => return Err(plain("account_banned")),
                            Status::Removed => return Err(plain("unknown_device")),
                        }
                    }
                    _ => None,
                };
                let challenge = Challenge {
                    id: ChallengeId::generate(),
                    server_id: self.challenge_server_id.unwrap_or(self.info.server_id),
                    operation: body.operation,
                    account_id,
                    public_key: body.public_key,
                    nonce: Nonce::from_bytes([9; 32]),
                    expires_at: OffsetDateTime::now_utc().replace_nanosecond(0).unwrap()
                        + time::Duration::seconds(60),
                };
                self.challenges.retain(|_, kept| {
                    !(kept.public_key == body.public_key && kept.operation == body.operation)
                });
                self.challenges.insert(challenge.id, challenge.clone());
                json(201, &challenge).map_err(plain)
            }
            (Method::Post, "/auth/sessions") => {
                let body: Login = read(request).map_err(plain)?;
                let challenge = self
                    .consume(&body.proof, ChallengeOperation::Login)
                    .map_err(plain)?;
                json(201, &self.start_session(challenge.public_key)).map_err(plain)
            }
            (Method::Delete, "/auth/session") => {
                self.session_device(request).map_err(plain)?;
                if let Some(token) = &request.bearer {
                    self.sessions.remove(token.expose());
                }
                Ok((204, Vec::new()))
            }
            (Method::Post, "/owner/claim") => {
                let body: ClaimOwner = read(request).map_err(plain)?;
                let challenge = self
                    .consume(&body.proof, ChallengeOperation::ClaimOwner)
                    .map_err(plain)?;
                let refuse = |code| Err((Some(Box::new(challenge.clone())), code));
                match &self.owner_code {
                    None => return refuse("owner_already_claimed"),
                    Some(code) if code != body.owner_code.expose() => {
                        return refuse("owner_code_invalid");
                    }
                    Some(_) => {}
                }
                if self.device_of_key(challenge.public_key).is_some() {
                    return refuse("key_already_registered");
                }
                self.owner_code = None;
                self.create_account(
                    &body.name,
                    challenge.public_key,
                    &body.device_name,
                    DeviceOrigin::OwnerCode,
                );
                json(201, &self.start_session(challenge.public_key)).map_err(plain)
            }
            (Method::Post, "/join") => {
                let body: Join = read(request).map_err(plain)?;
                let challenge = self
                    .consume(&body.proof, ChallengeOperation::Join)
                    .map_err(plain)?;
                let refuse = |code| Err((Some(Box::new(challenge.clone())), code));
                if self.device_of_key(challenge.public_key).is_some() {
                    return refuse("key_already_registered");
                }
                match self.invites.get_mut(body.invite_code.expose()) {
                    None => return refuse("invite_not_found"),
                    Some(0) => return refuse("invite_exhausted"),
                    Some(uses) => *uses -= 1,
                }
                self.create_account(
                    &body.name,
                    challenge.public_key,
                    &body.device_name,
                    DeviceOrigin::Invite,
                );
                json(201, &self.start_session(challenge.public_key)).map_err(plain)
            }
            (Method::Post, "/join-requests") => {
                let body: RequestToJoin = read(request).map_err(plain)?;
                let challenge = self
                    .consume(&body.proof, ChallengeOperation::RequestToJoin)
                    .map_err(plain)?;
                let refuse = |code| Err((Some(Box::new(challenge.clone())), code));
                if self.device_of_key(challenge.public_key).is_some() {
                    return refuse("key_already_registered");
                }
                let pending = self.requests.iter().any(|request| {
                    request.public_key == challenge.public_key
                        && request.state == JoinRequestState::Pending
                });
                if pending {
                    return refuse("join_request_pending");
                }
                let id = JoinRequestId::generate();
                self.requests.push(Request {
                    id,
                    public_key: challenge.public_key,
                    name: body.name.clone(),
                    state: JoinRequestState::Pending,
                    account_id: None,
                });
                let now = OffsetDateTime::now_utc();
                let created = JoinRequest {
                    id,
                    name: body.name,
                    created_at: now,
                    expires_at: now + time::Duration::days(7),
                };
                json(201, &created).map_err(plain)
            }
            (Method::Post, "/join-requests/status") => {
                let body: JoinRequestStatusQuery = read(request).map_err(plain)?;
                let challenge = self
                    .consume(&body.proof, ChallengeOperation::JoinRequestStatus)
                    .map_err(plain)?;
                let Some(found) = self
                    .requests
                    .iter()
                    .rfind(|request| request.public_key == challenge.public_key)
                else {
                    return Err((Some(Box::new(challenge)), "not_found"));
                };
                let status = JoinRequestStatus {
                    id: found.id,
                    state: found.state,
                    account_id: found.account_id,
                };
                json(200, &status).map_err(plain)
            }
            (Method::Post, "/device-links") => {
                let body: RequestDeviceLink = read(request).map_err(plain)?;
                let challenge = self
                    .consume(&body.proof, ChallengeOperation::LinkDevice)
                    .map_err(plain)?;
                if self.device_of_key(challenge.public_key).is_some() {
                    return Err((Some(Box::new(challenge)), "key_already_registered"));
                }
                self.links
                    .retain(|_, (key, _)| *key != challenge.public_key);
                self.next_token += 1;
                let code = match &self.link_code {
                    Some(code) => code.clone(),
                    None => format!("LINK-{:04}", self.next_token),
                };
                self.links
                    .insert(code.clone(), (challenge.public_key, body.device_name));
                let created = DeviceLinkCreated {
                    code: LinkCode::new(code),
                    expires_at: OffsetDateTime::now_utc() + time::Duration::minutes(10),
                };
                json(201, &created).map_err(plain)
            }
            (Method::Get, "/account") => {
                let account_id = self.session_device(request).map_err(plain)?.account_id;
                let account = OwnAccount {
                    id: account_id,
                    name: self.accounts[&account_id].name.clone(),
                    is_owner: false,
                    server_role_ids: Vec::new(),
                    server_permissions: Vec::new(),
                    workspace_ids: Vec::new(),
                };
                json(200, &account).map_err(plain)
            }
            (Method::Get, "/account/devices") => {
                let current = self.session_device(request).map_err(plain)?;
                let (current_id, account_id) = (current.id, current.account_id);
                let devices: Vec<Device> = self
                    .devices
                    .iter()
                    .filter(|device| device.account_id == account_id)
                    .map(|device| device.shown(current_id))
                    .collect();
                json(200, &devices).map_err(plain)
            }
            (Method::Post, "/account/device-links/lookup") => {
                self.session_device(request).map_err(plain)?;
                let body: LookUpDeviceLink = read(request).map_err(plain)?;
                let (public_key, device_name) = self
                    .links
                    .get(body.code.expose())
                    .cloned()
                    .ok_or(plain("link_code_invalid"))?;
                let pending = PendingDeviceLink {
                    public_key,
                    device_name,
                    expires_at: OffsetDateTime::now_utc() + time::Duration::minutes(10),
                };
                json(200, &pending).map_err(plain)
            }
            (Method::Post, "/account/device-links/approve") => {
                let approver = self.session_device(request).map_err(plain)?;
                let (approver_id, account_id) = (approver.id, approver.account_id);
                let body: ApproveDeviceLink = read(request).map_err(plain)?;
                let (public_key, device_name) = self
                    .links
                    .get(body.code.expose())
                    .cloned()
                    .ok_or(plain("link_code_invalid"))?;
                if public_key != body.public_key {
                    return Err(plain("public_key_mismatch"));
                }
                self.links.remove(body.code.expose());
                let id = self.add_device(account_id, public_key, &device_name, DeviceOrigin::Link);
                let shown = self
                    .devices
                    .iter()
                    .find(|device| device.id == id)
                    .map(|device| device.shown(approver_id));
                json(201, &shown).map_err(plain)
            }
            _ => Err(plain("not_found")),
        }
    }
}

impl DeviceRow {
    fn shown(&self, current: DeviceId) -> Device {
        Device {
            id: self.id,
            name: self.name.clone(),
            public_key: self.public_key,
            origin: self.origin,
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_login_at: None,
            current: self.id == current,
        }
    }
}

impl Transport for FakeServer {
    fn send(&self, request: HttpRequest) -> TransportFuture {
        let mut world = self.world();
        let path = request.url.path().to_owned();
        let lost = |kind| Err(TransportError::new(kind, "test"));
        let result = if world.down {
            lost(TransportErrorKind::Connect)
        } else {
            world.seen.push(format!("{:?} {path}", request.method));
            let fault = world
                .faults
                .iter_mut()
                .find(|(end, queue)| path.ends_with(end.as_str()) && !queue.is_empty())
                .and_then(|(_, queue)| queue.pop_front());
            match fault {
                Some(Fault::Busy) => Ok(refusal("busy")),
                Some(Fault::LoseRequest) => lost(TransportErrorKind::Interrupted),
                Some(Fault::LoseAnswer) => {
                    let _ = world.handle(&request);
                    lost(TransportErrorKind::Timeout)
                }
                Some(Fault::Garble) => Ok(match world.handle(&request) {
                    Ok((status, _)) => HttpResponse {
                        status,
                        retry_after: None,
                        body: b"<html>ok</html>".to_vec().into(),
                    },
                    Err(code) => refusal(code),
                }),
                None => Ok(match world.handle(&request) {
                    Ok((status, body)) => HttpResponse {
                        status,
                        retry_after: None,
                        body: body.into(),
                    },
                    Err(code) => refusal(code),
                }),
            }
        };
        Box::pin(async move { result })
    }
}
