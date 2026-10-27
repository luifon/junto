//! The routes of the reason of a ban and of a removal (ADR 0106), of the removal of a
//! member from one workspace (ADR 0106), of the filters of the audit log (ADR 0305) and of
//! the logo of the server (ADR 0101).

use junto_protocol::account::{Avatar, ModerationReason};
use junto_protocol::audit::{AuditFilterQuery, AuditPage};
use junto_protocol::ids::{AccountId, WorkspaceId};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use url::Url;

use super::client::{ApiClient, encode, read_json};
use super::error::{ApiError, ErrorCategory};
use super::retry::{self, Repeat, RetryPolicy};
use super::session::{AvatarImage, ServerApi};
use super::transport::{HttpRequest, HttpResponse, Method, RequestBody, Transport};
use crate::profile::{self, InvalidAvatar};
use crate::secret::SecretBytes;

/// The most characters of the reason of a ban or of a removal (ADR 0106).
pub const LONGEST_REASON: usize = 1000;

/// The reason as the server takes it: without the space around it, and absent when
/// nothing is left. A longer text and a control character are refused by the server.
pub fn stated_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned)
}

/// What a moderator states with a ban, a removal from the server or a removal from a
/// workspace: a reason that stays in the audit entry, and a message that the person
/// concerned reads in a private notice (ADR 0106, ADR 0120).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Statement {
    pub reason: Option<String>,
    pub message: Option<String>,
}

impl Statement {
    /// A statement with a reason and no message.
    pub fn reason(reason: &str) -> Self {
        Self {
            reason: Some(reason.to_owned()),
            message: None,
        }
    }
}

/// The body of a ban or of a removal. A text is sent without the space around it, and a
/// text that is empty then is left out. A request without either has no body, as the
/// request had before the fields existed.
fn statement_body(statement: &Statement) -> Result<Option<RequestBody>, ApiError> {
    let stated = ModerationReason {
        reason: stated_reason(statement.reason.as_deref()),
        recipient_message: stated_reason(statement.message.as_deref()),
    };
    if stated.reason.is_none() && stated.recipient_message.is_none() {
        return Ok(None);
    }
    let mut body =
        serde_json::to_value(&stated).map_err(|error| ApiError::Request(error.to_string()))?;
    if let serde_json::Value::Object(fields) = &mut body {
        fields.retain(|_, value| !value.is_null());
    }
    encode(&body).map(Some)
}

fn filters(query: &AuditFilterQuery) -> Result<Vec<(&'static str, String)>, ApiError> {
    let time = |time: OffsetDateTime| {
        time.format(&Rfc3339)
            .map_err(|error| ApiError::Request(error.to_string()))
    };
    let mut pairs = Vec::new();
    if let Some(action) = &query.action {
        pairs.push(("action", action.clone()));
    }
    if let Some(actor_id) = query.actor_id {
        pairs.push(("actor_id", actor_id.to_string()));
    }
    if let Some(target_id) = query.target_id {
        pairs.push(("target_id", target_id.to_string()));
    }
    if let Some(since) = query.since {
        pairs.push(("since", time(since)?));
    }
    if let Some(until) = query.until {
        pairs.push(("until", time(until)?));
    }
    if let Some(before) = query.before {
        pairs.push(("before", before.to_string()));
    }
    if let Some(limit) = query.limit {
        pairs.push(("limit", limit.to_string()));
    }
    Ok(pairs)
}

/// The image of a logo out of an answer. The bounds of an avatar hold for a logo
/// (ADR 0101), and the image is a file of an unknown sender.
fn logo_image(response: &HttpResponse) -> Result<AvatarImage, ApiError> {
    let media_type =
        profile::check_avatar(&response.body).map_err(|invalid| ApiError::InvalidResponse {
            status: response.status,
            reason: match invalid {
                InvalidAvatar::TooLarge => "the logo is too large",
                InvalidAvatar::Unsupported => "the logo is not an image",
            },
        })?;
    Ok(AvatarImage {
        media_type,
        hash: profile::avatar_hash(&response.body),
        bytes: response.body.to_vec(),
    })
}

/// A server without a logo answers `not_found`.
fn logo_or_none(read: Result<HttpResponse, ApiError>) -> Result<Option<AvatarImage>, ApiError> {
    match read {
        Ok(response) => logo_image(&response).map(Some),
        Err(error) if error.category() == Some(ErrorCategory::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Reads the logo of a server from the address that its public information names. The
/// route needs no account (ADR 0101). Absent: the server has no logo.
pub async fn logo_at(
    transport: &dyn Transport,
    url: Url,
    policy: &RetryPolicy,
) -> Result<Option<AvatarImage>, ApiError> {
    let request = HttpRequest {
        method: Method::Get,
        url,
        bearer: None,
        body: None,
    };
    logo_or_none(retry::send(transport, &request, Repeat::WhenUnchanged, policy).await)
}

impl ApiClient {
    /// The logo of the server, read without a session. Absent: the server has none.
    pub async fn server_logo(&self) -> Result<Option<AvatarImage>, ApiError> {
        logo_or_none(
            self.send(
                Method::Get,
                "/server/logo",
                &[],
                None,
                None,
                Repeat::WhenUnchanged,
            )
            .await,
        )
    }
}

impl ServerApi {
    /// Removes an account from the server: its devices, sessions, memberships and roles
    /// end (ADR 0106).
    pub async fn remove_account(&self, account_id: AccountId) -> Result<(), ApiError> {
        self.remove_account_stating(account_id, &Statement::default())
            .await
    }

    /// As [`Self::remove_account`], with a statement. The reason stays in the audit entry;
    /// the message is the notice that the person reads (ADR 0106, ADR 0120).
    pub async fn remove_account_stating(
        &self,
        account_id: AccountId,
        statement: &Statement,
    ) -> Result<(), ApiError> {
        let path = format!("/accounts/{account_id}/removal");
        self.call(Method::Post, path, Vec::new(), statement_body(statement)?)
            .await
            .map(|_| ())
    }

    /// Bans an account: its keys cannot log in or join again (ADR 0106).
    pub async fn ban_account(&self, account_id: AccountId) -> Result<(), ApiError> {
        self.ban_account_stating(account_id, &Statement::default())
            .await
    }

    /// As [`Self::ban_account`], with a statement as for a removal.
    pub async fn ban_account_stating(
        &self,
        account_id: AccountId,
        statement: &Statement,
    ) -> Result<(), ApiError> {
        let path = format!("/accounts/{account_id}/ban");
        self.call(Method::Put, path, Vec::new(), statement_body(statement)?)
            .await
            .map(|_| ())
    }

    /// Removes an account from one workspace. Its roles of that workspace go with the
    /// membership; its other workspaces and its account stay. The caller is an
    /// administrator with a session and stands above the account in the workspace
    /// (ADR 0106, ADR 0309).
    pub async fn remove_workspace_member(
        &self,
        workspace_id: WorkspaceId,
        account_id: AccountId,
        statement: &Statement,
    ) -> Result<(), ApiError> {
        let path = format!("/workspaces/{workspace_id}/members/{account_id}/removal");
        self.call(Method::Post, path, Vec::new(), statement_body(statement)?)
            .await
            .map(|_| ())
    }

    /// One page of the audit log of the server, the newest first, with the filters of
    /// the query. The next page repeats the filters with `before` (ADR 0305).
    pub async fn server_audit_log(&self, query: &AuditFilterQuery) -> Result<AuditPage, ApiError> {
        let path = "/audit-log".to_owned();
        read_json(&self.call(Method::Get, path, filters(query)?, None).await?)
    }

    /// One page of the audit log of a workspace, as [`Self::server_audit_log`].
    pub async fn workspace_audit_log(
        &self,
        workspace_id: WorkspaceId,
        query: &AuditFilterQuery,
    ) -> Result<AuditPage, ApiError> {
        let path = format!("/workspaces/{workspace_id}/audit-log");
        read_json(&self.call(Method::Get, path, filters(query)?, None).await?)
    }

    /// Sets the logo of the server. The body of the request is the image itself, under
    /// the bounds of an avatar. The caller is an administrator with a session (ADR 0101).
    pub async fn set_server_logo(&self, image: Vec<u8>) -> Result<Avatar, ApiError> {
        let kind = profile::check_avatar(&image)?;
        let body = RequestBody::image(kind, SecretBytes::from_vec(image));
        let path = "/server/logo".to_owned();
        read_json(&self.call(Method::Put, path, Vec::new(), Some(body)).await?)
    }

    pub async fn remove_server_logo(&self) -> Result<(), ApiError> {
        self.act(Method::Delete, "/server/logo".to_owned()).await
    }

    /// The logo of the server. Absent: the server has none.
    pub async fn server_logo(&self) -> Result<Option<AvatarImage>, ApiError> {
        logo_or_none(
            self.call(Method::Get, "/server/logo".to_owned(), Vec::new(), None)
                .await,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use junto_protocol::WorkspacePermission;
    use junto_protocol::account::ImageType;
    use junto_protocol::audit::AuditEvent;
    use junto_protocol::ids::{AuditEntryId, ChannelId, ServerId, WorkspaceRoleId};
    use junto_protocol::structure::{OverrideState, PermissionRule, RuleSubject};
    use serde::Serialize;

    use super::*;
    use crate::address::ServerAddress;
    use crate::api::transport::TransportFuture;
    use crate::secret::SecretToken;
    use crate::version::SupportedVersions;

    /// Answers every request with one status and one body, and keeps the requests.
    struct Recorded {
        status: u16,
        body: Vec<u8>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Transport for Recorded {
        fn send(&self, request: HttpRequest) -> TransportFuture {
            self.seen.lock().unwrap().push(request);
            let (status, body) = (self.status, self.body.clone());
            Box::pin(async move {
                Ok(HttpResponse {
                    status,
                    retry_after: None,
                    body: body.into(),
                    receipt: crate::api::Receipt::NotStated,
                })
            })
        }
    }

    impl Recorded {
        /// The method, the path, the query and the body of the one request that arrived.
        fn request(&self) -> (Method, String, Option<String>, Option<serde_json::Value>) {
            let seen = self.seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "{seen:?}");
            let url = &seen[0].url;
            let body = seen[0].body.as_ref().map(|body| {
                assert_eq!(body.media_type, "application/json");
                serde_json::from_slice(body.as_slice()).unwrap()
            });
            (
                seen[0].method,
                url.path().to_owned(),
                url.query().map(str::to_owned),
                body,
            )
        }
    }

    fn client(transport: Arc<Recorded>) -> ApiClient {
        ApiClient::new(
            transport,
            ServerAddress::parse("chat.example.test").unwrap(),
            ServerId::generate(),
            SupportedVersions::THIS_BUILD.negotiate(&[1]).unwrap(),
            RetryPolicy::default(),
        )
    }

    fn bytes(status: u16, body: Vec<u8>) -> (Arc<Recorded>, ServerApi) {
        let transport = Arc::new(Recorded {
            status,
            body,
            seen: Mutex::default(),
        });
        let api = client(transport.clone()).with_token(SecretToken::new("jst_test".to_owned()));
        (transport, api)
    }

    fn server<A: Serialize>(answer: &A) -> (Arc<Recorded>, ServerApi) {
        bytes(200, serde_json::to_vec(answer).unwrap())
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";

    #[tokio::test]
    async fn a_ban_and_a_removal_send_their_reason_and_message_and_no_body_without_one() {
        let account_id = AccountId::generate();
        let workspace_id = WorkspaceId::generate();
        let stated = serde_json::json!({ "reason": "Spam in #general" });

        let (seen, api) = server(&());
        api.ban_account_stating(account_id, &Statement::reason("  Spam in #general \n"))
            .await
            .unwrap();
        assert_eq!(
            seen.request(),
            (
                Method::Put,
                format!("/api/v1/accounts/{account_id}/ban"),
                None,
                Some(stated.clone())
            )
        );

        let (seen, api) = server(&());
        api.remove_account_stating(account_id, &Statement::reason("Spam in #general"))
            .await
            .unwrap();
        assert_eq!(
            seen.request(),
            (
                Method::Post,
                format!("/api/v1/accounts/{account_id}/removal"),
                None,
                Some(stated.clone())
            )
        );

        let (seen, api) = server(&());
        api.remove_workspace_member(
            workspace_id,
            account_id,
            &Statement::reason("Spam in #general"),
        )
        .await
        .unwrap();
        assert_eq!(
            seen.request(),
            (
                Method::Post,
                format!("/api/v1/workspaces/{workspace_id}/members/{account_id}/removal"),
                None,
                Some(stated)
            )
        );

        // A message to the person goes in its own field, also without a reason.
        let both = Statement {
            reason: Some("Spam".to_owned()),
            message: Some(" Please read the rules. ".to_owned()),
        };
        let (seen, api) = server(&());
        api.ban_account_stating(account_id, &both).await.unwrap();
        assert_eq!(
            seen.request().3,
            Some(serde_json::json!({
                "reason": "Spam",
                "recipient_message": "Please read the rules."
            }))
        );
        let message_only = Statement {
            reason: Some("  ".to_owned()),
            message: Some("Please read the rules.".to_owned()),
        };
        let (seen, api) = server(&());
        api.remove_workspace_member(workspace_id, account_id, &message_only)
            .await
            .unwrap();
        assert_eq!(
            seen.request().3,
            Some(serde_json::json!({ "recipient_message": "Please read the rules." }))
        );

        // No text, and texts of spaces only: the request of before, without a body.
        for statement in [
            Statement::default(),
            Statement {
                reason: Some("  \n ".to_owned()),
                message: Some(" ".to_owned()),
            },
        ] {
            let (seen, api) = server(&());
            api.ban_account_stating(account_id, &statement)
                .await
                .unwrap();
            assert_eq!(seen.request().3, None);
            let (seen, api) = server(&());
            api.remove_account_stating(account_id, &statement)
                .await
                .unwrap();
            assert_eq!(seen.request().3, None);
            let (seen, api) = server(&());
            api.remove_workspace_member(workspace_id, account_id, &statement)
                .await
                .unwrap();
            assert_eq!(seen.request().3, None);
        }
    }

    #[tokio::test]
    async fn a_change_of_one_permission_of_a_rule_names_its_subject_the_permission_and_its_state() {
        let (channel_id, role_id) = (ChannelId::generate(), WorkspaceRoleId::generate());
        let account_id = AccountId::generate();
        for (subject, part) in [
            (RuleSubject::Role { role_id }, format!("roles/{role_id}")),
            (
                RuleSubject::Person { account_id },
                format!("people/{account_id}"),
            ),
        ] {
            let held = PermissionRule {
                subject,
                allow: vec![WorkspacePermission::LockRoom],
                deny: vec![WorkspacePermission::MuteMembers],
            };
            for (state, name) in [
                (OverrideState::Allow, "allow"),
                (OverrideState::Deny, "deny"),
                (OverrideState::Inherit, "inherit"),
            ] {
                let (seen, api) = server(&held);
                let answer = api
                    .change_channel_rule(
                        channel_id,
                        subject,
                        WorkspacePermission::MuteMembers,
                        state,
                    )
                    .await
                    .unwrap();
                assert_eq!(answer, held);
                assert_eq!(
                    seen.request(),
                    (
                        Method::Patch,
                        format!("/api/v1/channels/{channel_id}/rules/{part}"),
                        None,
                        Some(serde_json::json!({ "permission": "mute_members", "state": name }))
                    )
                );
            }
        }
    }

    #[tokio::test]
    async fn an_audit_page_names_the_filters_that_are_set() {
        let page = AuditPage {
            entries: Vec::new(),
            next_before: None,
        };
        let (seen, api) = server(&page);
        api.server_audit_log(&AuditFilterQuery::default())
            .await
            .unwrap();
        assert_eq!(
            seen.request(),
            (Method::Get, "/api/v1/audit-log".to_owned(), None, None)
        );

        let (actor_id, target_id) = (AccountId::generate(), AccountId::generate());
        let before = AuditEntryId::generate();
        let since = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let query = AuditFilterQuery {
            action: Some("account_banned".to_owned()),
            actor_id: Some(actor_id),
            target_id: Some(target_id.as_uuid()),
            since: Some(since),
            until: Some(since + time::Duration::days(1)),
            before: Some(before),
            limit: Some(50),
        };
        let workspace_id = WorkspaceId::generate();
        let (seen, api) = server(&page);
        api.workspace_audit_log(workspace_id, &query).await.unwrap();
        let (method, path, sent, body) = seen.request();
        assert_eq!(method, Method::Get);
        assert_eq!(path, format!("/api/v1/workspaces/{workspace_id}/audit-log"));
        assert_eq!(body, None);
        assert_eq!(
            sent.unwrap(),
            format!(
                "action=account_banned&actor_id={actor_id}&target_id={target_id}\
                 &since=2023-11-14T22%3A13%3A20Z&until=2023-11-15T22%3A13%3A20Z\
                 &before={before}&limit=50"
            )
        );
    }

    #[tokio::test]
    async fn an_audit_page_with_an_action_of_a_newer_server_is_read_whole() {
        let page = br#"{"entries":[
            {"id":"0198f1c2-7d3a-7b1e-9c55-000000000003",
             "occurred_at":"2030-01-02T00:00:00Z",
             "actor_id":"0198f1c2-7d3a-7b1e-9c55-0000000000aa",
             "workspace_id":"0198f1c2-7d3a-7b1e-9c55-0000000000bb",
             "channel_id":null,
             "event":{"action":"glossary_updated","terms":3,"nested":{"a":[1,2]}}},
            {"id":"0198f1c2-7d3a-7b1e-9c55-000000000002",
             "occurred_at":"2030-01-01T00:00:00Z",
             "actor_id":null,"workspace_id":null,"channel_id":null,
             "event":{"action":"owner_claimed",
                      "account_id":"0198f1c2-7d3a-7b1e-9c55-0000000000aa"}}],
            "next_before":"0198f1c2-7d3a-7b1e-9c55-000000000002"}"#;
        let query = AuditFilterQuery::default();
        for log in [
            bytes(200, page.to_vec()).1.server_audit_log(&query).await,
            bytes(200, page.to_vec())
                .1
                .workspace_audit_log(WorkspaceId::generate(), &query)
                .await,
        ] {
            let log = log.unwrap();
            assert_eq!(log.entries.len(), 2);
            let later = &log.entries[0];
            assert_eq!(later.event, AuditEvent::Unknown);
            assert_eq!(later.id.to_string(), "0198f1c2-7d3a-7b1e-9c55-000000000003");
            assert_eq!(later.occurred_at.year(), 2030);
            assert!(later.actor_id.is_some() && later.workspace_id.is_some());
            assert!(matches!(
                log.entries[1].event,
                AuditEvent::OwnerClaimed { .. }
            ));
            assert!(log.next_before.is_some());
        }
    }

    #[tokio::test]
    async fn an_entry_of_a_ban_without_a_reason_field_is_read_as_one_without_a_reason() {
        let page = br#"{"entries":[
            {"id":"0198f1c2-7d3a-7b1e-9c55-000000000003",
             "occurred_at":"2030-01-02T00:00:00Z",
             "actor_id":null,"workspace_id":null,"channel_id":null,
             "event":{"action":"account_banned",
                      "account_id":"0198f1c2-7d3a-7b1e-9c55-0000000000aa",
                      "cancelled_invite_ids":[]}}],
            "next_before":null}"#;
        let log = bytes(200, page.to_vec())
            .1
            .server_audit_log(&AuditFilterQuery::default())
            .await
            .unwrap();
        assert!(matches!(
            &log.entries[0].event,
            AuditEvent::AccountBanned { reason: None, .. }
        ));
    }

    #[tokio::test]
    async fn the_logo_is_sent_as_an_image_read_with_its_hash_and_absent_without_one() {
        let logo = Avatar {
            hash: profile::avatar_hash(PNG),
            media_type: ImageType::Png,
            bytes: PNG.len() as u32,
            width: 64,
            height: 64,
        };
        let (seen, api) = server(&logo);
        assert_eq!(api.set_server_logo(PNG.to_vec()).await.unwrap(), logo);
        {
            let seen = seen.seen.lock().unwrap();
            assert_eq!(seen[0].method, Method::Put);
            assert_eq!(seen[0].url.path(), "/api/v1/server/logo");
            let body = seen[0].body.as_ref().unwrap();
            assert_eq!(body.media_type, "image/png");
            assert_eq!(body.as_slice(), PNG);
        }

        // Bytes that are no image of a server are not sent.
        let (seen, api) = server(&logo);
        assert!(api.set_server_logo(b"<svg/>".to_vec()).await.is_err());
        assert!(seen.seen.lock().unwrap().is_empty());

        let (seen, api) = server(&());
        api.remove_server_logo().await.unwrap();
        assert_eq!(
            seen.request(),
            (Method::Delete, "/api/v1/server/logo".to_owned(), None, None)
        );

        let (seen, api) = bytes(200, PNG.to_vec());
        let read = api.server_logo().await.unwrap().unwrap();
        assert_eq!(read.media_type, ImageType::Png);
        assert_eq!(read.hash, logo.hash);
        assert_eq!(read.bytes, PNG);
        assert_eq!(seen.request().1, "/api/v1/server/logo");

        // The public read carries no session.
        let transport = Arc::new(Recorded {
            status: 200,
            body: PNG.to_vec(),
            seen: Mutex::default(),
        });
        let read = client(transport.clone()).server_logo().await.unwrap();
        assert_eq!(read.unwrap().bytes, PNG);
        assert!(transport.seen.lock().unwrap()[0].bearer.is_none());

        let url = Url::parse("https://chat.example.test/api/v1/server/logo").unwrap();
        let transport = Arc::new(Recorded {
            status: 200,
            body: PNG.to_vec(),
            seen: Mutex::default(),
        });
        let read = logo_at(transport.as_ref(), url.clone(), &RetryPolicy::default())
            .await
            .unwrap();
        assert_eq!(read.unwrap().bytes, PNG);
        assert!(transport.seen.lock().unwrap()[0].bearer.is_none());

        // A server without a logo answers `not_found`.
        let missing = br#"{"error":{"code":"not_found","message":"no logo"}}"#.to_vec();
        let (_, api) = bytes(404, missing.clone());
        assert_eq!(api.server_logo().await.unwrap(), None);
        let transport = Arc::new(Recorded {
            status: 404,
            body: missing,
            seen: Mutex::default(),
        });
        let read = logo_at(transport.as_ref(), url, &RetryPolicy::default()).await;
        assert_eq!(read.unwrap(), None);

        // An answer that is no image is refused.
        let (_, api) = bytes(200, b"<html>".to_vec());
        assert!(matches!(
            api.server_logo().await,
            Err(ApiError::InvalidResponse { .. })
        ));
    }

    #[test]
    fn a_reason_is_trimmed_and_absent_when_nothing_is_left() {
        assert_eq!(stated_reason(None), None);
        assert_eq!(stated_reason(Some("")), None);
        assert_eq!(stated_reason(Some(" \t\n")), None);
        assert_eq!(stated_reason(Some(" a b ")), Some("a b".to_owned()));
    }
}
