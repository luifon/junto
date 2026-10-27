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
use junto_protocol::ids::{ChannelId, WorkspaceId};

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
    /// The workspaces whose archived list the client reads without a reader, to check the
    /// archived channels that it carried over a reload (ADR 0762).
    checking: BTreeSet<WorkspaceId>,
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
        AdminPart::Rules(channel_id) => {
            api.channel_rules(channel_id).await.map(AdminPayload::Rules)
        }
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
        AdminPart::CategoryRules(workspace_id, category_id) => api
            .category_rules(workspace_id, category_id)
            .await
            .map(AdminPayload::CategoryRules),
        AdminPart::MemberPermissions(workspace_id, account_id) => api
            .member_permissions(workspace_id, account_id)
            .await
            .map(AdminPayload::MemberPermissions),
        AdminPart::MemberAccess(workspace_id, account_id) => api
            .member_access(workspace_id, account_id)
            .await
            .map(AdminPayload::MemberAccess),
        AdminPart::ServerPermissions(account_id) => api
            .server_permissions(account_id)
            .await
            .map(AdminPayload::ServerPermissions),
    }
}

/// True when the state holds the workspace or the channel of a collection.
fn in_state(model: &ServerModel, part: AdminPart) -> bool {
    match part {
        AdminPart::Rules(channel_id) | AdminPart::ChannelPermissions(channel_id) => {
            model.readable_channel(channel_id).is_some()
        }
        AdminPart::CategoryRules(workspace_id, category_id) => {
            model.workspace(workspace_id).is_some_and(|workspace| {
                workspace
                    .categories
                    .iter()
                    .any(|category| category.id == category_id)
            })
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
        if self.checks_admin(part) {
            // The read goes on for the check of the carried channels, and its answer is
            // not kept.
            return;
        }
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

    /// Reads a collection that a reader has open again, whatever its state. A state that is
    /// stale is replaced by a snapshot, after which every open collection is read again.
    pub(super) fn read_admin_now(&mut self, part: AdminPart) {
        let held = self
            .model
            .as_ref()
            .is_some_and(|model| !self.stale && in_state(model, part));
        if held && self.wants_admin(part) {
            self.start(Load::Part(Part::Admin(part)));
        }
    }

    pub(super) fn wants_admin(&self, part: AdminPart) -> bool {
        self.admin.wanted.contains_key(&part)
    }

    /// True for the archived list of a workspace that the client reads to check the
    /// archived channels that it carried.
    fn checks_admin(&self, part: AdminPart) -> bool {
        matches!(part, AdminPart::Archived(workspace_id)
            if self.admin.checking.contains(&workspace_id))
    }

    /// The answer to the read of a collection, with the events that arrived since the
    /// read was sent.
    pub(super) fn admin_loaded(
        &mut self,
        part: AdminPart,
        result: Result<AdminPayload, ApiError>,
        kept: &[Arc<Event>],
    ) {
        let wanted = self.wants_admin(part);
        let check = self.checks_admin(part);
        if !wanted && !check {
            return;
        }
        if let AdminPart::Archived(workspace_id) = part {
            self.admin.checking.remove(&workspace_id);
        }
        let held = self.held_channels();
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
                if let AdminPart::Archived(workspace_id) = part {
                    model.close_lost_archived();
                    if !wanted {
                        // No reader holds the list: it decided which carried channels stay,
                        // and it goes again.
                        model.release_archived(workspace_id, &held);
                    }
                    self.close_lost_collections();
                }
                if !wanted {
                    return;
                }
                self.answer_admin(part, || Ok(()));
                // An event that names a change without carrying it arrived after the
                // read was sent: the answer can be older than the change.
                if kept.iter().any(|event| part.is_stale_after(event)) {
                    self.start(Load::Part(Part::Admin(part)));
                }
            }
            Err(error) if !wanted => {
                // The carried channels stay unchecked until the next reload.
                tracing::debug!(%error, ?part, "the archived list was not read for a check");
                if let AdminPart::Archived(workspace_id) = part {
                    model.release_archived(workspace_id, &held);
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

    /// The channels of the collections of one channel that readers have open.
    fn held_channels(&self) -> Vec<ChannelId> {
        self.admin
            .wanted
            .keys()
            .filter_map(|part| match part {
                AdminPart::Rules(channel_id) | AdminPart::ChannelPermissions(channel_id) => {
                    Some(*channel_id)
                }
                _ => None,
            })
            .collect()
    }

    /// Closes each open collection of one channel whose channel the state no longer holds.
    fn close_lost_collections(&mut self) {
        let Some(model) = &self.model else {
            return;
        };
        let lost: Vec<AdminPart> = self
            .admin
            .wanted
            .keys()
            .copied()
            .filter(|part| {
                matches!(part, AdminPart::Rules(_) | AdminPart::ChannelPermissions(_))
                    && !in_state(model, *part)
            })
            .collect();
        for part in lost {
            self.admin.wanted.remove(&part);
            self.admin.refreshing.remove(&part);
            let held = Part::Admin(part);
            self.resume.retain(|kept| *kept != held);
            self.pending
                .retain(|pending| pending.load != Load::Part(held));
            if let Some(model) = &mut self.model {
                model.set_admin(part, AdminSet::NotLoaded);
                self.model_changed = true;
            }
            self.answer_admin(part, || Err(LoadError::NotInState));
        }
    }

    fn answer_admin(&mut self, part: AdminPart, answer: impl Fn() -> Result<(), LoadError>) {
        for waiting in self.admin.opening.remove(&part).unwrap_or_default() {
            self.answers.push((waiting, answer()));
        }
    }

    /// After a snapshot: every collection that a reader has open is read again
    /// (ADR 0207), and a collection whose workspace or channel the new state does not hold
    /// is closed. A collection that was loaded shows its values of before until its read
    /// answers (ADR 0760). An archived channel whose messages a reader has open is carried
    /// until the archived list of its workspace answers (ADR 0762).
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
                AdminPart::Rules(channel_id) | AdminPart::ChannelPermissions(channel_id) => {
                    Some(PlaceId::Channel(*channel_id))
                }
                _ => None,
            }));
            model.carry_archived(before, &held);
        }
        // A carried channel is checked by the archived list of its workspace. The check
        // holds the read of the list also when a reader releases the list before the
        // answer, and a list that no reader holds is read for the check alone (ADR 0762).
        let checks: Vec<WorkspaceId> = self
            .model
            .as_ref()
            .map(|model| model.carried_archived_workspaces())
            .unwrap_or_default();
        self.admin.checking = checks.iter().copied().collect();
        for workspace_id in checks {
            let list = AdminPart::Archived(workspace_id);
            // The list that a reader holds is read below with the other open collections.
            if !self.admin.wanted.contains_key(&list) {
                self.start(Load::Part(Part::Admin(list)));
            }
        }
        for part in wanted {
            let held = self.model.as_mut().is_some_and(|model| {
                let held = in_state(model, part);
                if let Some(before) = before.filter(|_| held) {
                    model.carry_admin(before, part);
                }
                held
            });
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
        if self.checks_admin(part) && !self.wants_admin(part) {
            return true;
        }
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
