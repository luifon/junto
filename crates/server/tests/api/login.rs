use junto_protocol::account::OwnAccount;
use junto_protocol::auth::{Challenge, ChallengeOperation, Proof};
use junto_protocol::error::ErrorCode;
use junto_protocol::keys::Signature;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::support::{DeviceKey, TestServer};

async fn login_with(server: &TestServer, proof: &Proof) -> crate::support::Reply {
    server
        .open("/api/v1/auth/sessions", &json!({ "proof": proof }))
        .await
}

#[tokio::test]
async fn a_signed_challenge_starts_a_session() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let challenge = server
        .challenge(&owner.key, ChallengeOperation::Login)
        .await;
    assert_eq!(challenge.server_id, server.info.server_id);
    assert_eq!(challenge.account_id, Some(owner.account_id));
    assert_eq!(challenge.public_key, owner.key.public());
    assert_eq!(challenge.operation, ChallengeOperation::Login);
    assert_eq!(
        challenge.expires_at - server.clock.now().replace_nanosecond(0).unwrap(),
        Duration::seconds(60)
    );

    let session = server.login(&owner.key).await;
    assert_eq!(session.account_id, owner.account_id);
    assert_eq!(session.device_id, owner.device_id);
    assert!(session.token.expose().starts_with("jst_"));
    assert_ne!(session.token.expose(), owner.token);

    let account: OwnAccount = server
        .send::<Value>(
            Method::GET,
            "/api/v1/account",
            Some(session.token.expose()),
            None,
            None,
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(account.id, owner.account_id);
}

#[tokio::test]
async fn session_tokens_are_stored_hashed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let hashes: Vec<Vec<u8>> = sqlx::query_scalar("select token_hash from sessions")
        .fetch_all(&server.db)
        .await
        .unwrap();
    assert_eq!(hashes.len(), 1);
    assert_eq!(hashes[0].len(), 32);
    assert_ne!(hashes[0], owner.token.as_bytes());
}

#[tokio::test]
async fn a_wrong_signature_is_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = DeviceKey::generate();

    let challenge = server
        .challenge(&owner.key, ChallengeOperation::Login)
        .await;
    let signed_by_another_key = Proof {
        challenge_id: challenge.id,
        signature: other.sign(&challenge),
    };
    login_with(&server, &signed_by_another_key)
        .await
        .error(ErrorCode::InvalidSignature);

    let challenge = server
        .challenge(&owner.key, ChallengeOperation::Login)
        .await;
    let altered = Challenge {
        account_id: None,
        ..challenge.clone()
    };
    let signed_over_other_fields = Proof {
        challenge_id: challenge.id,
        signature: owner.key.sign(&altered),
    };
    login_with(&server, &signed_over_other_fields)
        .await
        .error(ErrorCode::InvalidSignature);

    let challenge = server
        .challenge(&owner.key, ChallengeOperation::Login)
        .await;
    let garbage = Proof {
        challenge_id: challenge.id,
        signature: Signature::from_bytes([7; 64]),
    };
    login_with(&server, &garbage)
        .await
        .error(ErrorCode::InvalidSignature);

    server.login(&owner.key).await;
}

#[tokio::test]
async fn an_expired_challenge_is_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let proof = server.proof(&owner.key, ChallengeOperation::Login).await;
    server.clock.advance(Duration::seconds(61));
    login_with(&server, &proof)
        .await
        .error(ErrorCode::ChallengeExpired);

    let proof = server.proof(&owner.key, ChallengeOperation::Login).await;
    server.clock.advance(Duration::seconds(59));
    login_with(&server, &proof)
        .await
        .ok::<Value>(StatusCode::CREATED);
}

#[tokio::test]
async fn a_challenge_is_used_once() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let proof = server.proof(&owner.key, ChallengeOperation::Login).await;
    login_with(&server, &proof)
        .await
        .ok::<Value>(StatusCode::CREATED);
    login_with(&server, &proof)
        .await
        .error(ErrorCode::ChallengeInvalid);

    let challenge = server
        .challenge(&owner.key, ChallengeOperation::Login)
        .await;
    let wrong = Proof {
        challenge_id: challenge.id,
        signature: Signature::from_bytes([1; 64]),
    };
    let right = Proof {
        challenge_id: challenge.id,
        signature: owner.key.sign(&challenge),
    };
    login_with(&server, &wrong)
        .await
        .error(ErrorCode::InvalidSignature);
    login_with(&server, &right)
        .await
        .ok::<Value>(StatusCode::CREATED);
    for proof in [&right, &wrong] {
        login_with(&server, proof)
            .await
            .error(ErrorCode::ChallengeInvalid);
    }
}

#[tokio::test]
async fn a_new_challenge_replaces_the_one_before() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let first = server.proof(&owner.key, ChallengeOperation::Login).await;
    let second = server.proof(&owner.key, ChallengeOperation::Login).await;
    login_with(&server, &first)
        .await
        .error(ErrorCode::ChallengeInvalid);
    login_with(&server, &second)
        .await
        .ok::<Value>(StatusCode::CREATED);
    let stored: i64 =
        sqlx::query_scalar("select count(*) from challenges where operation = 'login'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn a_challenge_is_bound_to_its_operation() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let for_link = server
        .proof(&owner.key, ChallengeOperation::LinkDevice)
        .await;
    login_with(&server, &for_link)
        .await
        .error(ErrorCode::ChallengeInvalid);

    let for_login = server.proof(&owner.key, ChallengeOperation::Login).await;
    server
        .open(
            "/api/v1/join",
            &json!({ "proof": for_login, "invite_code": "AAAA-AAAA-AAAA-AAAA", "name": "Name" }),
        )
        .await
        .error(ErrorCode::ChallengeInvalid);
}

#[tokio::test]
async fn a_challenge_of_one_server_is_unknown_to_another() {
    let first = TestServer::start().await;
    let second = TestServer::start().await;
    assert_ne!(first.info.server_id, second.info.server_id);
    let owner = first.owner().await;
    second.owner().await;
    let proof = first.proof(&owner.key, ChallengeOperation::Login).await;
    login_with(&second, &proof)
        .await
        .error(ErrorCode::ChallengeInvalid);
}

#[tokio::test]
async fn an_unknown_or_malformed_key_gets_no_login_challenge() {
    let server = TestServer::start().await;
    let stranger = DeviceKey::generate();
    server
        .challenge_reply(&stranger.public(), ChallengeOperation::Login)
        .await
        .error(ErrorCode::UnknownDevice);
    server
        .open(
            "/api/v1/auth/challenges",
            &json!({ "public_key": "short", "operation": "login" }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .open(
            "/api/v1/auth/challenges",
            &json!({ "public_key": stranger.public(), "operation": "become_owner" }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

async fn wrong_login(server: &TestServer, key: &DeviceKey, address: &str) -> crate::support::Reply {
    let challenge = server.challenge(key, ChallengeOperation::Login).await;
    let wrong = Proof {
        challenge_id: challenge.id,
        signature: Signature::from_bytes([9; 64]),
    };
    server
        .send(
            Method::POST,
            "/api/v1/auth/sessions",
            None,
            Some(&json!({ "proof": wrong })),
            Some(address),
        )
        .await
}

async fn right_login(server: &TestServer, key: &DeviceKey, address: &str) -> crate::support::Reply {
    let proof = server.proof(key, ChallengeOperation::Login).await;
    server
        .send(
            Method::POST,
            "/api/v1/auth/sessions",
            None,
            Some(&json!({ "proof": proof })),
            Some(address),
        )
        .await
}

fn behind_a_proxy(config: &mut junto_server::config::Config) {
    config.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
}

#[tokio::test]
async fn wrong_signatures_from_anyone_do_not_lock_a_device_key() {
    let server = TestServer::start_with(behind_a_proxy).await;
    let owner = server.owner().await;

    for _ in 0..25 {
        let reply = wrong_login(&server, &owner.key, "198.51.100.66").await;
        assert_eq!(reply.status.as_u16() / 100, 4, "{}", reply.text());
    }

    for address in ["198.51.100.7", "198.51.100.66"] {
        let reply = right_login(&server, &owner.key, address).await;
        assert_eq!(
            reply.status,
            StatusCode::CREATED,
            "{address}: {}",
            reply.text()
        );
    }
}

#[tokio::test]
async fn wrong_signatures_are_limited_per_client_address() {
    let server = TestServer::start_with(|config| {
        behind_a_proxy(config);
        config.limits.wrong_signatures.attempts = 3;
    })
    .await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;

    for key in [&owner.key, &guest.key, &owner.key] {
        wrong_login(&server, key, "198.51.100.66")
            .await
            .error(ErrorCode::InvalidSignature);
    }
    for key in [&owner.key, &guest.key] {
        let refused = wrong_login(&server, key, "198.51.100.66").await;
        refused.error(ErrorCode::TooManyAttempts);
        assert_eq!(refused.retry_after.as_deref(), Some("600"));
    }

    wrong_login(&server, &owner.key, "198.51.100.7")
        .await
        .error(ErrorCode::InvalidSignature);
    right_login(&server, &owner.key, "198.51.100.66")
        .await
        .ok::<Value>(StatusCode::CREATED);

    server.clock.advance(Duration::minutes(10));
    wrong_login(&server, &owner.key, "198.51.100.66")
        .await
        .error(ErrorCode::InvalidSignature);
}

#[tokio::test]
async fn the_challenges_of_one_key_are_bounded() {
    let server = TestServer::start().await;
    let key = DeviceKey::generate();
    for _ in 0..3 {
        for operation in ChallengeOperation::ALL {
            if *operation != ChallengeOperation::Login {
                server.challenge(&key, *operation).await;
            }
        }
    }
    let stored: i64 = sqlx::query_scalar("select count(*) from challenges where public_key = $1")
        .bind(key.public().as_bytes().as_slice())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored, 5);
    let attempts_table: Option<String> =
        sqlx::query_scalar("select to_regclass('key_attempts')::text")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(attempts_table, None);
}

#[tokio::test]
async fn a_request_without_a_valid_session_is_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let forged = format!("{}x", owner.token);
    for token in [
        None,
        Some("not-a-token"),
        Some("jst_unknown"),
        Some(forged.as_str()),
    ] {
        server
            .send::<Value>(Method::GET, "/api/v1/account", token, None, None)
            .await
            .error(ErrorCode::Unauthenticated);
    }
}

#[tokio::test]
async fn a_session_ends_at_logout_and_at_its_expiry() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let second = server.login(&owner.key).await;

    server
        .delete(&owner, "/api/v1/auth/session")
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&owner, "/api/v1/account")
        .await
        .error(ErrorCode::Unauthenticated);

    let second_token = second.token.expose().to_owned();
    let get = |token: String| {
        let server = &server;
        async move {
            server
                .send::<Value>(Method::GET, "/api/v1/account", Some(&token), None, None)
                .await
        }
    };
    get(second_token.clone()).await.ok::<Value>(StatusCode::OK);
    server.clock.advance(Duration::days(29));
    get(second_token.clone()).await.ok::<Value>(StatusCode::OK);
    server.clock.advance(Duration::days(2));
    get(second_token).await.error(ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn the_limit_on_challenge_requests_counts_per_address() {
    let server = TestServer::start_with(|config| {
        config.limits.challenge_requests.attempts = 3;
    })
    .await;
    let key = DeviceKey::generate();
    for _ in 0..3 {
        server.challenge(&key, ChallengeOperation::Join).await;
    }
    let refused = server
        .challenge_reply(&key.public(), ChallengeOperation::Join)
        .await;
    refused.error(ErrorCode::RateLimited);
    assert!(refused.retry_after.is_some());
    server.clock.advance(Duration::seconds(61));
    server.challenge(&key, ChallengeOperation::Join).await;
}

async fn challenge_from(server: &TestServer, prefix: u32) -> crate::support::Reply {
    let body = json!({ "public_key": DeviceKey::generate().public(), "operation": "join" });
    let address = format!("2001:db8:{prefix:x}::1");
    server
        .send(
            Method::POST,
            "/api/v1/auth/challenges",
            None,
            Some(&body),
            Some(&address),
        )
        .await
}

#[tokio::test]
async fn outstanding_challenges_have_a_ceiling_for_the_whole_server() {
    let server = TestServer::start_with(|config| {
        behind_a_proxy(config);
        config.limits.most_outstanding_challenges = 20;
    })
    .await;
    for index in 0..20 {
        challenge_from(&server, index)
            .await
            .ok::<Challenge>(StatusCode::CREATED);
    }
    for index in 20..40 {
        let refused = challenge_from(&server, index).await;
        refused.error(ErrorCode::ChallengesFull);
        assert_eq!(refused.retry_after.as_deref(), Some("60"));
    }
    let stored: i64 = sqlx::query_scalar("select count(*) from challenges")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored, 20);

    server.clock.advance(Duration::seconds(61));
    challenge_from(&server, 99)
        .await
        .ok::<Challenge>(StatusCode::CREATED);
}
