//! The routes of the administration that the other modules do not hold: deletions of the
//! structure (ADR 0308), archived channels (ADR 0410), retention (ADR 0409), exports
//! (ADR 0411) and access tokens (ADR 0210).

use junto_protocol::access_token::{
    AccessTokenCreated, AccessTokenPage, AccessTokenQuery, CreateAccessToken,
};
use junto_protocol::discovery::Retention;
use junto_protocol::export::Export;
use junto_protocol::ids::{
    AccessTokenId, CategoryId, ChannelId, ConversationId, ExportId, ServerRoleId, WorkspaceId,
    WorkspaceRoleId,
};
use junto_protocol::structure::{ArchivePeriod, ArchivedChannel, Channel};

use super::client::read_json;
use super::error::ApiError;
use super::messages::FileBody;
use super::session::ServerApi;
use super::transport::Method;

impl ServerApi {
    // Deletions of the structure.

    /// Deletes a workspace with its members, roles, categories and channels. The server
    /// refuses the default workspace, and a workspace with permanent messages for everyone
    /// except the server owner (ADR 0308).
    pub async fn delete_workspace(&self, workspace_id: WorkspaceId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/workspaces/{workspace_id}"))
            .await
    }

    /// Deletes a category. Its channels stay and have no category.
    pub async fn delete_category(
        &self,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
    ) -> Result<(), ApiError> {
        let path = format!("/workspaces/{workspace_id}/categories/{category_id}");
        self.act(Method::Delete, path).await
    }

    pub async fn delete_workspace_role(
        &self,
        workspace_id: WorkspaceId,
        role_id: WorkspaceRoleId,
    ) -> Result<(), ApiError> {
        let path = format!("/workspaces/{workspace_id}/roles/{role_id}");
        self.act(Method::Delete, path).await
    }

    pub async fn delete_server_role(&self, role_id: ServerRoleId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/server-roles/{role_id}"))
            .await
    }

    // Archived channels.

    /// Archives a channel: it is read-only and leaves the channel list (ADR 0410).
    pub async fn archive_channel(
        &self,
        channel_id: ChannelId,
    ) -> Result<ArchivedChannel, ApiError> {
        let path = format!("/channels/{channel_id}/archive");
        read_json(&self.call(Method::Put, path, Vec::new(), None).await?)
    }

    pub async fn restore_channel(&self, channel_id: ChannelId) -> Result<Channel, ApiError> {
        let path = format!("/channels/{channel_id}/archive");
        read_json(&self.call(Method::Delete, path, Vec::new(), None).await?)
    }

    /// The archived channels of a workspace that the account sees.
    pub async fn archived_channels(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<ArchivedChannel>, ApiError> {
        self.get(format!("/workspaces/{workspace_id}/archived-channels"))
            .await
    }

    /// The time after which the server deletes an archived channel of the workspace.
    pub async fn archive_period(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<ArchivePeriod, ApiError> {
        self.get(format!("/workspaces/{workspace_id}/archive-period"))
            .await
    }

    pub async fn set_archive_period(
        &self,
        workspace_id: WorkspaceId,
        period: ArchivePeriod,
    ) -> Result<ArchivePeriod, ApiError> {
        let path = format!("/workspaces/{workspace_id}/archive-period");
        self.send(Method::Put, path, &period).await
    }

    // Retention. Absent days: nothing is removed by age.

    /// After how many days the server removes the files of the messages of a workspace.
    pub async fn file_retention(&self, workspace_id: WorkspaceId) -> Result<Retention, ApiError> {
        self.get(format!("/workspaces/{workspace_id}/file-retention"))
            .await
    }

    pub async fn set_file_retention(
        &self,
        workspace_id: WorkspaceId,
        retention: &Retention,
    ) -> Result<Retention, ApiError> {
        let path = format!("/workspaces/{workspace_id}/file-retention");
        self.send(Method::Put, path, retention).await
    }

    /// After how many days the server removes the recordings of a workspace.
    pub async fn recording_retention(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Retention, ApiError> {
        self.get(format!("/workspaces/{workspace_id}/recording-retention"))
            .await
    }

    pub async fn set_recording_retention(
        &self,
        workspace_id: WorkspaceId,
        retention: &Retention,
    ) -> Result<Retention, ApiError> {
        let path = format!("/workspaces/{workspace_id}/recording-retention");
        self.send(Method::Put, path, retention).await
    }

    /// After how many days the server removes the files of direct messages.
    pub async fn direct_message_retention(&self) -> Result<Retention, ApiError> {
        self.get("/direct-message-retention".to_owned()).await
    }

    pub async fn set_direct_message_retention(
        &self,
        retention: &Retention,
    ) -> Result<Retention, ApiError> {
        let path = "/direct-message-retention".to_owned();
        self.send(Method::Put, path, retention).await
    }

    // Exports.

    /// Asks for an export of a channel. While an export of the channel waits or is
    /// written for the account, the server returns that export (ADR 0411).
    pub async fn export_channel(&self, channel_id: ChannelId) -> Result<Export, ApiError> {
        let path = format!("/channels/{channel_id}/exports");
        read_json(&self.call(Method::Post, path, Vec::new(), None).await?)
    }

    /// Asks for an export of a direct conversation of the account.
    pub async fn export_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Export, ApiError> {
        let path = format!("/conversations/{conversation_id}/exports");
        read_json(&self.call(Method::Post, path, Vec::new(), None).await?)
    }

    /// The exports of the account.
    pub async fn exports(&self) -> Result<Vec<Export>, ApiError> {
        self.get("/exports".to_owned()).await
    }

    pub async fn export(&self, export_id: ExportId) -> Result<Export, ApiError> {
        self.get(format!("/exports/{export_id}")).await
    }

    /// Removes an export of the account with its file.
    pub async fn remove_export(&self, export_id: ExportId) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/exports/{export_id}"))
            .await
    }

    /// The file of an export from the byte `from` on. The server ends the answer when the
    /// access of the account to the export ends (ADR 0411).
    pub async fn export_file(&self, export_id: ExportId, from: u64) -> Result<FileBody, ApiError> {
        let opened = self
            .open(format!("/exports/{export_id}/file"), from)
            .await?;
        Ok(FileBody {
            partial: opened.status == 206,
            body: opened.body,
        })
    }

    // Access tokens.

    /// Creates an access token. The answer is the only one that carries the token
    /// (ADR 0210).
    pub async fn create_access_token(
        &self,
        token: &CreateAccessToken,
    ) -> Result<AccessTokenCreated, ApiError> {
        self.send(Method::Post, "/access-tokens".to_owned(), token)
            .await
    }

    /// One page of the access tokens that have not expired, the newest first: every token
    /// for an administrator, and the own tokens for every other account.
    pub async fn access_tokens(
        &self,
        query: &AccessTokenQuery,
    ) -> Result<AccessTokenPage, ApiError> {
        let mut pairs = Vec::new();
        if let Some(before) = query.before {
            pairs.push(("before", before.to_string()));
        }
        if let Some(limit) = query.limit {
            pairs.push(("limit", limit.to_string()));
        }
        let path = "/access-tokens".to_owned();
        read_json(&self.call(Method::Get, path, pairs, None).await?)
    }

    pub async fn revoke_access_token(
        &self,
        access_token_id: AccessTokenId,
    ) -> Result<(), ApiError> {
        self.act(Method::Delete, format!("/access-tokens/{access_token_id}"))
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures_util::StreamExt as _;
    use junto_protocol::access_token::{AccessTokenInfo, TokenAction};
    use junto_protocol::export::ExportState;
    use junto_protocol::ids::{AccountId, ServerId};
    use junto_protocol::message::MessagePlace;
    use junto_protocol::secret::AccessToken;
    use junto_protocol::structure::ChannelKind;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use time::OffsetDateTime;

    use super::*;
    use crate::address::ServerAddress;
    use crate::api::transport::{
        HttpRequest, HttpResponse, OpenFuture, OpenedResponse, Transport, TransportFuture,
    };
    use crate::api::{ApiClient, RetryPolicy};
    use crate::secret::SecretToken;
    use crate::version::SupportedVersions;

    /// Answers every request with one body and keeps the requests.
    struct Recorded {
        body: Vec<u8>,
        seen: Mutex<Vec<HttpRequest>>,
        /// The byte from which each file was asked for.
        from: Mutex<Vec<u64>>,
    }

    impl Transport for Recorded {
        fn send(&self, request: HttpRequest) -> TransportFuture {
            self.seen.lock().unwrap().push(request);
            let body = self.body.clone();
            Box::pin(async move {
                Ok(HttpResponse {
                    status: 200,
                    retry_after: None,
                    body: body.into(),
                })
            })
        }

        fn open(&self, request: HttpRequest, from: u64) -> OpenFuture {
            self.seen.lock().unwrap().push(request);
            self.from.lock().unwrap().push(from);
            let status = if from > 0 { 206 } else { 200 };
            let body = bytes::Bytes::from(self.body.clone());
            Box::pin(async move {
                Ok(OpenedResponse {
                    status,
                    retry_after: None,
                    body: Box::pin(futures_util::stream::once(async move { Ok(body) })),
                })
            })
        }
    }

    impl Recorded {
        /// The method, the path and the query of the one request that arrived.
        fn request(&self) -> (Method, String, Option<String>) {
            let seen = self.seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "{seen:?}");
            assert!(seen[0].bearer.is_some());
            let url = &seen[0].url;
            (
                seen[0].method,
                url.path().to_owned(),
                url.query().map(str::to_owned),
            )
        }

        /// The body of the one request that arrived.
        fn sent<B: DeserializeOwned>(&self) -> Option<B> {
            let seen = self.seen.lock().unwrap();
            let body = seen[0].body.as_ref()?;
            assert_eq!(body.media_type, "application/json");
            Some(serde_json::from_slice(body.as_slice()).unwrap())
        }
    }

    /// A server that answers every request with `answer`.
    fn server<A: Serialize>(answer: &A) -> (Arc<Recorded>, ServerApi) {
        let transport = Arc::new(Recorded {
            body: serde_json::to_vec(answer).unwrap(),
            seen: Mutex::default(),
            from: Mutex::default(),
        });
        let api = ApiClient::new(
            transport.clone(),
            ServerAddress::parse("chat.example.test").unwrap(),
            ServerId::generate(),
            SupportedVersions::THIS_BUILD.negotiate(&[1]).unwrap(),
            RetryPolicy::default(),
        )
        .with_token(SecretToken::new("jst_test".to_owned()));
        (transport, api)
    }

    fn get(path: String) -> (Method, String, Option<String>) {
        (Method::Get, format!("/api/v1{path}"), None)
    }

    fn put(path: String) -> (Method, String, Option<String>) {
        (Method::Put, format!("/api/v1{path}"), None)
    }

    fn post(path: String) -> (Method, String, Option<String>) {
        (Method::Post, format!("/api/v1{path}"), None)
    }

    fn delete(path: String) -> (Method, String, Option<String>) {
        (Method::Delete, format!("/api/v1{path}"), None)
    }

    fn at() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_900_000_000).unwrap()
    }

    fn channel() -> Channel {
        Channel {
            id: ChannelId::generate(),
            workspace_id: WorkspaceId::generate(),
            category_id: None,
            kind: ChannelKind::Text,
            name: "general".to_owned(),
            position: 0,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        }
    }

    fn export() -> Export {
        Export {
            id: ExportId::generate(),
            place: MessagePlace::Channel {
                workspace_id: WorkspaceId::generate(),
                channel_id: ChannelId::generate(),
            },
            state: ExportState::Ready,
            failure: None,
            requested_at: at(),
            finished_at: Some(at()),
            expires_at: Some(at() + time::Duration::days(7)),
            bytes: Some(2048),
        }
    }

    fn token() -> AccessTokenInfo {
        AccessTokenInfo {
            id: AccessTokenId::generate(),
            name: "setup".to_owned(),
            created_by: AccountId::generate(),
            actions: vec![TokenAction::ReadStructure, TokenAction::ReadAuditLog],
            created_at: at(),
            expires_at: at() + time::Duration::days(30),
        }
    }

    #[tokio::test]
    async fn each_deletion_of_the_structure_is_a_delete_request_for_its_object() {
        let workspace_id = WorkspaceId::generate();
        let (seen, api) = server(&());
        api.delete_workspace(workspace_id).await.unwrap();
        assert_eq!(
            seen.request(),
            delete(format!("/workspaces/{workspace_id}"))
        );
        assert!(seen.sent::<serde_json::Value>().is_none());

        let category_id = CategoryId::generate();
        let (seen, api) = server(&());
        api.delete_category(workspace_id, category_id)
            .await
            .unwrap();
        assert_eq!(
            seen.request(),
            delete(format!(
                "/workspaces/{workspace_id}/categories/{category_id}"
            ))
        );

        let role_id = WorkspaceRoleId::generate();
        let (seen, api) = server(&());
        api.delete_workspace_role(workspace_id, role_id)
            .await
            .unwrap();
        assert_eq!(
            seen.request(),
            delete(format!("/workspaces/{workspace_id}/roles/{role_id}"))
        );

        let role_id = ServerRoleId::generate();
        let (seen, api) = server(&());
        api.delete_server_role(role_id).await.unwrap();
        assert_eq!(seen.request(), delete(format!("/server-roles/{role_id}")));
    }

    #[tokio::test]
    async fn the_archive_routes_name_the_channel_or_the_workspace_and_read_their_answers() {
        let archived = ArchivedChannel {
            channel: channel(),
            archived_at: at(),
            deletes_at: Some(at() + time::Duration::days(30)),
        };
        let channel_id = archived.channel.id;
        let workspace_id = archived.channel.workspace_id;

        let (seen, api) = server(&archived);
        assert_eq!(api.archive_channel(channel_id).await.unwrap(), archived);
        assert_eq!(
            seen.request(),
            put(format!("/channels/{channel_id}/archive"))
        );
        assert!(seen.sent::<serde_json::Value>().is_none());

        let (seen, api) = server(&archived.channel);
        assert_eq!(
            api.restore_channel(channel_id).await.unwrap(),
            archived.channel
        );
        assert_eq!(
            seen.request(),
            delete(format!("/channels/{channel_id}/archive"))
        );

        // A channel that the server never deletes has no time of deletion.
        let kept = ArchivedChannel {
            deletes_at: None,
            ..archived.clone()
        };
        let list = vec![archived, kept];
        let (seen, api) = server(&list);
        assert_eq!(api.archived_channels(workspace_id).await.unwrap(), list);
        assert_eq!(
            seen.request(),
            get(format!("/workspaces/{workspace_id}/archived-channels"))
        );

        let period = ArchivePeriod { days: Some(30) };
        let (seen, api) = server(&period);
        assert_eq!(api.archive_period(workspace_id).await.unwrap(), period);
        assert_eq!(
            seen.request(),
            get(format!("/workspaces/{workspace_id}/archive-period"))
        );

        let never = ArchivePeriod { days: None };
        let (seen, api) = server(&never);
        assert_eq!(
            api.set_archive_period(workspace_id, never).await.unwrap(),
            never
        );
        assert_eq!(
            seen.request(),
            put(format!("/workspaces/{workspace_id}/archive-period"))
        );
        assert_eq!(seen.sent::<ArchivePeriod>(), Some(never));
    }

    #[tokio::test]
    async fn the_retention_routes_name_the_workspace_or_the_direct_messages() {
        let workspace_id = WorkspaceId::generate();
        let days = Retention { days: Some(90) };
        let never = Retention { days: None };

        let (seen, api) = server(&days);
        assert_eq!(api.file_retention(workspace_id).await.unwrap(), days);
        assert_eq!(
            seen.request(),
            get(format!("/workspaces/{workspace_id}/file-retention"))
        );
        let (seen, api) = server(&never);
        assert_eq!(
            api.set_file_retention(workspace_id, &never).await.unwrap(),
            never
        );
        assert_eq!(
            seen.request(),
            put(format!("/workspaces/{workspace_id}/file-retention"))
        );
        assert_eq!(seen.sent::<Retention>(), Some(never.clone()));

        let (seen, api) = server(&never);
        assert_eq!(api.recording_retention(workspace_id).await.unwrap(), never);
        assert_eq!(
            seen.request(),
            get(format!("/workspaces/{workspace_id}/recording-retention"))
        );
        let (seen, api) = server(&days);
        assert_eq!(
            api.set_recording_retention(workspace_id, &days)
                .await
                .unwrap(),
            days
        );
        assert_eq!(
            seen.request(),
            put(format!("/workspaces/{workspace_id}/recording-retention"))
        );
        assert_eq!(seen.sent::<Retention>(), Some(days.clone()));

        let (seen, api) = server(&days);
        assert_eq!(api.direct_message_retention().await.unwrap(), days);
        assert_eq!(seen.request(), get("/direct-message-retention".to_owned()));
        let (seen, api) = server(&never);
        assert_eq!(
            api.set_direct_message_retention(&never).await.unwrap(),
            never
        );
        assert_eq!(seen.request(), put("/direct-message-retention".to_owned()));
        assert_eq!(seen.sent::<Retention>(), Some(never));
    }

    #[tokio::test]
    async fn the_export_routes_name_the_place_or_the_export_and_read_their_answers() {
        let export = export();
        let export_id = export.id;
        let channel_id = ChannelId::generate();
        let (seen, api) = server(&export);
        assert_eq!(api.export_channel(channel_id).await.unwrap(), export);
        assert_eq!(
            seen.request(),
            post(format!("/channels/{channel_id}/exports"))
        );
        assert!(seen.sent::<serde_json::Value>().is_none());

        let conversation_id = ConversationId::generate();
        let direct = Export {
            place: MessagePlace::Direct {
                conversation_id,
                low_account_id: AccountId::generate(),
                high_account_id: AccountId::generate(),
            },
            state: ExportState::Waiting,
            finished_at: None,
            expires_at: None,
            bytes: None,
            ..export.clone()
        };
        let (seen, api) = server(&direct);
        assert_eq!(
            api.export_conversation(conversation_id).await.unwrap(),
            direct
        );
        assert_eq!(
            seen.request(),
            post(format!("/conversations/{conversation_id}/exports"))
        );

        let list = vec![export.clone(), direct];
        let (seen, api) = server(&list);
        assert_eq!(api.exports().await.unwrap(), list);
        assert_eq!(seen.request(), get("/exports".to_owned()));

        let (seen, api) = server(&export);
        assert_eq!(api.export(export_id).await.unwrap(), export);
        assert_eq!(seen.request(), get(format!("/exports/{export_id}")));

        let (seen, api) = server(&());
        api.remove_export(export_id).await.unwrap();
        assert_eq!(seen.request(), delete(format!("/exports/{export_id}")));
    }

    #[tokio::test]
    async fn the_file_of_an_export_is_asked_for_from_a_byte_and_read_as_it_arrives() {
        let export_id = ExportId::generate();
        for (from, partial) in [(0, false), (1024, true)] {
            let (seen, api) = server(&"the file");
            let mut file = api.export_file(export_id, from).await.unwrap();
            assert_eq!(file.partial, partial);
            assert_eq!(seen.request(), get(format!("/exports/{export_id}/file")));
            assert_eq!(*seen.from.lock().unwrap(), [from]);
            let mut received = Vec::new();
            while let Some(part) = file.body.next().await {
                received.extend_from_slice(&part.unwrap());
            }
            assert_eq!(received, seen.body);
        }
    }

    #[tokio::test]
    async fn the_access_token_routes_send_the_token_to_create_and_the_page_that_is_asked_for() {
        let created = AccessTokenCreated {
            token: AccessToken::new("jat_secret"),
            access_token: token(),
        };
        let wanted = CreateAccessToken {
            name: created.access_token.name.clone(),
            actions: created.access_token.actions.clone(),
            expires_at: created.access_token.expires_at,
        };
        let (seen, api) = server(&created);
        assert_eq!(api.create_access_token(&wanted).await.unwrap(), created);
        assert_eq!(seen.request(), post("/access-tokens".to_owned()));
        assert_eq!(seen.sent::<CreateAccessToken>(), Some(wanted));

        let older = token();
        let page = AccessTokenPage {
            access_tokens: vec![created.access_token.clone()],
            next_before: Some(created.access_token.id),
        };
        let (seen, api) = server(&page);
        let first = api.access_tokens(&AccessTokenQuery::default()).await;
        assert_eq!(first.unwrap(), page);
        assert_eq!(seen.request(), get("/access-tokens".to_owned()));

        let (seen, api) = server(&page);
        let query = AccessTokenQuery {
            before: Some(older.id),
            limit: Some(200),
        };
        assert_eq!(api.access_tokens(&query).await.unwrap(), page);
        assert_eq!(
            seen.request(),
            (
                Method::Get,
                "/api/v1/access-tokens".to_owned(),
                Some(format!("before={}&limit=200", older.id))
            )
        );

        let (seen, api) = server(&());
        api.revoke_access_token(older.id).await.unwrap();
        assert_eq!(
            seen.request(),
            delete(format!("/access-tokens/{}", older.id))
        );
    }
}
