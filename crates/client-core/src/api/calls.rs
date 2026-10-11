//! The call routes and the snapshot (ADR 0207, ADR 0510). The server decides who is in a
//! call and gives out the token for the media server; no media passes through these
//! requests.

use junto_protocol::call::{
    AnswerEntryRequest, CallInvitation, CallJoin, CallMove, ChannelCall, EntryRequest,
    InviteToCall, MoveParticipant, MuteDirectConversation, OwnCallView, ReportCallState,
    ReportScreenShare, ReportSpeaking, ShareAudio, UpdateCallSettings,
};
use junto_protocol::ids::{AccountId, CallInvitationId, CallMoveId, ChannelId, EntryRequestId};
use junto_protocol::snapshot::Snapshot;
use junto_protocol::structure::Channel;
use time::OffsetDateTime;

use super::client::read_json;
use super::error::ApiError;
use super::session::ServerApi;
use super::transport::Method;

impl ServerApi {
    /// The state that the event stream continues from the cursor of the answer (ADR 0207).
    pub async fn snapshot(&self) -> Result<Snapshot, ApiError> {
        self.get("/snapshot".to_owned()).await
    }

    async fn join(&self, path: String) -> Result<CallJoin, ApiError> {
        read_json(&self.call(Method::Post, path, Vec::new(), None).await?)
    }

    // The call of the account.

    /// The call that the account is in, with the requests to enter that wait there.
    pub async fn own_call(&self) -> Result<OwnCallView, ApiError> {
        self.get("/call".to_owned()).await
    }

    pub async fn leave_call(&self) -> Result<(), ApiError> {
        self.act(Method::Delete, "/call".to_owned()).await
    }

    /// Reports the person's own microphone and sound settings. Only the session that
    /// holds the place in the call reports (ADR 0510).
    pub async fn report_call_state(&self, muted: bool, deafened: bool) -> Result<(), ApiError> {
        let report = ReportCallState { muted, deafened };
        self.put("/call/state".to_owned(), &report).await
    }

    pub async fn report_screen_share(&self, audio: ShareAudio) -> Result<(), ApiError> {
        let report = ReportScreenShare { audio };
        self.put("/call/screen-share".to_owned(), &report).await
    }

    pub async fn end_screen_share(&self) -> Result<(), ApiError> {
        self.act(Method::Delete, "/call/screen-share".to_owned())
            .await
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
