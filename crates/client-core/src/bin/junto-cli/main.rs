//! `junto-cli`: operates Junto servers from a terminal with the client library and nothing
//! else.

mod secret_input;

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::time::Duration;

use junto_client_core::address::ServerAddress;
use junto_client_core::book::{ConnectionMode, Membership};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessAction, AccessState, Notice};
use junto_client_core::flow::add_server::{AddServerAction, AddServerState};
use junto_client_core::flow::approve_link::{ApproveLinkAction, ApproveLinkState};
use junto_client_core::flow::link_all::{
    ApprovalProgress, ApproveDevicesAction, ApproveDevicesFlow, ApproveDevicesState, LONGEST_TEXT,
    LinkDeviceAction, LinkDeviceState, LinkProgress, LinkRequestText, ServerApproval,
};
use junto_client_core::paths::data_dir_of_this_user;
use junto_client_core::protocol::ids::{JoinRequestId, ServerId, WorkspaceId};
use junto_client_core::protocol::invite::{ApproveJoinRequest, CreateInvite};
use junto_client_core::protocol::secret::{InviteCode, LinkCode, OwnerCode};
use junto_client_core::{Client, ClientOptions, Reach, ServerHandle};
use time::OffsetDateTime;
use zeroize::Zeroizing;

const USAGE: &str = "usage: junto-cli [--data-dir <path>] <command> [options]

commands:
  servers                      list the saved servers
  add <address>                show the name of the server at an address and save it
                               [--when-selected]
  remove                       delete a saved server and the key of this device for it
  status                       contact the server and show what this device is there
  claim                        become the server owner with the one-time owner code
                               --name <name> [--device-name <name>]
  join                         create an account with an invite code
                               --name <name> [--device-name <name>]
  request                      ask for access without a code
                               --name <name> [--wait]
  login                        log in with the stored device key
  link                         link this device to an account of another device
                               [--device-name <name>] [--wait]
  approve-link                 approve the link code that a new device shows
                               [--yes]
  link-offer                   print the offer that a new device needs to link on every
                               server of this device
  link-all                     on a new device: take an offer and ask every server in it
                               for a link; prints the request for the first device
                               [--device-name <name>] [--wait]
  approve-all                  take the request of a new device and approve it on every
                               server of this device
                               [--yes]
  devices                      list the devices of the account
  invite                       create an invite code
                               [--max-uses <n>] [--expires-in-hours <n>] [--workspace <id>]...
  requests                     list the join requests that wait
  approve-request <id>         approve a join request
  profile                      show or set the local profile preset
                               [--name <name>]

Commands that work on one server take --server <identifier, address or name>. It can be
left out while one server is saved. --name can be left out when the profile preset has a
name.

An owner code, an invite code, a link code, an offer and a request are read from standard
input: one line, or a prompt without echo on a terminal. They are never taken from the
command line.";

const VALUE_OPTIONS: &[&str] = &[
    "data-dir",
    "server",
    "name",
    "device-name",
    "max-uses",
    "expires-in-hours",
    "workspace",
];
const FLAGS: &[&str] = &["when-selected", "wait", "yes"];
const WAIT_INTERVAL: Duration = Duration::from_secs(5);

enum Failure {
    Usage(String),
    Failed(String),
}

fn failed(error: impl std::fmt::Display) -> Failure {
    Failure::Failed(error.to_string())
}

fn usage(problem: impl Into<String>) -> Failure {
    Failure::Usage(problem.into())
}

#[derive(Default)]
struct Arguments {
    positional: Vec<String>,
    values: BTreeMap<String, Vec<String>>,
    flags: Vec<String>,
}

impl Arguments {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, Failure> {
        let mut parsed = Self::default();
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            let Some(name) = argument.strip_prefix("--") else {
                parsed.positional.push(argument);
                continue;
            };
            if FLAGS.contains(&name) {
                parsed.flags.push(name.to_owned());
            } else if VALUE_OPTIONS.contains(&name) {
                let value = arguments
                    .next()
                    .ok_or_else(|| usage(format!("--{name} needs a value")))?;
                parsed
                    .values
                    .entry(name.to_owned())
                    .or_default()
                    .push(value);
            } else if name == "code" {
                return Err(usage(
                    "a code is never given on the command line; it is read from standard input",
                ));
            } else {
                return Err(usage(format!("unknown option --{name}")));
            }
        }
        Ok(parsed)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    fn values(&self, name: &str) -> &[String] {
        self.values.get(name).map(Vec::as_slice).unwrap_or_default()
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    fn number(&self, name: &str) -> Result<Option<u32>, Failure> {
        self.value(name)
            .map(|text| {
                text.parse()
                    .map_err(|_| usage(format!("--{name} needs a whole number")))
            })
            .transpose()
    }

    fn device_name(&self) -> String {
        self.value("device-name").unwrap_or_default().to_owned()
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = match Arguments::parse(std::env::args().skip(1)) {
        Ok(arguments) => arguments,
        Err(failure) => return report(failure),
    };
    match run(arguments).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => report(failure),
    }
}

fn report(failure: Failure) -> ExitCode {
    match failure {
        Failure::Usage(problem) => {
            eprintln!("{problem}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Failure::Failed(problem) => {
            eprintln!("error: {problem}");
            ExitCode::FAILURE
        }
    }
}

async fn run(arguments: Arguments) -> Result<(), Failure> {
    let Some(command) = arguments.positional.first().map(String::as_str) else {
        return Err(usage("no command"));
    };
    let data_dir = match arguments.value("data-dir") {
        Some(path) => path.into(),
        None => data_dir_of_this_user().map_err(failed)?,
    };
    let client = Client::open(ClientOptions::new(data_dir)).map_err(failed)?;
    match command {
        "servers" => servers(&client),
        "add" => add(&client, &arguments).await,
        "profile" => profile(&client, &arguments),
        "link-offer" => {
            println!("offer: {}", client.link_offer().to_text());
            Ok(())
        }
        "link-all" => link_all(&client, &arguments).await,
        "approve-all" => approve_all(&client, &arguments).await,
        "remove" => {
            let server = pick(&client, &arguments)?;
            client.remove_server(server.id()).map_err(failed)?;
            println!("removed {}", server.entry().address);
            Ok(())
        }
        "status" => {
            let server = pick(&client, &arguments)?;
            let contacted = server.connect().await;
            notices(&server);
            show(&server);
            end_session(&server).await;
            contacted.map_err(failed)
        }
        "claim" => {
            let server = pick(&client, &arguments)?;
            let name = name(&client, &arguments)?;
            let action = AccessAction::ClaimOwner {
                code: OwnerCode::new(code("owner code")?.as_str()),
                name,
                device_name: arguments.device_name(),
            };
            enter(&server, action).await
        }
        "join" => {
            let server = pick(&client, &arguments)?;
            let name = name(&client, &arguments)?;
            let action = AccessAction::Join {
                code: InviteCode::new(code("invite code")?.as_str()),
                name,
                device_name: arguments.device_name(),
            };
            enter(&server, action).await
        }
        "request" => {
            let server = pick(&client, &arguments)?;
            let action = AccessAction::RequestAccess {
                name: name(&client, &arguments)?,
            };
            server.dispatch(action).await.map_err(failed)?;
            wait_or_show(&server, &arguments).await
        }
        "link" => {
            let server = pick(&client, &arguments)?;
            let action = AccessAction::RequestLink {
                device_name: arguments.device_name(),
            };
            server.dispatch(action).await.map_err(failed)?;
            wait_or_show(&server, &arguments).await
        }
        "login" => {
            let server = logged_in(&client, &arguments).await?;
            let account = server.api().own_account().await;
            show(&server);
            end_session(&server).await;
            let account = account.map_err(failed)?;
            println!("name: {}", account.name);
            println!("owner: {}", account.is_owner);
            Ok(())
        }
        "approve-link" => {
            let server = logged_in(&client, &arguments).await?;
            let result = approve_link(&server, &arguments).await;
            end_session(&server).await;
            result
        }
        "devices" => {
            let server = logged_in(&client, &arguments).await?;
            let devices = server.api().devices().await;
            end_session(&server).await;
            for device in devices.map_err(failed)? {
                let name = match device.name.as_str() {
                    "" => "(no name)",
                    name => name,
                };
                let current = if device.current {
                    "  (this device)"
                } else {
                    ""
                };
                println!(
                    "{}  {}  {:?}  {name}{current}",
                    device.id, device.public_key, device.origin
                );
            }
            Ok(())
        }
        "invite" => {
            let invite = create_invite(&arguments)?;
            let server = logged_in(&client, &arguments).await?;
            let created = server.api().create_invite(&invite).await;
            end_session(&server).await;
            let created = created.map_err(failed)?;
            println!("code: {}", created.code.expose());
            println!("uses: {}", created.invite.max_uses);
            println!("expires: {}", created.invite.expires_at);
            Ok(())
        }
        "requests" => {
            let server = logged_in(&client, &arguments).await?;
            let requests = server.api().join_requests().await;
            end_session(&server).await;
            for request in requests.map_err(failed)? {
                println!(
                    "{}  {}  until {}",
                    request.id, request.name, request.expires_at
                );
            }
            Ok(())
        }
        "approve-request" => {
            let id: JoinRequestId = arguments
                .positional
                .get(1)
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| usage("approve-request needs the identifier of a request"))?;
            let server = logged_in(&client, &arguments).await?;
            let approved = server
                .api()
                .approve_join_request(id, &ApproveJoinRequest::default())
                .await;
            end_session(&server).await;
            let status = approved.map_err(failed)?;
            println!("request {}: {:?}", status.id, status.state);
            Ok(())
        }
        other => Err(usage(format!("unknown command {other}"))),
    }
}

fn servers(client: &Client) -> Result<(), Failure> {
    for server in client.servers() {
        let entry = server.entry();
        let membership = match entry.membership {
            Membership::None => "no account",
            Membership::JoinRequested { .. } => "waiting for approval",
            Membership::Member { .. } => "member",
            Membership::Banned { .. } => "banned",
            Membership::Removed => "removed",
        };
        println!(
            "{}  {}  {}  {membership}",
            entry.server_id, entry.address, entry.name
        );
    }
    Ok(())
}

async fn add(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let address = arguments
        .positional
        .get(1)
        .ok_or_else(|| usage("add needs an address"))?;
    let flow = client.add_server();
    let fetched = flow
        .dispatch(AddServerAction::Fetch {
            address: address.clone(),
        })
        .await;
    match flow.view().state {
        AddServerState::Preview(preview) => {
            println!("name: {}", preview.name);
            println!("address: {}", preview.address);
            if let Err(incompatibility) = preview.version {
                println!("warning: {incompatibility}");
            }
        }
        AddServerState::KeyOfOtherAddress { key_address, .. } => {
            return Err(Failure::Failed(format!(
                "this device holds a key of this server that was created for {key_address}. \
                 An address that copies the identifier of another server would get its \
                 signatures, so the key is not used here."
            )));
        }
        _ => {}
    }
    fetched.map_err(failed)?;
    let connection_mode = if arguments.flag("when-selected") {
        ConnectionMode::WhenSelected
    } else {
        ConnectionMode::Always
    };
    flow.dispatch(AddServerAction::Save { connection_mode })
        .await
        .map_err(failed)?;
    if let AddServerState::Saved { server_id } = flow.view().state {
        println!("server: {server_id}");
    }
    Ok(())
}

fn profile(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let preset = match arguments.value("name") {
        Some(name) => client.profile().set_name(name).map_err(failed)?,
        None => client.profile().load().map_err(failed)?,
    };
    println!("name: {}", preset.name);
    Ok(())
}

fn pick(client: &Client, arguments: &Arguments) -> Result<ServerHandle, Failure> {
    let servers = client.servers();
    let Some(selector) = arguments.value("server") else {
        return match servers.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err(Failure::Failed("no server is saved; use `add`".to_owned())),
            _ => Err(usage("several servers are saved; name one with --server")),
        };
    };
    let by_id = selector.parse::<ServerId>().ok();
    let by_address = ServerAddress::parse(selector).ok();
    let mut matches = servers.into_iter().filter(|server| {
        let entry = server.entry();
        Some(entry.server_id) == by_id
            || Some(&entry.address) == by_address.as_ref()
            || entry.name == selector
    });
    match (matches.next(), matches.next()) {
        (Some(server), None) => Ok(server),
        (None, _) => Err(Failure::Failed(format!(
            "no saved server matches {selector}"
        ))),
        (Some(_), Some(_)) => Err(usage(format!("several saved servers match {selector}"))),
    }
}

fn name(client: &Client, arguments: &Arguments) -> Result<String, Failure> {
    if let Some(name) = arguments.value("name") {
        return Ok(name.to_owned());
    }
    let preset = client.profile().load().map_err(failed)?;
    if preset.name.is_empty() {
        return Err(usage("give --name, or set a name with `profile --name`"));
    }
    Ok(preset.name)
}

/// Reads a code or another text that must not be on the command line. The text stays in a
/// buffer that is overwritten when it is dropped, until a type of the protocol takes it.
fn secret(what: &str, limit: usize) -> Result<Zeroizing<String>, Failure> {
    let line = secret_input::read_secret(what, limit).map_err(failed)?;
    if line.is_empty() {
        return Err(usage(format!("no {what} on standard input")));
    }
    Ok(line)
}

fn code(what: &str) -> Result<Zeroizing<String>, Failure> {
    secret(what, secret_input::LONGEST_CODE)
}

/// An offer or a request for a link on every server.
fn bundle(what: &str) -> Result<Zeroizing<String>, Failure> {
    secret(what, LONGEST_TEXT + 64)
}

/// True when the user agrees: `--yes`, or a line that says so.
fn confirmed(arguments: &Arguments, question: &str) -> Result<bool, Failure> {
    if arguments.flag("yes") {
        return Ok(true);
    }
    eprint!("{question} [y/N] ");
    let answer = secret_input::read_line(16).map_err(failed)?;
    Ok(matches!(answer.as_str(), "y" | "Y" | "yes"))
}

fn create_invite(arguments: &Arguments) -> Result<CreateInvite, Failure> {
    let workspace_ids = arguments
        .values("workspace")
        .iter()
        .map(|text| {
            text.parse::<WorkspaceId>()
                .map_err(|_| usage("--workspace needs a workspace identifier"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CreateInvite {
        expires_at: arguments
            .number("expires-in-hours")?
            .map(|hours| OffsetDateTime::now_utc() + time::Duration::hours(hours.into())),
        max_uses: arguments.number("max-uses")?,
        workspace_ids,
    })
}

/// Runs an action that ends in a session, shows the result and ends the session.
async fn enter(server: &ServerHandle, action: AccessAction) -> Result<(), Failure> {
    let result = server.dispatch(action).await;
    notices(server);
    show(server);
    end_session(server).await;
    result.map_err(failed)
}

/// The login that every command of an account goes through. The recoveries of the account
/// since the login before are shown here, before the output of the command: the server
/// tells each device once.
async fn logged_in(client: &Client, arguments: &Arguments) -> Result<ServerHandle, Failure> {
    let server = pick(client, arguments)?;
    let contacted = server.connect().await;
    notices(&server);
    match server.access().state {
        AccessState::LoggedIn { .. } => Ok(server),
        _ => {
            show(&server);
            contacted.map_err(failed)?;
            Err(Failure::Failed("this device is not logged in".to_owned()))
        }
    }
}

/// Prints the recoveries that the server told at a login and that were not printed yet.
/// It runs after every contact that can log in, before the output of the command.
fn notices(server: &ServerHandle) {
    for recovery in server.take_recoveries() {
        let by = match recovery.performed_by {
            Some(account_id) => format!("the account {account_id}"),
            None => "a command on the host of the server".to_owned(),
        };
        println!(
            "recovery: a device was given access to the account on {} at {} by {by}",
            server.entry().address,
            recovery.performed_at
        );
    }
}

fn all_notices(client: &Client) {
    for server in client.servers() {
        notices(&server);
    }
}

/// A command is one process, so it ends the session that it started.
async fn end_session(server: &ServerHandle) {
    notices(server);
    if matches!(server.access().state, AccessState::LoggedIn { .. }) {
        let _ = server.dispatch(AccessAction::LogOut).await;
    }
}

async fn wait_or_show(server: &ServerHandle, arguments: &Arguments) -> Result<(), Failure> {
    show(server);
    if !arguments.flag("wait") {
        return Ok(());
    }
    server
        .wait_for_decision(WAIT_INTERVAL)
        .await
        .map_err(failed)?;
    notices(server);
    show(server);
    end_session(server).await;
    Ok(())
}

async fn approve_link(server: &ServerHandle, arguments: &Arguments) -> Result<(), Failure> {
    let flow = server.approve_link();
    let code = LinkCode::new(code("link code")?.as_str());
    flow.dispatch(ApproveLinkAction::LookUp { code })
        .await
        .map_err(failed)?;
    if let ApproveLinkState::Confirm(pending) = flow.view().state {
        println!("device name: {}", pending.device_name);
        println!("public key: {}", pending.public_key);
    }
    if !confirmed(
        arguments,
        "The new device must show the same public key. Approve?",
    )? {
        return Err(Failure::Failed("not approved".to_owned()));
    }
    let approved = flow.dispatch(ApproveLinkAction::Approve).await;
    match flow.view().state {
        ApproveLinkState::Approved { device } => {
            println!("approved device: {}", device.id);
            Ok(())
        }
        ApproveLinkState::OutcomeUnknown(_) => Err(Failure::Failed(
            "the answer of the server was lost; run `devices` to see whether the device was added"
                .to_owned(),
        )),
        _ => approved.map_err(failed),
    }
}

async fn link_all(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let flow = client.link_this_device();
    let offer = bundle("link offer")?;
    let started = flow
        .dispatch(LinkDeviceAction::Start {
            offer: offer.as_str().to_owned(),
            device_name: arguments.device_name(),
        })
        .await;
    all_notices(client);
    if !matches!(started, Ok(()) | Err(FlowError::Incomplete { .. })) {
        return started.map_err(failed);
    }
    show_links(&flow.view().state);
    if arguments.flag("wait") {
        while let LinkDeviceState::Requested { servers, .. } = flow.view().state {
            if servers
                .iter()
                .all(|server| server.progress != LinkProgress::Waiting)
            {
                break;
            }
            tokio::time::sleep(WAIT_INTERVAL).await;
            let _ = flow.dispatch(LinkDeviceAction::Check).await;
            all_notices(client);
        }
        show_links(&flow.view().state);
    }
    for server in client.servers() {
        end_session(&server).await;
    }
    match flow.view().error {
        None => Ok(()),
        Some(error) => Err(failed(error)),
    }
}

fn show_links(state: &LinkDeviceState) {
    let LinkDeviceState::Requested {
        servers,
        request,
        fingerprint,
        ..
    } = state
    else {
        return;
    };
    for server in servers {
        let progress = match &server.progress {
            LinkProgress::Working => "in progress".to_owned(),
            LinkProgress::Waiting => "waiting for approval".to_owned(),
            LinkProgress::Linked => "linked".to_owned(),
            LinkProgress::Failed(error) => format!("failed: {error}"),
        };
        println!("{}  {}  {progress}", server.server_id, server.address);
    }
    if let (Some(request), Some(fingerprint)) = (request, fingerprint) {
        println!("fingerprint: {fingerprint}");
        println!("request: {}", request.expose());
    }
}

async fn approve_all(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let flow = client.approve_devices();
    let request = LinkRequestText::new(bundle("link request")?);
    let looked_up = flow
        .dispatch(ApproveDevicesAction::LookUp { request })
        .await;
    all_notices(client);
    let ApproveDevicesState::Confirm {
        fingerprint,
        device_name,
        servers,
    } = flow.view().state
    else {
        return looked_up.map_err(failed);
    };
    println!("device name: {device_name}");
    println!("fingerprint: {fingerprint}");
    show_approvals(&servers);
    let question = "The new device must show the same fingerprint. Approve on every server, \
                    each for the account shown?";
    let agreed = confirmed(arguments, question)?;
    let mut result = if agreed {
        flow.dispatch(ApproveDevicesAction::Approve)
            .await
            .map_err(failed)
    } else {
        Err(Failure::Failed("not approved".to_owned()))
    };
    all_notices(client);
    if let Some(again) = confirm_changed_accounts(&flow, arguments).await? {
        result = again;
        all_notices(client);
    }
    if let (true, ApproveDevicesState::Confirm { servers, .. }) = (agreed, flow.view().state) {
        show_approvals(&servers);
    }
    for server in client.servers() {
        end_session(&server).await;
    }
    result
}

/// A server on which this device has another account than the one that was shown is
/// approved only after the user agreed to that account. `--yes` does not agree to it.
async fn confirm_changed_accounts(
    flow: &ApproveDevicesFlow,
    arguments: &Arguments,
) -> Result<Option<Result<(), Failure>>, Failure> {
    let ApproveDevicesState::Confirm { servers, .. } = flow.view().state else {
        return Ok(None);
    };
    let mut confirmed_any = false;
    for server in servers {
        let ApprovalProgress::AccountChanged { now } = server.progress else {
            continue;
        };
        println!(
            "{}  {}  this device now has the account {now} there",
            server.server_id, server.name
        );
        if arguments.flag("yes") {
            continue;
        }
        eprint!("Approve the new device for the account {now}? [y/N] ");
        let answer = secret_input::read_line(16).map_err(failed)?;
        if matches!(answer.as_str(), "y" | "Y" | "yes") {
            let server_id = server.server_id;
            flow.dispatch(ApproveDevicesAction::ConfirmAccount { server_id })
                .await
                .map_err(failed)?;
            confirmed_any = true;
        }
    }
    if !confirmed_any {
        return Ok(None);
    }
    let approved = flow.dispatch(ApproveDevicesAction::Approve).await;
    Ok(Some(approved.map_err(failed)))
}

fn show_approvals(servers: &[ServerApproval]) {
    for server in servers {
        let progress = match &server.progress {
            ApprovalProgress::Working => "in progress".to_owned(),
            ApprovalProgress::Ready => "ready".to_owned(),
            ApprovalProgress::Approved => "approved".to_owned(),
            ApprovalProgress::Failed(error) => format!("failed: {error}"),
            ApprovalProgress::Unknown => "unknown: the answer of the server was lost".to_owned(),
            ApprovalProgress::AccountChanged { now } => {
                format!("not approved: this device now has the account {now}")
            }
        };
        let account = match server.account_id {
            Some(account_id) => format!("account {account_id}"),
            None => "no account".to_owned(),
        };
        println!(
            "{}  {}  {account}  {}  {progress}",
            server.server_id, server.name, server.public_key
        );
    }
}

fn show(server: &ServerHandle) {
    let entry = server.entry();
    println!("server: {} ({})", entry.name, entry.address);
    match server.reach() {
        Reach::Unknown => {}
        Reach::Reachable { version } => println!("api: {version}"),
        Reach::Unreachable => println!("reach: the server cannot be reached"),
        Reach::Incompatible(incompatibility) => println!("reach: {incompatibility}"),
        Reach::OtherServer { reported } => {
            println!("reach: the address answers as another server ({reported})");
        }
    }
    match server.access().state {
        AccessState::NoAccount { notice } => {
            println!("state: no account");
            let reason = match notice {
                None => return,
                Some(Notice::Removed) => "the account or this device was removed",
                Some(Notice::RequestDenied) => "the join request was denied",
                Some(Notice::RequestExpired) => "the join request expired",
                Some(Notice::LinkExpired) => "the link code expired",
                Some(Notice::KeyMissing) => "the key of this device is not stored any more",
            };
            println!("notice: {reason}");
        }
        AccessState::WaitingForApproval {
            join_request_id,
            name,
        } => {
            println!("state: waiting for approval");
            if let Some(join_request_id) = join_request_id {
                println!("request: {join_request_id}");
            }
            println!("name: {name}");
        }
        AccessState::WaitingForLink {
            code,
            public_key,
            expires_at,
        } => {
            println!("state: waiting for a device link");
            println!("code: {}", code.expose());
            println!("public key: {public_key}");
            println!("expires: {expires_at}");
        }
        AccessState::LoggedOut { account_id } => {
            println!("state: logged out");
            println!("account: {account_id}");
        }
        AccessState::LoggedIn {
            account_id,
            device_id,
            ..
        } => {
            println!("state: logged in");
            println!("account: {account_id}");
            println!("device: {device_id}");
        }
        AccessState::Banned { .. } => println!("state: banned"),
    }
}
