use junto_protocol::account::OwnAccount;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::auth::SessionCreated;
use junto_protocol::error::ErrorCode;
use junto_protocol::structure::{ChannelKind, ChannelList, Workspace};
use junto_protocol::{API_VERSION, ServerPermission};
use reqwest::StatusCode;

#[cfg(unix)]
use crate::support::Scratch;
use crate::support::{DeviceKey, TestServer, member};

#[tokio::test]
async fn server_information_is_public() {
    let server = TestServer::start().await;
    assert_eq!(server.info.name, "Acme");
    assert_eq!(server.info.logo_path, None);
    assert_eq!(server.info.api_versions, [API_VERSION]);
}

#[tokio::test]
async fn first_start_makes_a_usable_server_and_the_owner_code_makes_the_owner() {
    let server = TestServer::start().await;
    assert_eq!(server.owner_code.replace('-', "").len(), 20);

    let stranger = DeviceKey::generate();
    server
        .claim_owner_reply(&stranger, "AAAA-AAAA-AAAA-AAAA-AAAA")
        .await
        .error(ErrorCode::OwnerCodeInvalid);

    let key = DeviceKey::generate();
    let lower_case_without_hyphens = server.owner_code.replace('-', "").to_lowercase();
    let session: SessionCreated = server
        .claim_owner_reply(&key, &lower_case_without_hyphens)
        .await
        .ok(StatusCode::CREATED);
    assert!(session.recoveries.is_empty());
    let owner = member(key, session);

    let account: OwnAccount = server
        .get(&owner, "/api/v1/account")
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.id, owner.account_id);
    assert_eq!(account.name, "Owner");
    assert!(account.is_owner);
    assert_eq!(account.server_permissions, Vec::<ServerPermission>::new());

    let workspaces: Vec<Workspace> = server
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces.len(), 1);
    assert!(workspaces[0].is_default);
    assert_eq!(workspaces[0].name, "Acme");
    assert_eq!(account.workspace_ids, [workspaces[0].id]);

    let channels: ChannelList = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspaces[0].id),
        )
        .await
        .ok(StatusCode::OK);
    let kinds: Vec<(ChannelKind, &str)> = channels
        .channels
        .iter()
        .map(|channel| (channel.kind, channel.name.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            (ChannelKind::Text, "general"),
            (ChannelKind::Voice, "voice")
        ]
    );

    let late = DeviceKey::generate();
    server
        .claim_owner_reply(&late, &server.owner_code)
        .await
        .error(ErrorCode::OwnerAlreadyClaimed);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert_eq!(log.entries.len(), 1);
    assert_eq!(log.entries[0].actor_id, Some(owner.account_id));
    assert_eq!(
        log.entries[0].event,
        AuditEvent::OwnerClaimed {
            account_id: owner.account_id
        }
    );
}

#[tokio::test]
async fn a_restart_before_the_claim_replaces_the_owner_code() {
    let first = TestServer::start().await;
    let second = first.restart().await;
    assert_ne!(first.owner_code, second.owner_code);
    assert_eq!(first.info.server_id, second.info.server_id);

    second
        .claim_owner_reply(&DeviceKey::generate(), &first.owner_code)
        .await
        .error(ErrorCode::OwnerCodeInvalid);
    let owner = second.owner().await;

    let third = second.restart().await;
    assert_eq!(third.owner_code, "");
    let workspaces: Vec<Workspace> = third
        .get(&owner, "/api/v1/workspaces")
        .await
        .ok(StatusCode::OK);
    assert_eq!(workspaces.len(), 1);
    let stored_code: Option<Vec<u8>> =
        sqlx::query_scalar("select owner_code_hash from server_state")
            .fetch_one(&third.db)
            .await
            .unwrap();
    assert_eq!(stored_code, None);
}

#[tokio::test]
async fn the_owner_code_is_stored_only_as_a_hash() {
    let server = TestServer::start().await;
    let stored: Vec<u8> = sqlx::query_scalar("select owner_code_hash from server_state")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert_ne!(stored, server.owner_code.as_bytes());
}

#[tokio::test]
async fn two_devices_that_present_the_owner_code_at_once_make_one_owner() {
    let server = TestServer::start().await;
    let first = DeviceKey::generate();
    let second = DeviceKey::generate();
    let (first_reply, second_reply) = tokio::join!(
        server.claim_owner_reply(&first, &server.owner_code),
        server.claim_owner_reply(&second, &server.owner_code),
    );
    let mut statuses = [first_reply.status, second_reply.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
    let accounts: i64 = sqlx::query_scalar("select count(*) from accounts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}

/// A server without an owner writes the notice of its owner code to standard output in one
/// write, so that a reader of standard output gets the code with the sentences before it in
/// one read.
#[cfg(unix)]
#[tokio::test]
async fn the_binary_writes_the_notice_of_the_owner_code_in_one_write() {
    let server = TestServer::start().await;
    let scratch = Scratch::new();
    let notice = one_write_of_owner_code(
        &server,
        &scratch,
        &[],
        "This server has no owner. The first client that presents this one-time code becomes \
         the server owner. A restart replaces the code.",
    )
    .await;
    assert_ne!(notice, server.owner_code);
    server
        .claim_owner_reply(&DeviceKey::generate(), &notice)
        .await
        .ok::<SessionCreated>(StatusCode::CREATED);
}

/// `issue-owner-code` writes its notice in one write too.
#[cfg(unix)]
#[tokio::test]
async fn the_host_command_writes_the_notice_of_the_owner_code_in_one_write() {
    let server = TestServer::start().await;
    let scratch = Scratch::new();
    let notice = one_write_of_owner_code(
        &server,
        &scratch,
        &["issue-owner-code"],
        "This server has no owner. The first client that presents this one-time code becomes \
         the server owner.",
    )
    .await;
    server
        .claim_owner_reply(&DeviceKey::generate(), &notice)
        .await
        .ok::<SessionCreated>(StatusCode::CREATED);
}

/// Runs the server binary with `arguments` on the database of `server` until it writes a
/// text that starts with `sentences`, and returns the owner code of that text. The standard
/// output of the binary is a datagram socket, which receives each write as one datagram, and
/// the test requires that one datagram holds the sentences, a blank line, the code indented
/// by four spaces, and a blank line.
#[cfg(unix)]
async fn one_write_of_owner_code(
    server: &TestServer,
    scratch: &Scratch,
    arguments: &[&str],
    sentences: &'static str,
) -> String {
    use std::os::unix::net::UnixDatagram;
    use std::time::{Duration, Instant};

    let (output, received) = UnixDatagram::pair().unwrap();
    let log = scratch.0.join("standard-error");
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_junto-server"));
    command
        .args(arguments)
        .env_clear()
        .env("JUNTO_DATABASE_URL", server.database_url())
        .env("JUNTO_LISTEN_ADDRESS", "127.0.0.1:0")
        .env("JUNTO_ATTACHMENTS_DIR", &scratch.0)
        .env("JUNTO_EXPORTS_DIR", &scratch.0)
        .env("JUNTO_STOP_WITH_PROCESS", std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::os::fd::OwnedFd::from(output))
        .stderr(std::fs::File::create(&log).unwrap());
    let writes = tokio::task::spawn_blocking(move || {
        let mut child = command.spawn().unwrap();
        drop(command);
        received
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let started = Instant::now();
        let mut writes = Vec::new();
        let mut datagram = vec![0; 64 * 1024];
        let found = loop {
            match received.recv(&mut datagram) {
                Ok(length) => {
                    let write = String::from_utf8_lossy(&datagram[..length]).into_owned();
                    let found = write.contains(sentences);
                    writes.push(write);
                    if found {
                        break true;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("the standard output of the binary failed: {error}"),
            }
            if child.try_wait().unwrap().is_some() || started.elapsed() > Duration::from_secs(60) {
                break false;
            }
        };
        let _ = child.kill();
        child.wait().unwrap();
        (found, writes)
    })
    .await
    .unwrap();
    let (found, writes) = writes;
    let standard_error = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        found,
        "the binary wrote no notice of an owner code; its writes: {writes:?}; standard error: \
         {standard_error}"
    );
    let notice = writes.last().unwrap();
    let code = notice
        .strip_prefix(sentences)
        .and_then(|rest| rest.strip_prefix("\n\n    "))
        .and_then(|rest| rest.strip_suffix("\n\n"))
        .filter(|code| code.len() == 24 && !code.contains(char::is_whitespace));
    match code {
        Some(code) => code.to_owned(),
        None => panic!("the notice of the owner code is not one complete write: {writes:?}"),
    }
}
