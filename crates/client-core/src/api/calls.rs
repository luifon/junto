//! The call routes and the snapshot (ADR 0207, ADR 0510). The server decides who is in a
//! call and gives out the token for the media server; no media passes through these
//! requests.

use junto_protocol::call::{
    AnswerEntryRequest, CallInvitation, CallJoin, CallMove, ChannelCall, EntryRequest,
    InviteToCall, MAX_SHARES, MoveParticipant, MuteDirectConversation, OwnCallView,
    ReportCallState, ReportScreenShare, ReportSpeaking, ShareAudio, ShareKind, UpdateCallSettings,
};
use junto_protocol::ids::{AccountId, CallInvitationId, CallMoveId, ChannelId, EntryRequestId};
use junto_protocol::recording::{Recording, StartRecording};
use junto_protocol::snapshot::SnapshotResponse;
use junto_protocol::structure::Channel;
use time::OffsetDateTime;
use uuid::Uuid;

use super::client::read_json;
use super::error::ApiError;
use super::session::ServerApi;
use super::transport::Method;

impl ServerApi {
    /// The state that the event stream continues from the cursor of the answer (ADR 0207),
    /// with the newest private moderation notices of the account (ADR 0120).
    pub async fn snapshot(&self) -> Result<SnapshotResponse, ApiError> {
        self.get("/snapshot".to_owned()).await
    }

    /// Every request for a place states that this client shows `MAX_SHARES` shares of one
    /// person (ADR 0510).
    async fn join(&self, path: String) -> Result<CallJoin, ApiError> {
        self.join_showing(path, Some(MAX_SHARES)).await
    }

    async fn join_showing(&self, path: String, shown: Option<u8>) -> Result<CallJoin, ApiError> {
        let query = shown
            .map(|shown| vec![("shares", shown.to_string())])
            .unwrap_or_default();
        read_json(&self.call(Method::Post, path, query, None).await?)
    }

    /// A request for a new token of the seat `identity` (ADR 0520): it makes, ends and
    /// takes no seat. A seat that cannot be used is refused with `seat_ended` and the
    /// reason in [`super::Rejection::left`].
    async fn renew(&self, path: String, identity: &str) -> Result<CallJoin, ApiError> {
        let query = vec![
            ("renews", identity.to_owned()),
            ("shares", MAX_SHARES.to_string()),
        ];
        read_json(&self.call(Method::Post, path, query, None).await?)
    }

    // The call of the account.

    /// The call that the account is in, with the requests to enter that wait there.
    pub async fn own_call(&self) -> Result<OwnCallView, ApiError> {
        self.get("/call".to_owned()).await
    }

    /// Leaves the call of the account, from whichever device it is in the call.
    pub async fn leave_call(&self) -> Result<(), ApiError> {
        self.act(Method::Delete, "/call".to_owned()).await
    }

    /// Leaves the place that this device was given with `identity`. A place that another
    /// device or session of the account holds stays, and the answer is the same.
    pub async fn leave_call_as(&self, identity: &str) -> Result<(), ApiError> {
        let query = vec![("identity", identity.to_owned())];
        self.call(Method::Delete, "/call".to_owned(), query, None)
            .await
            .map(|_| ())
    }

    /// Reports the person's own microphone and sound settings. Only the session that
    /// holds the place in the call reports (ADR 0510).
    pub async fn report_call_state(&self, muted: bool, deafened: bool) -> Result<(), ApiError> {
        let report = ReportCallState { muted, deafened };
        self.put("/call/state".to_owned(), &report).await
    }

    /// Reports the share of this device in `slot`, with what it shows and its sound.
    /// `share_id` names the share that was started in the slot: another one is a new share
    /// (ADR 0510).
    pub async fn report_screen_share(
        &self,
        slot: u8,
        kind: Option<ShareKind>,
        audio: ShareAudio,
        share_id: Option<Uuid>,
    ) -> Result<(), ApiError> {
        let report = ReportScreenShare {
            audio,
            slot,
            kind,
            share_id,
        };
        self.put("/call/screen-share".to_owned(), &report).await
    }

    /// Reports the end of the share in `slot`, or of every share of the person without one.
    pub async fn end_screen_share(&self, slot: Option<u8>) -> Result<(), ApiError> {
        let path = match slot {
            Some(slot) => format!("/call/screen-share?slot={slot}"),
            None => "/call/screen-share".to_owned(),
        };
        self.act(Method::Delete, path).await
    }

    pub async fn raise_hand(&self) -> Result<(), ApiError> {
        self.act(Method::Put, "/call/hand".to_owned()).await
    }

    pub async fn lower_hand(&self) -> Result<(), ApiError> {
        self.act(Method::Delete, "/call/hand".to_owned()).await
    }

    /// Reports whether the person speaks, for the marks that people outside the voice
    /// channel see (ADR 0513).
    pub async fn report_speaking(&self, speaking: bool) -> Result<(), ApiError> {
        let report = ReportSpeaking { speaking };
        self.put("/call/speaking".to_owned(), &report).await
    }

    // The call of a voice channel.

    pub async fn channel_call(&self, channel_id: ChannelId) -> Result<ChannelCall, ApiError> {
        self.get(format!("/channels/{channel_id}/call")).await
    }

    /// Changes the user limit and the setting that hides who is inside (ADR 0514).
    pub async fn update_call_settings(
        &self,
        channel_id: ChannelId,
        change: &UpdateCallSettings,
    ) -> Result<Channel, ApiError> {
        let path = format!("/channels/{channel_id}/call/settings");
        self.send(Method::Patch, path, change).await
    }

    /// Asks for a place in the call of a voice channel. The answer carries the token for
    /// the media server, which is usable for about one minute and for one connection.
    pub async fn join_channel_call(&self, channel_id: ChannelId) -> Result<CallJoin, ApiError> {
        self.join(format!("/channels/{channel_id}/call/join")).await
    }

    /// As [`ServerApi::join_channel_call`], stating `shown` shares of one person. `None`
    /// is the request of a client from before several shares, which a test uses as such
    /// a client.
    pub async fn join_channel_call_showing(
        &self,
        channel_id: ChannelId,
        shown: Option<u8>,
    ) -> Result<CallJoin, ApiError> {
        self.join_showing(format!("/channels/{channel_id}/call/join"), shown)
            .await
    }

    /// A new token for the seat `identity` in the call of a voice channel.
    pub async fn renew_channel_call(
        &self,
        channel_id: ChannelId,
        identity: &str,
    ) -> Result<CallJoin, ApiError> {
        self.renew(format!("/channels/{channel_id}/call/join"), identity)
            .await
    }

    pub async fn lock_call(&self, channel_id: ChannelId) -> Result<(), ApiError> {
        self.act(Method::Put, format!("/channels/{channel_id}/call/lock"))
            .await
    }

    pub async fn unlock_call(&self, channel_id: ChannelId) -> Result<(), ApiError> {
        let path = format!("/channels/{channel_id}/call/lock");
        self.act(Method::Delete, path).await
    }

    pub async fn start_meeting(&self, channel_id: ChannelId) -> Result<(), ApiError> {
        let path = format!("/channels/{channel_id}/call/meeting");
        self.act(Method::Put, path).await
    }

    pub async fn end_meeting(&self, channel_id: ChannelId) -> Result<(), ApiError> {
        let path = format!("/channels/{channel_id}/call/meeting");
        self.act(Method::Delete, path).await
    }

    // The recording of the call of a voice channel (ADR 0601, ADR 0604).

    /// Starts a recording of the call that the caller is in. The answer has the state
    /// `starting`; what follows arrives as events. `language`: the language of the speech
    /// for the transcript, or nothing when it is to be detected.
    pub async fn start_recording(
        &self,
        channel_id: ChannelId,
        language: Option<&str>,
    ) -> Result<Recording, ApiError> {
        let path = format!("/channels/{channel_id}/call/recording");
        let body = StartRecording {
            language: language.map(str::to_owned),
        };
        self.send(Method::Put, path, &body).await
    }

    /// Asks for the end of the recording of the call that the caller is in.
    pub async fn stop_recording(&self, channel_id: ChannelId) -> Result<(), ApiError> {
        let path = format!("/channels/{channel_id}/call/recording");
        self.act(Method::Delete, path).await
    }

    // Requests to enter a locked or full voice channel.

    pub async fn request_entry(&self, channel_id: ChannelId) -> Result<EntryRequest, ApiError> {
        let path = format!("/channels/{channel_id}/call/entry-requests");
        read_json(&self.call(Method::Post, path, Vec::new(), None).await?)
    }

    /// Accepts or denies the request of a person outside. Anyone inside answers, and the
    /// first answer applies.
    pub async fn answer_entry_request(
        &self,
        request_id: EntryRequestId,
        accept: bool,
    ) -> Result<(), ApiError> {
        let path = format!("/call/entry-requests/{request_id}/answer");
        let answer = AnswerEntryRequest { accept };
        self.call(
            Method::Post,
            path,
            Vec::new(),
            Some(super::client::encode(&answer)?),
        )
        .await
        .map(|_| ())
    }

    /// Withdraws the own request to enter.
    pub async fn cancel_entry_request(&self, request_id: EntryRequestId) -> Result<(), ApiError> {
        let path = format!("/call/entry-requests/{request_id}");
        self.act(Method::Delete, path).await
    }

    // Invitations.

    /// Invites an account into the call of the voice channel that the caller is in.
    pub async fn invite_to_call(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
    ) -> Result<CallInvitation, ApiError> {
        let path = format!("/channels/{channel_id}/call/invitations");
        self.send(Method::Post, path, &InviteToCall { account_id })
            .await
    }

    /// Accepts an invitation and takes the place in the call.
    pub async fn accept_call_invitation(
        &self,
        invitation_id: CallInvitationId,
    ) -> Result<CallJoin, ApiError> {
        self.join(format!("/call/invitations/{invitation_id}/acceptance"))
            .await
    }

    pub async fn refuse_call_invitation(
        &self,
        invitation_id: CallInvitationId,
    ) -> Result<(), ApiError> {
        let path = format!("/call/invitations/{invitation_id}/refusal");
        self.act(Method::Post, path).await
    }

    /// Withdraws an invitation that the caller made.
    pub async fn cancel_call_invitation(
        &self,
        invitation_id: CallInvitationId,
    ) -> Result<(), ApiError> {
        let path = format!("/call/invitations/{invitation_id}");
        self.act(Method::Delete, path).await
    }

    // Moves between voice channels (ADR 0511).

    pub async fn move_participant(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
        to_channel_id: ChannelId,
    ) -> Result<CallMove, ApiError> {
        let path = format!("/channels/{channel_id}/call/participants/{account_id}/move");
        self.send(Method::Post, path, &MoveParticipant { to_channel_id })
            .await
    }

    /// The token for the destination of a move of the caller. Only the session that holds
    /// the place in the source gets it.
    pub async fn join_move(&self, move_id: CallMoveId) -> Result<CallJoin, ApiError> {
        self.join(format!("/call/moves/{move_id}/join")).await
    }

    /// Reports that this device could not join the destination of a move.
    pub async fn fail_move(&self, move_id: CallMoveId) -> Result<(), ApiError> {
        self.act(Method::Post, format!("/call/moves/{move_id}/failure"))
            .await
    }

    // Moderation in a call (ADR 0503).

    pub async fn block_microphone(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
    ) -> Result<(), ApiError> {
        self.act(
            Method::Put,
            participant(channel_id, account_id, "/microphone-block"),
        )
        .await
    }

    pub async fn unblock_microphone(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
    ) -> Result<(), ApiError> {
        let path = participant(channel_id, account_id, "/microphone-block");
        self.act(Method::Delete, path).await
    }

    /// Lowers the raised hand of another person without removing a microphone block
    /// (ADR 0516).
    pub async fn lower_participant_hand(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
    ) -> Result<(), ApiError> {
        let path = participant(channel_id, account_id, "/hand");
        self.act(Method::Delete, path).await
    }

    pub async fn block_sound(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
    ) -> Result<(), ApiError> {
        self.act(
            Method::Put,
            participant(channel_id, account_id, "/sound-block"),
        )
        .await
    }

    pub async fn unblock_sound(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
    ) -> Result<(), ApiError> {
        let path = participant(channel_id, account_id, "/sound-block");
        self.act(Method::Delete, path).await
    }

    /// Removes a person from the call of a voice channel.
    pub async fn disconnect_participant(
        &self,
        channel_id: ChannelId,
        account_id: AccountId,
    ) -> Result<(), ApiError> {
        self.act(Method::Delete, participant(channel_id, account_id, ""))
            .await
    }

    // Direct-message calls (ADR 0501, ADR 0515).

    /// Asks for a place in the direct-message call with an account. The first account
    /// that enters rings the other one.
    pub async fn join_direct_call(&self, account_id: AccountId) -> Result<CallJoin, ApiError> {
        self.join(format!("/direct-calls/{account_id}/join")).await
    }

    /// A new token for the seat `identity` in the direct call with an account.
    pub async fn renew_direct_call(
        &self,
        account_id: AccountId,
        identity: &str,
    ) -> Result<CallJoin, ApiError> {
        self.renew(format!("/direct-calls/{account_id}/join"), identity)
            .await
    }

    /// Declines the ring of the direct-message call with an account.
    pub async fn refuse_direct_call(&self, account_id: AccountId) -> Result<(), ApiError> {
        self.act(Method::Post, format!("/direct-calls/{account_id}/refusal"))
            .await
    }

    /// Mutes the direct-message conversation with an account until `until`, or until the
    /// mute is turned off. A muted conversation gets no call invitation.
    pub async fn mute_direct_conversation(
        &self,
        account_id: AccountId,
        until: Option<OffsetDateTime>,
    ) -> Result<(), ApiError> {
        let path = format!("/direct-messages/{account_id}/mute");
        self.put(path, &MuteDirectConversation { until }).await
    }

    pub async fn unmute_direct_conversation(&self, account_id: AccountId) -> Result<(), ApiError> {
        self.act(
            Method::Delete,
            format!("/direct-messages/{account_id}/mute"),
        )
        .await
    }

    async fn put<B: serde::Serialize>(&self, path: String, body: &B) -> Result<(), ApiError> {
        let body = Some(super::client::encode(body)?);
        self.call(Method::Put, path, Vec::new(), body)
            .await
            .map(|_| ())
    }
}

fn participant(channel_id: ChannelId, account_id: AccountId, end: &str) -> String {
    format!("/channels/{channel_id}/call/participants/{account_id}{end}")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use junto_protocol::ids::{CallId, RecordingId, ServerId, WorkspaceId};
    use junto_protocol::recording::{RecordingDetail, RecordingPage, RecordingState};
    use junto_protocol::transcript::TranscriptState;

    use super::*;
    use crate::address::ServerAddress;
    use crate::api::transport::{HttpRequest, HttpResponse, Transport, TransportFuture};
    use crate::api::{ApiClient, RetryPolicy};
    use crate::secret::SecretToken;
    use crate::version::SupportedVersions;

    /// Answers every request with one body and keeps the requests.
    struct Recorded {
        body: Vec<u8>,
        seen: Mutex<Vec<HttpRequest>>,
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
                    receipt: crate::api::Receipt::NotStated,
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
            assert!(seen[0].body.is_none());
            let url = &seen[0].url;
            (
                seen[0].method,
                url.path().to_owned(),
                url.query().map(str::to_owned),
            )
        }
    }

    /// A server that answers every request with `answer`.
    fn server<A: serde::Serialize>(answer: &A) -> (Arc<Recorded>, ServerApi) {
        let transport = Arc::new(Recorded {
            body: serde_json::to_vec(answer).unwrap(),
            seen: Mutex::default(),
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

    fn recording(workspace_id: WorkspaceId) -> Recording {
        let at = OffsetDateTime::from_unix_timestamp(1_900_000_000).unwrap();
        Recording {
            id: RecordingId::generate(),
            workspace_id,
            channel_id: ChannelId::generate(),
            call_id: CallId::generate(),
            state: RecordingState::Ready,
            started_by: AccountId::generate(),
            requested_at: at,
            recording_since: Some(at),
            stop: None,
            ended_at: Some(at),
            failure: None,
            language: "auto".to_owned(),
            segments: 2,
            failed_segments: 0,
            bytes: Some(4096),
            timeline_started_at: Some(at),
            transcript: TranscriptState::Ready,
            transcript_lines: 3,
        }
    }

    #[tokio::test]
    async fn the_recordings_of_a_workspace_are_read_by_the_page_that_is_asked_for() {
        let workspace_id = WorkspaceId::generate();
        let newest = recording(workspace_id);
        let page = RecordingPage {
            recordings: vec![newest.clone()],
            next_before: Some(newest.id),
        };
        let path = format!("/api/v1/workspaces/{workspace_id}/recordings");

        let (seen, api) = server(&page);
        assert_eq!(
            api.recordings(workspace_id, None, None).await.unwrap(),
            page
        );
        assert_eq!(seen.request(), (Method::Get, path.clone(), None));

        let (seen, api) = server(&page);
        let read = api.recordings(workspace_id, Some(newest.id), Some(200));
        assert_eq!(read.await.unwrap(), page);
        let query = format!("before={}&limit=200", newest.id);
        assert_eq!(seen.request(), (Method::Get, path.clone(), Some(query)));

        let (seen, api) = server(&page);
        api.recordings(workspace_id, None, Some(1)).await.unwrap();
        let query = Some("limit=1".to_owned());
        assert_eq!(seen.request(), (Method::Get, path, query));
    }

    #[tokio::test]
    async fn one_recording_is_read_with_its_segments() {
        let detail = RecordingDetail {
            recording: recording(WorkspaceId::generate()),
            segments: Vec::new(),
        };
        let recording_id = detail.recording.id;
        let (seen, api) = server(&detail);
        assert_eq!(api.recording(recording_id).await.unwrap(), detail);
        let path = format!("/api/v1/recordings/{recording_id}");
        assert_eq!(seen.request(), (Method::Get, path, None));
    }
}
