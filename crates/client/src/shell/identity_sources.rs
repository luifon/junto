use super::{
    Shell,
    account_menus::{AccountSource, OpenAccountMenu},
};
use crate::state::{
    feed::FeedRow,
    recorded::CardState,
    view::{ShellView, TranscriptPanelState},
};

impl OpenAccountMenu {
    pub(super) fn additional_identity_present(&self, view: &ShellView) -> bool {
        let server = self.owner.entry.server_id;
        match self.source.as_ref() {
            Some(AccountSource::Profile) => {
                view.selected.as_ref().is_some_and(|row| row.id == server)
                    && self
                        .people
                        .iter()
                        .all(|person| view.person.account_id == Some(person.account_id))
            }
            Some(AccountSource::MentionCandidate { pane, place, .. }) => {
                view.selected.as_ref().is_some_and(|row| row.id == server)
                    && match pane {
                        super::Pane::Main => view
                            .lists()
                            .and_then(|lists| lists.feed.as_ref())
                            .is_some_and(|feed| feed.place == *place),
                        super::Pane::Thread => view
                            .workspace()
                            .and_then(|workspace| workspace.thread.as_ref())
                            .and_then(|thread| thread.feed.as_ref())
                            .is_some_and(|feed| feed.place == *place),
                    }
            }
            Some(AccountSource::Invitation(id)) => view.invitations.iter().any(|invited| {
                invited.id == *id
                    && invited.target.server_id == server
                    && self
                        .people
                        .iter()
                        .all(|person| invited.caller == person.account_id)
            }),
            Some(AccountSource::Transcript {
                recording,
                line,
                preview,
            }) => {
                if view.selected.as_ref().is_none_or(|row| row.id != server) {
                    return false;
                }
                let account = |account| {
                    self.people
                        .iter()
                        .all(|person| person.account_id == account)
                };
                if *preview {
                    view.lists().and_then(|lists| lists.feed.as_ref()).is_some_and(|feed| feed.rows.iter().any(|row| {
                        matches!(row, FeedRow::Recording(row) if row.card.recording_id == *recording && matches!(&row.card.state, CardState::Shown(detail) if detail.transcript.lines.iter().any(|row| row.line == *line && account(row.account_id))))
                    }))
                } else {
                    view.workspace().and_then(|workspace| workspace.transcript.as_ref()).is_some_and(|panel| panel.recording_id == *recording && matches!(&panel.state, TranscriptPanelState::Shown { open, .. } if open.lines.lines.iter().any(|row| row.line == *line && account(row.account_id))))
                }
            }
            _ => false,
        }
    }
}

impl Shell {
    pub(super) fn additional_identity_visible(
        &self,
        open: &OpenAccountMenu,
        view: &ShellView,
    ) -> bool {
        match open.source.as_ref() {
            Some(AccountSource::Profile) => {
                matches!(self.layer, super::Layer::None) && !self.detached
            }
            Some(AccountSource::MentionCandidate { pane, query, .. }) => {
                matches!(self.layer, super::Layer::None)
                    && self.mention_identity_current(*pane, query, &open.people, view)
            }
            Some(AccountSource::Transcript { .. }) => matches!(self.layer, super::Layer::None),
            Some(AccountSource::Invitation(id)) => {
                let mut invitations: Vec<_> = view.invitations.iter().collect();
                invitations.sort_by_key(|invited| (invited.created_at, invited.id));
                let hidden = if self.invitations_open {
                    0
                } else {
                    invitations.len().saturating_sub(super::invited::MOST_CARDS)
                };
                invitations
                    .into_iter()
                    .skip(hidden)
                    .any(|invited| invited.id == *id)
            }
            _ => true,
        }
    }
}
