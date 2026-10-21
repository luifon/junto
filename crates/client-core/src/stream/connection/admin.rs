//! What the stream client does for the collections of the administration (ADR 0760):
//! which of them readers have open, their reads, and their return after a reload of the
//! state.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use junto_protocol::access_token::AccessTokenQuery;
use junto_protocol::events::Event;
use tokio::sync::oneshot;

use super::{Client, Load};
use crate::api::{ApiError, ServerApi};
use crate::live::LoadError;
use crate::model::{
    AdminPart, AdminPayload, AdminSet, CollectionState, LoadFailure, MOST_TOKENS, Part, PlaceId,
    ServerModel, WorkspaceSettings,
};

type Reply = oneshot::Sender<Result<(), LoadError>>;

/// The largest page of access tokens that the server gives (ADR 0210).
const TOKEN_PAGE: u32 = 200;

#[derive(Default)]
pub(super) struct Collections {
    /// The collections that readers have open, with the number of their readers.
    wanted: BTreeMap<AdminPart, usize>,
    /// The readers that wait for the first answer of a collection.
    opening: BTreeMap<AdminPart, Vec<Reply>>,
    /// The loaded collections whose new read is on its way or was interrupted. Such a
    /// collection shows its earlier values, so the state does not tell that a read of it
    /// is still owed.
    refreshing: BTreeSet<AdminPart>,
}

/// Reads one collection from its route.
pub(super) async fn read(api: &ServerApi, part: AdminPart) -> Result<AdminPayload, ApiError> {
    match part {
        AdminPart::Accounts => api.listed_accounts().await.map(AdminPayload::Accounts),
        AdminPart::Invites => api.invites().await.map(AdminPayload::Invites),
        AdminPart::JoinRequests => api.join_requests().await.map(AdminPayload::JoinRequests),
        AdminPart::AccessTokens => {
            let mut tokens = Vec::new();
            let mut before = None;
            // A page that adds nothing ends the read, so a server cannot keep it running.
            loop {
                let query = AccessTokenQuery {
                    before,
                    limit: Some(TOKEN_PAGE),
                };
                let page = api.access_tokens(&query).await?;
                let added = page.access_tokens.len();
                tokens.extend(page.access_tokens);
                let full = tokens.len() >= MOST_TOKENS;
                match page.next_before {
                    Some(next) if added > 0 && !full => before = Some(next),
                    next => {
                        tokens.truncate(MOST_TOKENS);
                        return Ok(AdminPayload::AccessTokens {
                            tokens,
                            more: next.is_some(),
                        });
                    }
                }
            }
        }
        AdminPart::Exports => api.exports().await.map(AdminPayload::Exports),
        AdminPart::ServerRoles => api
            .listed_server_roles()
            .await
            .map(AdminPayload::ServerRoles),
        AdminPart::RoleActions(workspace_id) => api
            .listed_workspace_roles(workspace_id)
            .await
            .map(AdminPayload::RoleActions),
        AdminPart::DirectRetention => api
            .direct_message_retention()
            .await
            .map(AdminPayload::DirectRetention),
        AdminPart::AttachmentLimits => api
            .attachment_limits()
            .await
            .map(AdminPayload::AttachmentLimits),
        AdminPart::Overrides(channel_id) => api
            .channel_overrides(channel_id)
            .await
            .map(AdminPayload::Overrides),
        AdminPart::Archived(workspace_id) => api
            .archived_channels(workspace_id)
            .await
            .map(AdminPayload::Archived),
        AdminPart::WorkspaceSettings(workspace_id) => {
            let archive_period = api.archive_period(workspace_id).await?;
            let file_retention = api.file_retention(workspace_id).await?;
            let recording_retention = api.recording_retention(workspace_id).await?;
            Ok(AdminPayload::WorkspaceSettings(WorkspaceSettings {
                archive_period,
                file_retention,
                recording_retention,
            }))
        }
        AdminPart::ChannelPermissions(channel_id) => api
            .channel_permissions(channel_id)
            .await
            .map(AdminPayload::ChannelPermissions),
        AdminPart::Glossary(workspace_id) => {
            let glossary = api.glossary(workspace_id).await?;
            // A server is not trusted to answer for the workspace that was asked for
            // (ADR 0714).
            if glossary.workspace_id != workspace_id {
                return Err(ApiError::InvalidResponse {
                    status: 200,
                    reason: "the glossary of another workspace",
                });
            }
            Ok(AdminPayload::Glossary(glossary.terms))
        }
    }
}

/// True when the state holds the workspace or the channel of a collection.
fn in_state(model: &ServerModel, part: AdminPart) -> bool {
    match part {
        AdminPart::Overrides(channel_id) | AdminPart::ChannelPermissions(channel_id) => {
            model.readable_channel(channel_id).is_some()
        }
        part => part
            .workspace()
            .is_none_or(|workspace_id| model.workspace(workspace_id).is_some()),
    }
}

impl Client {
    pub(super) fn open_admin(&mut self, part: AdminPart, reply: Reply) {
        if self.target.is_none() {
            let _ = reply.send(Err(LoadError::Off));
            return;
        }
        let state = match &self.model {
            Some(model) if !self.stale => {
                if !in_state(model, part) {
                    let _ = reply.send(Err(LoadError::NotInState));
                    return;
                }
                Some(model.admin.state(part))
            }
            // The collection is read when the state has arrived.
            _ => None,
        };
        *self.admin.wanted.entry(part).or_default() += 1;
        match state {
            Some(CollectionState::Loaded) => self.answers.push((reply, Ok(()))),
            Some(CollectionState::NotLoaded | CollectionState::Failed(_)) => {
                self.admin.opening.entry(part).or_default().push(reply);
                self.start(Load::Part(Part::Admin(part)));
            }
            Some(CollectionState::Loading) | None => {
                self.admin.opening.entry(part).or_default().push(reply);
            }
        }
    }

    pub(super) fn release_admin(&mut self, part: AdminPart) {
        let Some(readers) = self.admin.wanted.get_mut(&part) else {
            return;
        };
        *readers = readers.saturating_sub(1);
        if *readers > 0 {
            return;
        }
        self.admin.wanted.remove(&part);
        self.admin.opening.remove(&part);
        self.admin.refreshing.remove(&part);
        let held = Part::Admin(part);
        self.resume.retain(|kept| *kept != held);
        self.pending
            .retain(|pending| pending.load != Load::Part(held));
        if let Some(model) = &mut self.model {
            model.set_admin(part, AdminSet::NotLoaded);
            self.model_changed = true;
        }
    }

    /// Reads a collection again that a reader has open and whose last read failed.
    pub(super) fn read_admin_again(&mut self, part: AdminPart) {
        let failed = self.model.as_ref().is_some_and(|model| {
            !self.stale && matches!(model.admin.state(part), CollectionState::Failed(_))
        });
        if failed && self.admin.wanted.contains_key(&part) {
            if let Some(model) = &mut self.model {
                model.set_admin(part, AdminSet::NotLoaded);
            }
            self.start(Load::Part(Part::Admin(part)));
        }
    }

    pub(super) fn wants_admin(&self, part: AdminPart) -> bool {
        self.admin.wanted.contains_key(&part)
    }

    /// The answer to the read of a collection, with the events that arrived since the
    /// read was sent.
    pub(super) fn admin_loaded(
        &mut self,
        part: AdminPart,
        result: Result<AdminPayload, ApiError>,
        kept: &[Arc<Event>],
    ) {
        if !self.wants_admin(part) {
            return;
        }
        let Some(model) = &mut self.model else {
            return;
        };
        self.model_changed = true;
        self.admin.refreshing.remove(&part);
        if !in_state(model, part) {
            // The workspace or the channel of the collection left the state while the
            // read was on its way: the answer belongs to nothing that the state holds.
            model.set_admin(part, AdminSet::NotLoaded);
            self.admin.wanted.remove(&part);
            self.answer_admin(part, || Err(LoadError::NotInState));
            return;
        }
        match result {
            Ok(payload) => {
                model.set_admin(part, AdminSet::Loaded(payload));
                model.replay_admin(part, kept.iter().map(Arc::as_ref));
                if matches!(part, AdminPart::Archived(_)) {
                    model.close_lost_archived();
                }
                self.answer_admin(part, || Ok(()));
                // An event that names a change without carrying it arrived after the
                // read was sent: the answer can be older than the change.
                if kept.iter().any(|event| part.is_stale_after(event)) {
                    self.start(Load::Part(Part::Admin(part)));
                }
            }
            Err(error) => {
                tracing::debug!(%error, ?part, "a collection of the administration was not read");
                model.set_admin(part, AdminSet::Failed(LoadFailure::of(&error)));
                let error = Arc::new(error);
                self.answer_admin(part, || Err(LoadError::Request(error.clone())));
            }
        }
    }

    fn answer_admin(&mut self, part: AdminPart, answer: impl Fn() -> Result<(), LoadError>) {
        for waiting in self.admin.opening.remove(&part).unwrap_or_default() {
            self.answers.push((waiting, answer()));
        }
    }

    /// After a snapshot: every collection that a reader has open is read again
    /// (ADR 0207), and a collection whose workspace or channel the new state does not hold
    /// is closed. An archived channel whose messages a reader has open is carried until
    /// the archived list of its workspace answers (ADR 0762).
    pub(super) fn carry_admin(&mut self, before: Option<&ServerModel>, places: &[PlaceId]) {
        let Some(model) = &mut self.model else {
            return;
        };
        let wanted: Vec<AdminPart> = self.admin.wanted.keys().copied().collect();
        if let Some(before) = before {
            // A collection of one channel holds its channel as a reader of its messages
            // does: an archived channel is in no snapshot, and without it the collection
            // would close at every reload.
            let mut held = places.to_vec();
            held.extend(wanted.iter().filter_map(|part| match part {
                AdminPart::Overrides(channel_id) | AdminPart::ChannelPermissions(channel_id) => {
                    Some(PlaceId::Channel(*channel_id))
                }
                _ => None,
            }));
            model.carry_archived(before, &held);
        }
        for part in wanted {
            let held = self
                .model
                .as_ref()
                .is_some_and(|model| in_state(model, part));
            if held {
                self.start(Load::Part(Part::Admin(part)));
            } else {
                self.admin.wanted.remove(&part);
                self.admin.refreshing.remove(&part);
                self.answer_admin(part, || Err(LoadError::NotInState));
            }
        }
    }

    /// The account or the level changed: no reader has a collection open any more.
    pub(super) fn forget_admin(&mut self) {
        let work = std::mem::take(&mut self.admin);
        for reply in work.opening.into_values().flatten() {
            let _ = reply.send(Err(LoadError::Off));
        }
    }

    /// True when the state still needs the read of a collection that a change of the
    /// session interrupted.
    pub(super) fn admin_read_needed(&self, part: AdminPart, model: &ServerModel) -> bool {
        self.wants_admin(part)
            && (model.admin.state(part) != CollectionState::Loaded
                || self.admin.refreshing.contains(&part))
    }

    /// A read of a collection starts. A collection that is loaded keeps its values on
    /// screen, and the client remembers that its new read is owed.
    pub(super) fn admin_read_starts(&mut self, part: AdminPart) {
        let Some(model) = &mut self.model else {
            return;
        };
        if model.admin.state(part) == CollectionState::Loaded {
            self.admin.refreshing.insert(part);
        }
        model.set_admin(part, AdminSet::Loading);
        self.model_changed = true;
    }
}
