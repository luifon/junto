//! A server in memory for the tests of the flows. It follows the rules of the real server
//! for challenges, signatures over the origin, sessions, joining, join requests, device
//! links, bans, removals, recovery notices and profiles, and it can be told to fail.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use junto_protocol::account::{
    AccountProfile, ApproveDeviceLink, Avatar, Device, DeviceLinkCreated, DeviceOrigin,
    LookUpDeviceLink, OwnAccount, PendingDeviceLink, RecoveryNotice, RequestDeviceLink,
    UpdateProfile,
};
use junto_protocol::auth::{
    Challenge, ChallengeOperation, ClaimOwner, CreateChallenge, Login, Proof, SessionCreated,
};
use junto_protocol::ids::{AccountId, ChallengeId, DeviceId, JoinRequestId, RecoveryId, ServerId};
use junto_protocol::invite::{
    Join, JoinRequest, JoinRequestState, JoinRequestStatus, JoinRequestStatusQuery, RequestToJoin,
};
use junto_protocol::keys::{Nonce, PublicKey};
use junto_protocol::origin::Origin;
use junto_protocol::secret::{LinkCode, SessionToken};
use junto_protocol::server::ServerInfo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use time::OffsetDateTime;

use super::transport::{
    HttpRequest, HttpResponse, Method, Transport, TransportError, TransportErrorKind,
    TransportFuture,
};
use crate::profile::{InvalidAvatar, avatar_hash, check_avatar};

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
    avatar: Option<Vec<u8>>,
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
    owner: Option<AccountId>,
    /// The notices that devices acknowledged, kept for logins that tell them again.
    acknowledged: Vec<(DeviceId, RecoveryNotice)>,
    stale_logins: bool,
    challenges: BTreeMap<ChallengeId, Challenge>,
    accounts: BTreeMap<AccountId, Account>,
    devices: Vec<DeviceRow>,
    sessions: BTreeMap<String, DeviceId>,
    invites: BTreeMap<String, u32>,
    requests: Vec<Request>,
    links: BTreeMap<String, (PublicKey, String)>,
    faults: Vec<(String, VecDeque<Fault>)>,
    /// Refusals with a status and a code of the test, by the end of the path.
    refusals: Vec<(String, u16, String)>,
    down: bool,
    seen: Vec<String>,
    next_token: u32,
    challenge_server_id: Option<ServerId>,
    /// Added to the expiry of each challenge: a negative time makes a challenge that the
    /// clock of the device sees as expired.
    challenge_expiry_shift: time::Duration,
    /// Each session of a login names this account in place of the account of the key.
    session_account: Option<AccountId>,
    link_code: Option<String>,
    notices: Vec<(DeviceId, RecoveryNotice)>,
    /// The public origins. Without a list the server accepts the origin of the address
    /// that a request was sent to.
    public_origins: Option<Vec<Origin>>,
    avatar_refusal: Option<&'static str>,
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
                owner: None,
                acknowledged: Vec::new(),
                stale_logins: false,
                challenges: BTreeMap::new(),
                accounts: BTreeMap::new(),
                devices: Vec::new(),
                sessions: BTreeMap::new(),
                invites: BTreeMap::new(),
                requests: Vec::new(),
                links: BTreeMap::new(),
                faults: Vec::new(),
                refusals: Vec::new(),
                down: false,
                seen: Vec::new(),
                next_token: 0,
                challenge_server_id: None,
                challenge_expiry_shift: time::Duration::ZERO,
                session_account: None,
                link_code: None,
                notices: Vec::new(),
                public_origins: None,
                avatar_refusal: None,
            }),
        })
    }

    fn world(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    pub(crate) fn server_id(&self) -> ServerId {
        self.world().info.server_id
    }

    /// Moves the expiry of the challenges by this time from the clock of the test.
    pub(crate) fn shift_challenge_expiry(&self, shift: time::Duration) {
        self.world().challenge_expiry_shift = shift;
    }

    /// Makes the sessions of logins name this account.
    pub(crate) fn set_session_account(&self, account_id: AccountId) {
        self.world().session_account = Some(account_id);
    }

    /// Makes the challenges name another server than the server information does.
    pub(crate) fn set_challenge_server_id(&self, server_id: ServerId) {
        self.world().challenge_server_id = Some(server_id);
    }

    /// Makes the server answer every link request with this text as the code.
    pub(crate) fn set_link_code(&self, code: &str) {
        self.world().link_code = Some(code.to_owned());
    }

    /// States the public origins of the server, as `JUNTO_PUBLIC_ORIGINS` does. A proof for
    /// another origin is refused.
    pub(crate) fn set_public_origins(&self, origins: &[&str]) {
        let origins = origins
            .iter()
            .map(|origin| Origin::parse(origin).unwrap())
            .collect();
        self.world().public_origins = Some(origins);
    }

    /// Stores a new owner code, as the host command does. A claim with it returns the
    /// owner to the existing owner account on a new device, which is a recovery.
    pub(crate) fn issue_owner_code(&self, code: &str) {
        self.world().owner_code = Some(code.to_owned());
    }

    /// Makes every login tell the notices that its device has acknowledged too, as a login
    /// does whose answer was computed before the acknowledgement.
    pub(crate) fn set_stale_logins(&self, stale: bool) {
        self.world().stale_logins = stale;
    }

    /// Makes the server refuse every avatar with this code, as it does for an image with
    /// too many pixels.
    pub(crate) fn refuse_avatars(&self, code: Option<&'static str>) {
        self.world().avatar_refusal = code;
    }

    /// The stored avatar of an account.
    pub(crate) fn avatar_of(&self, account_id: AccountId) -> Option<Vec<u8>> {
        self.world().accounts[&account_id].avatar.clone()
    }

    pub(crate) fn name_of(&self, account_id: AccountId) -> String {
        self.world().accounts[&account_id].name.clone()
    }

    /// The number of challenges that wait for an answer.
    pub(crate) fn challenge_count(&self) -> usize {
        self.world().challenges.len()
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

    /// The next request whose path ends with `path` is refused with this status and this
    /// code, which can be one that the client does not know. No work is done.
    pub(crate) fn refuse_next(&self, path: &str, status: u16, code: &str) {
        self.world()
            .refusals
            .push((path.to_owned(), status, code.to_owned()));
    }

    /// Stores bytes as the avatar of an account without any check, as a server that does
    /// not follow the rules could.
    pub(crate) fn store_avatar(&self, account_id: AccountId, image: Vec<u8>) {
        if let Some(account) = self.world().accounts.get_mut(&account_id) {
            account.avatar = Some(image);
        }
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

    /// Removes one device from its account and ends its sessions. The account stays.
    pub(crate) fn remove_device(&self, public_key: PublicKey) {
        let mut world = self.world();
        let removed: Vec<DeviceId> = world
            .devices
            .iter()
            .filter(|device| device.public_key == public_key)
            .map(|device| device.id)
            .collect();
        world
            .sessions
            .retain(|_, device_id| !removed.contains(device_id));
        world
            .devices
            .retain(|device| device.public_key != public_key);
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
    pub(crate) fn recover(&self, account_id: AccountId) -> RecoveryId {
        self.world().recover(account_id, RecoveryId::generate())
    }

    /// Like [`Self::recover`], with an identifier that the test chooses: a server can
    /// state any identifier.
    pub(crate) fn recover_with(&self, account_id: AccountId, recovery_id: RecoveryId) {
        self.world().recover(account_id, recovery_id);
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
        receipt: crate::api::Receipt::NotStated,
    }
}

impl World {
    fn recover(&mut self, account_id: AccountId, recovery_id: RecoveryId) -> RecoveryId {
        let world = self;
        world.end_sessions_of(account_id);
        let notice = RecoveryNotice {
            id: recovery_id,
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
        notice.id
    }

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
                avatar: None,
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
        let mut recoveries = self.notices_of(device_id);
        if self.stale_logins {
            let acknowledged = self.acknowledged.iter();
            recoveries.extend(
                acknowledged
                    .filter(|(device, _)| *device == device_id)
                    .map(|(_, notice)| notice.clone()),
            );
        }
        SessionCreated {
            token: SessionToken::new(token),
            expires_at: OffsetDateTime::now_utc() + time::Duration::days(30),
            account_id,
            device_id,
            recoveries,
        }
    }

    /// The recoveries that a device has not acknowledged, oldest first. A login does not
    /// remove them.
    fn notices_of(&self, device_id: DeviceId) -> Vec<RecoveryNotice> {
        self.notices
            .iter()
            .filter(|(device, _)| *device == device_id)
            .map(|(_, notice)| notice.clone())
            .collect()
    }

    fn avatar_description(&self, account_id: AccountId) -> Option<Avatar> {
        let image = self.accounts[&account_id].avatar.as_ref()?;
        Some(Avatar {
            hash: avatar_hash(image),
            media_type: check_avatar(image).ok()?,
            bytes: image.len() as u32,
            width: 1,
            height: 1,
        })
    }

    /// Checks a proof like the real server and uses the challenge up. An operation that
    /// then refuses puts the challenge back, as a transaction that does not commit does.
    /// The origin is checked before the challenge is read, so a proof for another origin
    /// leaves the challenge unused. `reached` is the origin that the request was sent to.
    fn consume(
        &mut self,
        proof: &Proof,
        operation: ChallengeOperation,
        reached: &Origin,
    ) -> Result<Challenge, &'static str> {
        let accepted = match &self.public_origins {
            Some(origins) => origins.contains(&proof.origin),
            None => proof.origin == *reached,
        };
        if !accepted {
            return Err("origin_rejected");
        }
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
        key.verify_strict(&challenge.signing_input(&proof.origin), &signature)
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
        let reached = Origin::parse(&request.url.origin().ascii_serialization())
            .map_err(|_| plain("invalid_request"))?;
        let reached = &reached;
        if let Some(account) = route.strip_prefix("/accounts/") {
            return self.account_route(request, account).map_err(plain);
        }
        if let Some(recovery) = route.strip_prefix("/account/recovery-notices/") {
            let device_id = self.session_device(request).map_err(plain)?.id;
            if request.method != Method::Delete {
                return Err(plain("method_not_allowed"));
            }
            let recovery_id: RecoveryId = recovery.parse().map_err(|_| plain("not_found"))?;
            let Some(index) = self
                .notices
                .iter()
                .position(|(device, notice)| *device == device_id && notice.id == recovery_id)
            else {
                return Err(plain("not_found"));
            };
            let acknowledged = self.notices.remove(index);
            self.acknowledged.push(acknowledged);
            return Ok((204, Vec::new()));
        }
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
                        + time::Duration::seconds(60)
                        + self.challenge_expiry_shift,
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
                    .consume(&body.proof, ChallengeOperation::Login, reached)
                    .map_err(plain)?;
                let mut session = self.start_session(challenge.public_key);
                if let Some(account_id) = self.session_account {
                    session.account_id = account_id;
                }
                json(201, &session).map_err(plain)
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
                    .consume(&body.proof, ChallengeOperation::ClaimOwner, reached)
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
                match self.owner {
                    // The owner returns on a new device: a recovery of the owner account.
                    Some(owner) => {
                        self.add_device(
                            owner,
                            challenge.public_key,
                            &body.device_name,
                            DeviceOrigin::OwnerCode,
                        );
                        self.recover(owner, RecoveryId::generate());
                    }
                    None => {
                        let owner = self.create_account(
                            &body.name,
                            challenge.public_key,
                            &body.device_name,
                            DeviceOrigin::OwnerCode,
                        );
                        self.owner = Some(owner);
                    }
                }
                json(201, &self.start_session(challenge.public_key)).map_err(plain)
            }
            (Method::Post, "/join") => {
                let body: Join = read(request).map_err(plain)?;
                let challenge = self
                    .consume(&body.proof, ChallengeOperation::Join, reached)
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
                    .consume(&body.proof, ChallengeOperation::RequestToJoin, reached)
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
                    .consume(&body.proof, ChallengeOperation::JoinRequestStatus, reached)
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
                    .consume(&body.proof, ChallengeOperation::LinkDevice, reached)
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
                json(200, &self.own_account(account_id)).map_err(plain)
            }
            (Method::Patch, "/account") => {
                let account_id = self.session_device(request).map_err(plain)?.account_id;
                let body: UpdateProfile = read(request).map_err(plain)?;
                if body.name.is_empty() || body.name.chars().count() > 64 {
                    return Err(plain("invalid_request"));
                }
                if let Some(account) = self.accounts.get_mut(&account_id) {
                    account.name = body.name;
                }
                json(200, &self.own_account(account_id)).map_err(plain)
            }
            (Method::Put, "/account/avatar") => {
                let account_id = self.session_device(request).map_err(plain)?.account_id;
                let image = request
                    .body
                    .as_ref()
                    .map(|body| body.as_slice().to_vec())
                    .unwrap_or_default();
                match check_avatar(&image) {
                    Err(InvalidAvatar::TooLarge) => return Err(plain("image_too_large")),
                    Err(InvalidAvatar::Unsupported) => return Err(plain("image_unsupported")),
                    Ok(_) => {}
                }
                if let Some(code) = self.avatar_refusal {
                    return Err(plain(code));
                }
                if let Some(account) = self.accounts.get_mut(&account_id) {
                    account.avatar = Some(image);
                }
                json(200, &self.avatar_description(account_id)).map_err(plain)
            }
            (Method::Delete, "/account/avatar") => {
                let account_id = self.session_device(request).map_err(plain)?.account_id;
                if let Some(account) = self.accounts.get_mut(&account_id) {
                    account.avatar = None;
                }
                Ok((204, Vec::new()))
            }
            (Method::Get, "/account/recovery-notices") => {
                let device_id = self.session_device(request).map_err(plain)?.id;
                json(200, &self.notices_of(device_id)).map_err(plain)
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

impl World {
    fn own_account(&self, account_id: AccountId) -> OwnAccount {
        OwnAccount {
            id: account_id,
            name: self.accounts[&account_id].name.clone(),
            avatar: self.avatar_description(account_id),
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: Vec::new(),
        }
    }

    /// The profile and the avatar of an account. Every account of this server sees every
    /// other one; an account that does not exist is `not_found`.
    fn account_route(&mut self, request: &HttpRequest, rest: &str) -> Reply {
        self.session_device(request)?;
        let (account, what) = rest.split_once('/').ok_or("not_found")?;
        let account_id: AccountId = account.parse().map_err(|_| "not_found")?;
        let known = self
            .accounts
            .get(&account_id)
            .filter(|account| account.status == Status::Active)
            .ok_or("not_found")?;
        match (request.method, what) {
            (Method::Get, "profile") => json(
                200,
                &AccountProfile {
                    id: account_id,
                    name: known.name.clone(),
                    avatar: self.avatar_description(account_id),
                },
            ),
            (Method::Get, "avatar") => Ok((200, known.avatar.clone().ok_or("not_found")?)),
            _ => Err("not_found"),
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
            let refused = world
                .refusals
                .iter()
                .position(|(end, _, _)| path.ends_with(end.as_str()))
                .map(|index| world.refusals.remove(index));
            if let Some((_, status, code)) = refused {
                let body = format!(r#"{{"error":{{"code":"{code}","message":"a sentence"}}}}"#);
                return Box::pin(async move {
                    Ok(HttpResponse {
                        status,
                        retry_after: None,
                        body: body.into_bytes().into(),
                        receipt: crate::api::Receipt::NotStated,
                    })
                });
            }
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
                        receipt: crate::api::Receipt::NotStated,
                    },
                    Err(code) => refusal(code),
                }),
                None => Ok(match world.handle(&request) {
                    Ok((status, body)) => HttpResponse {
                        status,
                        retry_after: None,
                        body: body.into(),
                        receipt: crate::api::Receipt::NotStated,
                    },
                    Err(code) => refusal(code),
                }),
            }
        };
        Box::pin(async move { result })
    }
}
