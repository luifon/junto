//! `junto-cli`: operates Junto servers from a terminal with the client library and nothing
//! else.

mod admin;
mod discovery;
mod live;
mod messages;
mod recordings;
mod roles;
mod secret_input;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use junto_client_core::address::ServerAddress;
use junto_client_core::book::{ConnectionMode, Membership, ProfileSync};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::{AccessAction, AccessState, Notice};
use junto_client_core::flow::add_server::{AddServerAction, AddServerState};
use junto_client_core::flow::approve_link::{ApproveLinkAction, ApproveLinkState};
use junto_client_core::flow::link_all::{
    ApprovalProgress, ApproveDevicesAction, ApproveDevicesFlow, ApproveDevicesState, LONGEST_TEXT,
    LinkDeviceAction, LinkDeviceState, LinkProgress, LinkRequestText, ServerApproval,
};
use junto_client_core::paths::data_dir_of_this_user;
use junto_client_core::profile::{AVATAR_MAX_BYTES, ProfilePreset};
use junto_client_core::protocol::account::Avatar;
use junto_client_core::protocol::account::RecoveryNotice;
use junto_client_core::protocol::ids::{
    AccountId, JoinRequestId, RecoveryId, ServerId, WorkspaceId,
};
use junto_client_core::protocol::invite::{ApproveJoinRequest, CreateInvite};
use junto_client_core::protocol::secret::{InviteCode, LinkCode, OwnerCode};
use junto_client_core::{Client, ClientOptions, Owner, Reach, ServerHandle};
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
  login                        log in with the stored device key; ends a logout
  logout                       log out of the account on this device: no command logs in
                               again until `login`; the account and the key stay
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
  recoveries                   list the recoveries of the account that this device has not
                               acknowledged
  acknowledge-recovery <id>    state that a recovery was seen; this device is not told of
                               it again
  account [<account id>]       show the account of this device, or the profile of another
                               account; the options change the account of this device
                               [--name <name>] [--avatar <file>] [--no-avatar]
                               [--save-avatar <new file>]
  invite                       create an invite code
                               [--max-uses <n>] [--expires-in-hours <n>] [--workspace <id>]...
  requests                     list the join requests that wait
  approve-request <id>         approve a join request
  deny-request <id>            deny a join request
  invites <action> [arguments] list the invites, or cancel one
  redeem                       enter the workspaces of an invite code with the account of
                               this device
  workspaces <action> [arguments]
  categories <action> [arguments]
  channels <action> [arguments]
  overrides <action> [arguments]
  roles <action> [arguments]
  members <action> [arguments]
  accounts <action> [arguments]
  tokens <action> [arguments]
  exports <action> [arguments]
  audit <action> [arguments]
  archive-period <action> [arguments]
  retention <action> [arguments]
  logo <action> [arguments]
                               send one request of the administration routes
  profile                      show or set the local profile preset
                               [--name <name>] [--avatar <file>] [--no-avatar]
  state                        connect the event stream of the server and show the state
                               that it gives: workspaces, channels, calls, invitations
  watch                        show the state and then every event of the server, until
                               the line `quit` on standard input or an interrupt
                               [--seconds <n>]
  call <action> [arguments]    send one request of the call routes
  messages <action> [arguments]
                               send one request of the message routes
  threads <action> [arguments] send one request of the thread routes
  pins <action> [arguments]    send one request of the pin routes
  notices <action> [arguments] send one request of the notice and notification routes
  recordings <action> [arguments]
                               send one request of the routes of recordings and of
                               their transcripts
  search <text>                search the messages and transcript lines that the account
                               can read; the text takes words, a phrase in quotation
                               marks, `or`, and a minus before a word
                               [--channel <id>] [--author <account>] [--from <time>]
                               [--until <time>] [--limit <n>] [--cursor <continuation>]

actions of call:
  show [<channel>]             the call of a voice channel, or the call of this account
  join <channel>               ask for a place in the call of a voice channel
  leave
  state <muted yes|no> <deafened yes|no>
  share <application|system|none>      report a screen share and its sound
  unshare
  hand <up|down>
  speaking <yes|no>
  settings <channel> [--limit <n> | --no-limit] [--hide <yes|no>]
  lock <channel>, unlock <channel>
  meeting <channel> <on|off>
  ask <channel>                ask to enter a locked or full voice channel
  answer <request> <accept|deny>
  cancel-request <request>
  invite <channel> <account>
  accept <invitation>, refuse <invitation>, cancel-invitation <invitation>
  move <channel> <account> <to channel>
  join-move <move>, fail-move <move>
  block-microphone, unblock-microphone, block-sound, unblock-sound, disconnect
                               each with <channel> <account>
  direct <account>             ask for a place in the direct-message call with an account
  refuse-direct <account>
  mute-direct <account> [--hours <n>], unmute-direct <account>

actions of messages:
  list <place> [--before <position> | --after <position>] [--limit <n>]
                               one page of the history, the latest page without a position
  show <message>
  send <place> [--text <text>] [--reply <message>] [--file <path>]...
                               an empty line in the text starts a new paragraph
  edit <message> --text <text>
  delete <message>
  react <message> <emoji>, unreact <message> <emoji>
  read <place> <position>      move the read position of the account forward
  unread <place>               the read position and the unread counts
  conversations                the direct conversations of the account
  open <account>               the direct conversation with an account
  limits                       the limits of the server for the files of a message
  set-limits --bytes <n> --files <n>
  upload <place> <file>        upload a file and print its attachment
  download <message> <attachment> --to <directory>
                               save a file of a message under a name that no file has

A place is channel:<id>, direct:<conversation id>, or the identifier of a channel.

actions of threads:
  list <root message> [--before <position> | --after <position>] [--limit <n>]
                               the root, one page of the replies, and the fork channels
  reply <root message> [--text <text>] [--reply <message>] [--file <path>]...
  state <root message>         the read position and the unread counts in the thread
  read <root message> <position>
  unread [--before <continuation>] [--limit <n>]
                               the threads that hold something unread, the newest first
  follow <root message>, unfollow <root message>
                               the choice to follow a thread or not
  clear-follow <root message>  no choice: the rule of the server decides
  followed <channel> [--before <continuation>] [--limit <n>]
                               the threads of a channel that the account follows
  fork <root message> --name <name> [--kind <text|voice>]
                               copy the thread into a new channel

actions of pins:
  list <channel>
  add <message>, remove <message>

actions of notices:
  list [--before <notice>] [--limit <n>]
  read <notice>
  settings <scope>             the stored notification settings of a scope
  set-settings <scope> [--level <all|mentions|nothing|inherit>]
               [--mute <yes|no>] [--mute-hours <n>]
               [--role-mentions <yes|no|inherit>] [--everyone-mentions <yes|no|inherit>]
               [--dnd <yes|no>] [--quiet <HH:MM-HH:MM> [--time-zone <name>]] [--no-quiet]
                               change the named settings of a scope and keep the others;
                               the mute and the mention switches belong to a channel,
                               --dnd and --quiet to the account; a time zone is a
                               name such as Europe/Lisbon, and UTC without one
  read-all [--until <time>]    mark every notice up to a time as read, in one request
  default <workspace>          the level that an administrator set for a workspace
  set-default <workspace> <all|mentions|nothing>
  remove-default <workspace>   the workspace sets no level

A scope is account, workspace:<id> or channel:<id>. A time is written as
2026-01-31T00:00:00Z.

actions of recordings:
  list <workspace> [--before <recording>] [--limit <n>]
                               the recordings of a workspace, the newest first
  show <recording>             the recording with its segments and their files
  remove <recording>           delete the recording with its files and its transcript
  transcript <recording>       the state of the transcript, its speakers and the result
                               for each audio segment
  request-transcript <recording>
                               ask for the transcript again, when there is none or it
                               failed
  lines <recording> [--after <line>] [--limit <n>]
                               one page of the lines of the transcript
  text <recording> [--to <directory>]
                               the transcript as text; with --to it is saved as a file
                               under a name that no file has
  correct <recording> <line> --text <text>
  rename-speaker <recording> <number> --name <name>
                               name the voice with the label `Speaker <number>`
  download <recording> <segment> [--to <directory>]
                               save the file of a segment under a name that no file has,
                               in the directory of the tool without --to; a download
                               that was cut continues in its unfinished file
actions of workspaces:
  list
  create <name>
  rename <workspace> <name>
  delete <workspace>           delete a workspace with its members, roles, categories and
                               channels

actions of categories:
  create <workspace> <name>
  rename <workspace> <category> <name>
  move <workspace> <category> <position>
  delete <workspace> <category>
                               the channels of the category stay and have no category

actions of channels:
  list <workspace>             the categories and the channels that the account sees
  create <workspace> <name> --kind <text|voice> [--category <id>]
  update <channel> [--name <name>] [--category <id> | --no-category] [--position <n>]
         [--permanent <on|off>]
  delete <channel>
  archive <channel>, restore <channel>
  archived <workspace>         the archived channels that the account sees
  call-settings <channel> [--limit <n> | --no-limit] [--hide <on|off>]

actions of overrides:
  list <channel>
  set <channel> <role> [--allow <permission>]... [--deny <permission>]...
                               set the override of a role in a channel; without an option
                               the override is removed. A permission is written as the
                               server names it, such as view_channel
  change <channel> <role> <permission> <allow|deny|inherit>
                               change one permission of the override of a role in a channel
                               and keep the other permissions of it

actions of roles:
  list <workspace>             the roles of a workspace: the \"everyone\" role, then the
                               other roles from the highest position to the lowest
  create <workspace> <name> [--permissions <list>]
  rename <workspace> <role> <name>
  permissions <workspace> <role> <list>
                               replace the permissions of a role; --none in the place of
                               the list gives the role no permission
  delete <workspace> <role>
  give <workspace> <account> <role>, take <workspace> <account> <role>
  move <workspace> <role> <position>
                               give a role the position that a role of the list holds; each
                               role between the two positions takes the position of the
                               role beside it, in one request
  server <action> [arguments]  the same actions for the roles of the server, each without
                               <workspace>

A list of permissions is written with commas and without spaces, such as
view_channel,manage_messages. A role of the server takes the permissions of the server,
such as create_invites.

actions of members:
  list <workspace>
  add <workspace> <account>    add an existing account to a workspace
  remove <workspace> <account> [--reason <text>] [--message <text>]
                               remove an account from one workspace; the reason stays in
                               the audit log, the message is shown to the account

actions of accounts:
  list
  remove <account> [--reason <text>] [--message <text>]
  ban <account> [--reason <text>] [--message <text>]
                               the reason stays in the audit log, the message is shown to
                               the account
  unban <account>

actions of invites:
  list
  revoke <invite>

actions of tokens:
  list [--before <token>] [--limit <n>]
  create <name> --action <action>... --days <n>
                               create an access token and print it once. An action is
                               read_structure, manage_structure, manage_members or
                               read_audit_log
  revoke <token>

actions of exports:
  list
  channel <channel>, conversation <conversation>
                               ask for an export
  show <export>
  download <export> <new file>
  remove <export>

actions of audit:
  server [filters] [--before <entry>] [--limit <n>]
  workspace <workspace> [filters] [--before <entry>] [--limit <n>]
                               the filters: --action <name> for one action as the server
                               names it, such as account_banned; --actor <account>;
                               --target <identifier> for an identifier that the entry
                               names; --since <time> and --until <time>, each written as
                               2026-01-31T00:00:00Z

actions of archive-period:
  show <workspace>
  set <workspace> <days|never> the time after which the server deletes an archived channel

actions of retention:
  show <workspace>             the retention of the files and of the recordings
  files <workspace> <days|never>
  recordings <workspace> <days|never>
  direct show
  direct set <days|never>      the retention of the files of direct messages

actions of logo:
  show                         the type, the size and the hash of the logo of the server
  set <file>                   send an image file as the logo of the server
  remove

An action that joins a call prints the token for the media server only with --show-token.
A place in a call belongs to the session that asked for it, and every command ends its
session, so a place that `call` takes ends with the command. `watch` takes the same actions
as lines on standard input, without the word `call`, and sends them with its own session,
which lasts until the watch ends.

Commands that work on one server take --server <identifier, address or name>. It can be
left out while one server is saved. --name can be left out when the profile preset has a
name.

An owner code, an invite code, a link code, an offer and a request are read from standard
input: one line, or a prompt without echo on a terminal. They are never taken from the
command line.

Every command that logs in prints the recoveries of the account that this device has not
acknowledged, before its own output.";

const VALUE_OPTIONS: &[&str] = &[
    "data-dir",
    "server",
    "name",
    "device-name",
    "max-uses",
    "expires-in-hours",
    "workspace",
    "avatar",
    "save-avatar",
    "seconds",
    "limit",
    "hide",
    "hours",
    "text",
    "reply",
    "file",
    "before",
    "after",
    "bytes",
    "files",
    "to",
    "kind",
    "level",
    "mute",
    "mute-hours",
    "role-mentions",
    "everyone-mentions",
    "dnd",
    "quiet",
    "time-zone",
    "channel",
    "author",
    "from",
    "until",
    "cursor",
    "category",
    "position",
    "permanent",
    "allow",
    "deny",
    "action",
    "days",
    "reason",
    "message",
    "actor",
    "target",
    "since",
    "permissions",
];
const FLAGS: &[&str] = &[
    "when-selected",
    "wait",
    "yes",
    "no-avatar",
    "no-limit",
    "show-token",
    "no-quiet",
    "no-category",
    "none",
];
const WAIT_INTERVAL: Duration = Duration::from_secs(5);

enum Failure {
    Usage(String),
    Failed(String),
}

/// Text that a server sent, as the tool prints it: without control characters, so that a
/// server cannot move the cursor of a terminal or erase what the tool printed before.
fn shown(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
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
        "logout" => {
            let server = pick(&client, &arguments)?;
            let result = server
                .dispatch_when_free(AccessAction::LogOutByPerson)
                .await;
            show(&server);
            result.map_err(failed)
        }
        "login" => {
            let server = pick(&client, &arguments)?;
            if matches!(server.access().state, AccessState::LoggedOutByPerson { .. }) {
                server
                    .dispatch_when_free(AccessAction::LogIn)
                    .await
                    .map_err(failed)?;
            }
            let server = logged_in(&client, &arguments).await?;
            let account = server.api().own_account().await;
            show(&server);
            end_session(&server).await;
            let account = account.map_err(failed)?;
            println!("name: {}", shown(&account.name));
            println!("owner: {}", account.is_owner);
            Ok(())
        }
        "approve-link" => {
            let server = logged_in(&client, &arguments).await?;
            let result = approve_link(&server, &arguments).await;
            end_session(&server).await;
            result
        }
        "recoveries" => {
            let server = logged_in(&client, &arguments).await?;
            if let AccessState::LoggedIn { recoveries, .. } = server.access().state
                && recoveries.is_empty()
            {
                println!("no recovery waits for an acknowledgement");
            }
            end_session(&server).await;
            Ok(())
        }
        "acknowledge-recovery" => {
            let id: RecoveryId = arguments
                .positional
                .get(1)
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| usage("acknowledge-recovery needs the identifier of a recovery"))?;
            let server = logged_in(&client, &arguments).await?;
            let acknowledged = server.acknowledge_recovery(id).await;
            end_session(&server).await;
            acknowledged.map_err(failed)?;
            println!("acknowledged recovery {id}");
            Ok(())
        }
        "account" => {
            let other = arguments
                .positional
                .get(1)
                .map(|text| {
                    text.parse::<AccountId>()
                        .map_err(|_| usage("account takes the identifier of an account"))
                })
                .transpose()?;
            let changes = arguments.value("name").is_some()
                || arguments.value("avatar").is_some()
                || arguments.flag("no-avatar");
            if other.is_some() && changes {
                return Err(usage("only the account of this device can be changed"));
            }
            if arguments.value("avatar").is_some() && arguments.flag("no-avatar") {
                return Err(usage("--avatar and --no-avatar exclude each other"));
            }
            let image = arguments.value("avatar").map(read_image).transpose()?;
            let server = logged_in(&client, &arguments).await?;
            let result = account(&server, &arguments, other, image).await;
            end_session(&server).await;
            result
        }
        "devices" => {
            let server = logged_in(&client, &arguments).await?;
            let devices = server.api().devices().await;
            end_session(&server).await;
            for device in devices.map_err(failed)? {
                let name = match shown(&device.name) {
                    name if name.is_empty() => "(no name)".to_owned(),
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
            println!("code: {}", shown(created.code.expose()));
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
                    request.id,
                    shown(&request.name),
                    request.expires_at
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
        "state" => live::state(&client, &arguments).await,
        "watch" => live::watch(&client, &arguments).await,
        "call" => live::call(&client, &arguments).await,
        "messages" => messages::messages(&client, &arguments).await,
        "threads" | "pins" | "notices" | "search" => {
            discovery::run(&client, command, &arguments).await
        }
        "recordings" => recordings::run(&client, &arguments).await,
        "deny-request" | "invites" | "redeem" | "workspaces" | "categories" | "channels"
        | "overrides" | "members" | "accounts" | "tokens" | "exports" | "audit" | "logo"
        | "archive-period" | "retention" => admin::run(&client, command, &arguments).await,
        "roles" => roles::run(&client, &arguments).await,
        other => Err(usage(format!("unknown command {other}"))),
    }
}

fn servers(client: &Client) -> Result<(), Failure> {
    for server in client.servers() {
        let entry = server.entry();
        let membership = match entry.membership {
            Membership::None => "no account",
            Membership::JoinRequested { .. } => "waiting for approval",
            Membership::Member { .. } if entry.logged_out => "member, logged out",
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
    if arguments.value("avatar").is_some() && arguments.flag("no-avatar") {
        return Err(usage("--avatar and --no-avatar exclude each other"));
    }
    let image = arguments.value("avatar").map(read_image).transpose()?;
    let store = client.profile();
    let mut preset: ProfilePreset = store.load().map_err(failed)?;
    if let Some(name) = arguments.value("name") {
        preset = store.set_name(name).map_err(failed)?;
    }
    if let Some(image) = image {
        preset = store.set_avatar(&image).map_err(failed)?;
    }
    if arguments.flag("no-avatar") {
        preset = store.clear_avatar().map_err(failed)?;
    }
    println!("name: {}", preset.name);
    match preset.avatar {
        Some(kind) => println!("avatar: {}", kind.media_type()),
        None => println!("avatar: none"),
    }
    Ok(())
}

/// Reads an image file. One byte more than an avatar can have is read at most, so that a
/// larger file is refused by the check of the library and not held in memory.
fn read_image(path: &str) -> Result<Vec<u8>, Failure> {
    let unreadable = |error: std::io::Error| Failure::Failed(format!("{path}: {error}"));
    let file = std::fs::File::open(path).map_err(unreadable)?;
    let mut image = Vec::new();
    file.take(AVATAR_MAX_BYTES as u64 + 1)
        .read_to_end(&mut image)
        .map_err(unreadable)?;
    Ok(image)
}

fn show_avatar(avatar: Option<&Avatar>) {
    match avatar {
        Some(avatar) => println!(
            "avatar: {}  {} bytes  {}x{}  {}",
            avatar.media_type.media_type(),
            avatar.bytes,
            avatar.width,
            avatar.height,
            shown(&avatar.hash)
        ),
        None => println!("avatar: none"),
    }
}

/// Shows the account of this device after the changes that the options ask for, or the
/// profile of another account. `--save-avatar` writes the image to a file that does not
/// exist yet.
async fn account(
    server: &ServerHandle,
    arguments: &Arguments,
    other: Option<AccountId>,
    image: Option<Vec<u8>>,
) -> Result<(), Failure> {
    let api = server.api();
    let (account_id, avatar) = match other {
        Some(account_id) => {
            let profile = api.profile(account_id).await.map_err(failed)?;
            println!("account: {}", profile.id);
            println!("name: {}", shown(&profile.name));
            (profile.id, profile.avatar)
        }
        None => {
            if let Some(name) = arguments.value("name") {
                api.update_profile(name).await.map_err(failed)?;
            }
            if let Some(image) = image {
                api.set_avatar(image).await.map_err(failed)?;
            }
            if arguments.flag("no-avatar") {
                api.remove_avatar().await.map_err(failed)?;
            }
            let account = api.own_account().await.map_err(failed)?;
            println!("account: {}", account.id);
            println!("name: {}", shown(&account.name));
            println!("owner: {}", account.is_owner);
            (account.id, account.avatar)
        }
    };
    show_avatar(avatar.as_ref());
    let Some(path) = arguments.value("save-avatar") else {
        return Ok(());
    };
    if avatar.is_none() {
        return Err(Failure::Failed("the account has no avatar".to_owned()));
    }
    let image = api.avatar(account_id).await.map_err(failed)?;
    let unwritable = |error: std::io::Error| Failure::Failed(format!("{path}: {error}"));
    let mut file = std::fs::File::create_new(path).map_err(unwritable)?;
    file.write_all(&image.bytes).map_err(unwritable)?;
    println!(
        "saved: {path}  {}  {}",
        image.media_type.media_type(),
        image.hash
    );
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
/// that this device has not acknowledged are shown here, before the output of the command.
async fn logged_in(client: &Client, arguments: &Arguments) -> Result<ServerHandle, Failure> {
    let server = pick(client, arguments)?;
    let contacted = server.connect().await;
    notices(&server);
    match server.access().state {
        AccessState::LoggedIn { .. } => Ok(server),
        AccessState::LoggedOutByPerson { .. } => Err(Failure::Failed(
            "this device is logged out of this server; `login` logs in again".to_owned(),
        )),
        _ => {
            show(&server);
            contacted.map_err(failed)?;
            Err(Failure::Failed("this device is not logged in".to_owned()))
        }
    }
}

/// The recoveries that this process has printed, each with the owner of the server that
/// stated it (ADR 0745). Two servers can state the same identifier.
static PRINTED: Mutex<Vec<(Owner, RecoveryId)>> = Mutex::new(Vec::new());

/// The recoveries that are not printed for `owner` yet. From now on they count as printed
/// for that owner, and for no other.
fn unprinted<'a>(
    printed: &mut Vec<(Owner, RecoveryId)>,
    owner: Owner,
    recoveries: &'a [RecoveryNotice],
) -> Vec<&'a RecoveryNotice> {
    recoveries
        .iter()
        .filter(|recovery| {
            let key = (owner, recovery.id);
            let new = !printed.contains(&key);
            if new {
                printed.push(key);
            }
            new
        })
        .collect()
}

/// Prints the recoveries that this device has not acknowledged, each once in this process.
/// It runs after every contact that can log in, before the output of the command. A
/// printed line does not prove that a person read it, so the tool acknowledges a recovery
/// only with the command `acknowledge-recovery` (ADR 0118).
fn notices(server: &ServerHandle) {
    let AccessState::LoggedIn { recoveries, .. } = server.access().state else {
        return;
    };
    let Some(owner) = server.owner() else {
        return;
    };
    let mut printed = PRINTED.lock().unwrap_or_else(PoisonError::into_inner);
    for recovery in unprinted(&mut printed, owner, &recoveries) {
        let by = match recovery.performed_by {
            Some(account_id) => format!("the account {account_id}"),
            None => "a command on the host of the server".to_owned(),
        };
        println!(
            "recovery {}: a device was given access to the account on {} at {} by {by}; not \
             acknowledged",
            recovery.id,
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

/// A command is one process, so it ends the session that it started, also one that a
/// login of this moment creates.
async fn end_session(server: &ServerHandle) {
    notices(server);
    let _ = server.end_session().await;
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
        AccessState::LoggedOutByPerson { account_id } => {
            println!("state: logged out by the person; `login` logs in again");
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
            match server.profile_sync() {
                ProfileSync::NotNeeded | ProfileSync::Applied => {}
                ProfileSync::Pending { attempts } => {
                    println!("preset avatar: not sent yet ({attempts} failed tries)");
                }
                ProfileSync::Refused => println!("preset avatar: refused by the server"),
            }
        }
        AccessState::Banned { .. } => println!("state: banned"),
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::EntryKey;
    use junto_client_core::protocol::account::RecoveryNotice;
    use junto_client_core::protocol::ids::{AccountId, RecoveryId, ServerId};
    use time::OffsetDateTime;

    use super::{Owner, shown, unprinted};

    #[test]
    fn recovery_notices_with_equal_ids_on_two_servers_are_both_printed() {
        let owner = || Owner {
            entry: EntryKey {
                server_id: ServerId::generate(),
                generation: 1,
            },
            account_id: AccountId::generate(),
        };
        let (first, second) = (owner(), owner());
        let notice = RecoveryNotice {
            id: RecoveryId::generate(),
            device_id: None,
            performed_by: None,
            performed_at: OffsetDateTime::UNIX_EPOCH,
        };
        let recoveries = vec![notice.clone()];
        let mut printed = Vec::new();
        // Each server is printed once, also when a later contact states it again.
        assert_eq!(unprinted(&mut printed, first, &recoveries), [&notice]);
        assert!(unprinted(&mut printed, first, &recoveries).is_empty());
        assert_eq!(unprinted(&mut printed, second, &recoveries), [&notice]);
        assert!(unprinted(&mut printed, second, &recoveries).is_empty());
        // The next account of an entry, and the entry added again, are other owners.
        let mut account = first;
        account.account_id = AccountId::generate();
        let mut added_again = first;
        added_again.entry.generation += 1;
        assert_eq!(unprinted(&mut printed, account, &recoveries), [&notice]);
        assert_eq!(unprinted(&mut printed, added_again, &recoveries), [&notice]);
    }

    #[test]
    fn text_of_a_server_is_printed_without_control_characters() {
        assert_eq!(shown("Ada"), "Ada");
        assert_eq!(
            shown("Ada\u{1b}[2J\u{1b}[H\rowner: true\n"),
            "Ada[2J[Howner: true"
        );
        assert_eq!(shown("a\u{7}\u{8}\u{9b}b\u{0}"), "ab");
        assert_eq!(shown("Zo\u{eb} \u{1f600}"), "Zo\u{eb} \u{1f600}");
    }
}
