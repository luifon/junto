//! The signed tokens of the media server: JSON Web Tokens with HMAC-SHA256, as LiveKit reads
//! and writes them.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};

use crate::media::{JoinGrant, Notification};

const HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

/// The time by which the clocks of the two servers may differ, and by which the media server
/// accepts a token after its stated end (ADR 0506).
pub const TOLERANCE: Duration = Duration::seconds(60);

fn mac(secret: &[u8], signed: &str) -> Option<Hmac<Sha256>> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).ok()?;
    mac.update(signed.as_bytes());
    Some(mac)
}

pub fn sign(secret: &[u8], claims: &Value) -> Option<String> {
    let header = URL_SAFE_NO_PAD.encode(HEADER);
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).ok()?);
    let signed = format!("{header}.{payload}");
    let signature = mac(secret, &signed)?.finalize().into_bytes();
    Some(format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

/// The claims of a token that `secret` signed with HMAC-SHA256. The times of the token are
/// not checked here.
pub fn verify(secret: &[u8], token: &str) -> Option<Value> {
    let (signed, signature) = token.rsplit_once('.')?;
    let (header, payload) = signed.split_once('.')?;
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    mac(secret, signed)?.verify_slice(&signature).ok()?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

/// The claims of a join token: one room, one identity, the display name of the participant,
/// and the sources that the participant may publish. The participant may subscribe, and may
/// send no data messages.
pub fn join_claims(key: &str, grant: &JoinGrant<'_>) -> Value {
    let sources: Vec<&str> = grant
        .publishing
        .sources
        .iter()
        .map(|source| source.token_name())
        .collect();
    json!({
        "iss": key,
        "sub": grant.identity,
        "name": grant.name,
        "nbf": grant.now.unix_timestamp(),
        "exp": (grant.now + grant.lifetime).unix_timestamp(),
        "video": {
            "room": grant.room.as_str(),
            "roomJoin": true,
            "canSubscribe": true,
            "canPublish": true,
            "canPublishData": false,
            "canPublishSources": sources,
            "canUpdateOwnMetadata": false,
        },
    })
}

/// What one request of the server to the API of the media server may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiGrant<'a> {
    /// List the rooms.
    RoomList,
    /// Read and change one room.
    RoomAdmin(&'a str),
    /// Start, stop and read the jobs of the recorder.
    Record,
}

/// The claims of a token for one request of the server to the API of the media server.
pub fn api_claims(key: &str, grant: ApiGrant<'_>, now: OffsetDateTime) -> Value {
    let grant = match grant {
        ApiGrant::RoomAdmin(room) => json!({ "roomAdmin": true, "room": room }),
        ApiGrant::RoomList => json!({ "roomList": true }),
        ApiGrant::Record => json!({ "roomRecord": true }),
    };
    json!({
        "iss": key,
        "nbf": (now - TOLERANCE).unix_timestamp(),
        "exp": (now + TOLERANCE).unix_timestamp(),
        "video": grant,
    })
}

/// Verifies a notification of the media server: the token is signed with the secret of the
/// key that it names, it has not ended, and it carries the SHA-256 hash of exactly `body`.
pub fn notification(
    key: &str,
    secret: &[u8],
    authorization: &str,
    body: &[u8],
    now: OffsetDateTime,
) -> Option<Notification> {
    let token = authorization
        .strip_prefix("Bearer ")
        .unwrap_or(authorization)
        .trim();
    let claims = verify(secret, token)?;
    if claims.get("iss").and_then(Value::as_str) != Some(key) {
        return None;
    }
    let ends = claims.get("exp").and_then(Value::as_i64)?;
    if now.unix_timestamp() > ends.saturating_add(TOLERANCE.whole_seconds()) {
        return None;
    }
    let stated = claims.get("sha256").and_then(Value::as_str)?;
    let stated = STANDARD.decode(stated).ok()?;
    let hash = Sha256::digest(body);
    if !crate::secrets::constant_time_eq(&stated, &hash) {
        return None;
    }
    let key = notification_key(&hash, ends);
    let body: Value = serde_json::from_slice(body).ok()?;
    let valid_until = OffsetDateTime::from_unix_timestamp(ends).ok()? + TOLERANCE;
    Some(Notification {
        event: body.get("event")?.as_str()?.to_owned(),
        room: [
            "/room/name",
            "/egressInfo/roomName",
            "/egress_info/room_name",
        ]
        .into_iter()
        .find_map(|pointer| body.pointer(pointer))
        .and_then(Value::as_str)
        .map(str::to_owned),
        valid_until,
        key,
    })
}

fn notification_key(body_hash: &[u8], ends: i64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(body_hash);
    hasher.update(ends.to_be_bytes());
    hasher.finalize().into()
}

/// Signs a notification as the media server does. Used by the fake media server and by
/// tests.
pub fn signed_notification(key: &str, secret: &[u8], body: &[u8], now: OffsetDateTime) -> String {
    let claims = json!({
        "iss": key,
        "nbf": now.unix_timestamp(),
        "exp": (now + Duration::minutes(5)).unix_timestamp(),
        "sha256": STANDARD.encode(Sha256::digest(body)),
    });
    sign(secret, &claims).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{Publishing, RoomName};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    #[test]
    fn a_known_token_has_the_known_signature() {
        let token = sign(b"secret", &json!({ "sub": "1234567890" })).unwrap();
        assert_eq!(
            token,
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.\
             Rq8IxqeX7eA6GgYxlcHdPFVRNFFZc5rEI3MQTZZbK3I"
        );
        assert_eq!(
            verify(b"secret", &token).unwrap(),
            json!({ "sub": "1234567890" })
        );
    }

    #[test]
    fn a_token_is_verified_with_its_secret_and_its_algorithm() {
        let token = sign(b"secret", &json!({ "sub": "seat" })).unwrap();
        assert_eq!(verify(b"other", &token), None);
        let (signed, signature) = token.rsplit_once('.').unwrap();
        let (_, payload) = signed.split_once('.').unwrap();
        let unsigned_header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        assert_eq!(
            verify(
                b"secret",
                &format!("{unsigned_header}.{payload}.{signature}")
            ),
            None
        );
        assert_eq!(
            verify(b"secret", &format!("{unsigned_header}.{payload}.")),
            None
        );
        let other_payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"other"}"#);
        let header = signed.split_once('.').unwrap().0;
        assert_eq!(
            verify(b"secret", &format!("{header}.{other_payload}.{signature}")),
            None
        );
        for malformed in ["", "a", "a.b", "a.b.c", "..", &token[..token.len() - 2]] {
            assert_eq!(verify(b"secret", malformed), None, "{malformed}");
        }
    }

    #[test]
    fn a_join_token_names_one_room_one_identity_and_the_sources() {
        let room = RoomName::generate("0123456789ab").unwrap();
        let publishing = Publishing::pictures_only();
        let grant = JoinGrant {
            room: &room,
            identity: "account:seat",
            name: "Ana Lima",
            publishing: &publishing,
            now: now(),
            lifetime: Duration::seconds(60),
        };
        let claims = join_claims("key", &grant);
        assert_eq!(claims["iss"], "key");
        assert_eq!(claims["sub"], "account:seat");
        assert_eq!(claims["name"], "Ana Lima");
        assert_eq!(claims["exp"], 1_800_000_060);
        assert_eq!(claims["video"]["room"], room.as_str());
        assert_eq!(claims["video"]["roomJoin"], true);
        assert_eq!(
            claims["video"]["canPublishSources"],
            json!(["camera", "screen_share"])
        );
        assert_eq!(claims["video"]["canPublishData"], false);
        assert_eq!(claims["video"].get("roomAdmin"), None);
        assert_eq!(claims["video"].get("roomCreate"), None);
    }

    #[test]
    fn a_notification_is_accepted_only_with_a_signature_over_its_body() {
        let body = br#"{"event":"participant_joined","room":{"name":"jr-a-b"}}"#;
        let authorization = signed_notification("key", b"secret", body, now());
        let expected = Notification {
            event: "participant_joined".to_owned(),
            room: Some("jr-a-b".to_owned()),
            valid_until: now() + Duration::minutes(5) + TOLERANCE,
            key: notification_key(
                &Sha256::digest(body),
                (now() + Duration::minutes(5)).unix_timestamp(),
            ),
        };
        assert_eq!(
            notification("key", b"secret", &authorization, body, now()),
            Some(expected.clone())
        );
        assert_eq!(
            notification(
                "key",
                b"secret",
                &format!("Bearer {authorization}"),
                body,
                now()
            ),
            Some(expected)
        );
        let other_body = br#"{"event":"participant_joined","room":{"name":"jr-a-c"}}"#;
        assert_eq!(
            notification("key", b"secret", &authorization, other_body, now()),
            None
        );
        assert_eq!(
            notification("key", b"other", &authorization, body, now()),
            None
        );
        assert_eq!(
            notification("other", b"secret", &authorization, body, now()),
            None
        );
        let late = now() + Duration::minutes(7);
        assert_eq!(
            notification("key", b"secret", &authorization, body, late),
            None
        );
        let without_hash = sign(b"secret", &json!({ "iss": "key", "exp": 1_900_000_000 })).unwrap();
        assert_eq!(
            notification("key", b"secret", &without_hash, body, now()),
            None
        );
        assert_eq!(notification("key", b"secret", "", body, now()), None);
    }

    #[test]
    fn a_notification_of_the_recorder_names_the_room_of_its_job() {
        for body in [
            br#"{"event":"egress_ended","egressInfo":{"egressId":"EG_1","roomName":"jr-a-b"}}"#
                .as_slice(),
            br#"{"event":"egress_ended","egress_info":{"egress_id":"EG_1","room_name":"jr-a-b"}}"#
                .as_slice(),
        ] {
            let authorization = signed_notification("key", b"secret", body, now());
            let read = notification("key", b"secret", &authorization, body, now()).unwrap();
            assert_eq!(read.event, "egress_ended");
            assert_eq!(read.room.as_deref(), Some("jr-a-b"));
        }
    }

    #[test]
    fn a_request_token_grants_one_thing() {
        let admin = api_claims("key", ApiGrant::RoomAdmin("jr-a-b"), now());
        assert_eq!(
            admin["video"],
            json!({ "roomAdmin": true, "room": "jr-a-b" })
        );
        let list = api_claims("key", ApiGrant::RoomList, now());
        assert_eq!(list["video"], json!({ "roomList": true }));
        let record = api_claims("key", ApiGrant::Record, now());
        assert_eq!(record["video"], json!({ "roomRecord": true }));
        assert_eq!(record["iss"], "key");
    }
}
