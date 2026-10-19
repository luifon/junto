//! What the application does for the glossary of a workspace (ADR 0616): a change of the
//! person is made from the list that the state holds, and is sent as the whole new list
//! with the list that it changes, so that the server refuses it when another person changed
//! the glossary meanwhile (ADR 0607).

use gpui::Context;
use junto_client_core::model::GlossaryList;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::{AdminOpen, AdminPage, AskKind};
use crate::state::AppState;
use crate::text;

/// The most terms that a glossary holds (ADR 0607).
pub const MOST_TERMS: usize = 200;
/// The most characters of one term.
pub const LONGEST_TERM: usize = 200;

/// A change of the glossary that the person asks for. A change names the term that the
/// page showed at its position: a list that holds another term there changed meanwhile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlossaryChange {
    /// A term at the end of the list.
    Add(String),
    Change {
        position: u32,
        from: String,
        to: String,
    },
    Remove {
        position: u32,
        term: String,
    },
    /// The term moves one place up, towards the most important end, or down.
    Move {
        position: u32,
        term: String,
        up: bool,
    },
}

impl GlossaryChange {
    pub const fn ask(&self) -> AskKind {
        match self {
            Self::Add(_) => AskKind::AddTerm,
            Self::Change { position, .. } => AskKind::ChangeTerm(*position),
            Self::Remove { position, .. } => AskKind::RemoveTerm(*position),
            Self::Move { position, .. } => AskKind::MoveTerm(*position),
        }
    }
}

/// The glossary that a change of the person replaced. The page takes no further change
/// while the state still holds that list: a change made from it would be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub server_id: ServerId,
    pub workspace_id: WorkspaceId,
    pub terms: Vec<String>,
}

/// Why a term cannot be in the glossary `terms`, where the term at `except` is the one that
/// it replaces. The rules are those of the server (ADR 0607).
pub fn term_problem(term: &str, terms: &[String], except: Option<usize>) -> Option<&'static str> {
    if term.is_empty() {
        return Some(text::admin::glossary::TERM_EMPTY);
    }
    if term.chars().count() > LONGEST_TERM {
        return Some(text::admin::glossary::TERM_LONG);
    }
    if term.chars().any(char::is_control) || term.contains("<|") || term.contains("|>") {
        return Some(text::admin::glossary::TERM_CHARACTERS);
    }
    let twice = terms
        .iter()
        .enumerate()
        .any(|(index, held)| Some(index) != except && held == term);
    twice.then_some(text::admin::glossary::TERM_TWICE)
}

/// The new list that a change makes of `held`. `Ok(None)`: the change changes nothing.
/// `Err`: why it is not sent.
pub fn changed(
    held: &[String],
    change: &GlossaryChange,
) -> Result<Option<Vec<String>>, &'static str> {
    let at = |position: u32, term: &str| {
        usize::try_from(position)
            .ok()
            .filter(|index| held.get(*index).is_some_and(|held| held == term))
            .ok_or(text::admin::glossary::CHANGED_MEANWHILE)
    };
    let mut terms = held.to_vec();
    match change {
        GlossaryChange::Add(term) => {
            let term = term.trim();
            if held.len() >= MOST_TERMS {
                return Err(text::admin::glossary::FULL);
            }
            if let Some(problem) = term_problem(term, held, None) {
                return Err(problem);
            }
            terms.push(term.to_owned());
        }
        GlossaryChange::Change { position, from, to } => {
            let index = at(*position, from)?;
            let to = to.trim();
            if to == from {
                return Ok(None);
            }
            if let Some(problem) = term_problem(to, held, Some(index)) {
                return Err(problem);
            }
            if let Some(slot) = terms.get_mut(index) {
                to.clone_into(slot);
            }
        }
        GlossaryChange::Remove { position, term } => {
            let index = at(*position, term)?;
            terms.remove(index);
        }
        GlossaryChange::Move { position, term, up } => {
            let index = at(*position, term)?;
            let other = if *up {
                index.checked_sub(1)
            } else {
                Some(index + 1).filter(|other| *other < held.len())
            };
            let Some(other) = other else {
                return Ok(None);
            };
            terms.swap(index, other);
        }
    }
    Ok(Some(terms))
}

impl AppState {
    /// The glossary of a workspace as the state holds it now.
    fn held_glossary(
        &self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
    ) -> Option<GlossaryList> {
        let model = self.client.server(server_id)?.live().view().model?;
        model.admin.glossary(workspace_id).loaded().cloned()
    }

    /// Changes the glossary of the shown workspace. The new list is made from the list
    /// that the state holds now, and is sent with it: the server replaces the glossary
    /// only while it holds that list (ADR 0607). A change that changes nothing sends
    /// nothing. Returns whether a request was sent, or why the change was not sent.
    pub fn change_glossary(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        change: GlossaryChange,
        cx: &mut Context<Self>,
    ) -> Result<bool, &'static str> {
        let Some(held) = self.held_glossary(server_id, workspace_id) else {
            return Err(text::admin::glossary::NOT_READ);
        };
        // A list that the client holds in part would lose the terms that it does not hold.
        if held.cut {
            return Err(text::admin::glossary::CUT);
        }
        let Some(terms) = changed(&held.terms, &change)? else {
            return Ok(false);
        };
        let base = held.terms;
        let sent = Sent {
            server_id,
            workspace_id,
            terms: base.clone(),
        };
        self.admin_ask(
            server_id,
            change.ask(),
            move |api| async move { api.set_glossary(workspace_id, &terms, Some(&base)).await },
            move |this, _, _| this.admin.glossary_sent = Some(sent),
            cx,
        );
        Ok(true)
    }

    /// The state holds another glossary than the one that the last change replaced: the
    /// change, or a later one, has arrived, and the page takes changes again.
    pub(super) fn settle_glossary_sent(&mut self) {
        let Some(sent) = &self.admin.glossary_sent else {
            return;
        };
        let open = self.admin.open
            == Some(AdminOpen::Server {
                server_id: sent.server_id,
                page: AdminPage::Glossary,
            });
        let held = self.held_glossary(sent.server_id, sent.workspace_id);
        if !open || held.is_none_or(|held| held.terms != sent.terms) {
            self.admin.glossary_sent = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(terms: &[&str]) -> Vec<String> {
        terms.iter().map(|term| (*term).to_owned()).collect()
    }

    #[test]
    fn a_change_is_made_from_the_held_list_and_names_the_term_that_it_changes() {
        let held = list(&["Quorvex", "Lumitrax", "Ysolde"]);
        let add = GlossaryChange::Add("  Kestrel ".to_owned());
        assert_eq!(
            changed(&held, &add),
            Ok(Some(list(&["Quorvex", "Lumitrax", "Ysolde", "Kestrel"])))
        );
        let change = GlossaryChange::Change {
            position: 1,
            from: "Lumitrax".to_owned(),
            to: "Lumitrax Ltd".to_owned(),
        };
        assert_eq!(
            changed(&held, &change),
            Ok(Some(list(&["Quorvex", "Lumitrax Ltd", "Ysolde"])))
        );
        let remove = GlossaryChange::Remove {
            position: 0,
            term: "Quorvex".to_owned(),
        };
        assert_eq!(
            changed(&held, &remove),
            Ok(Some(list(&["Lumitrax", "Ysolde"])))
        );
        let up = GlossaryChange::Move {
            position: 2,
            term: "Ysolde".to_owned(),
            up: true,
        };
        assert_eq!(
            changed(&held, &up),
            Ok(Some(list(&["Quorvex", "Ysolde", "Lumitrax"])))
        );
        // The ends of the list do not move further, and an unchanged term sends nothing.
        let first_up = GlossaryChange::Move {
            position: 0,
            term: "Quorvex".to_owned(),
            up: true,
        };
        let last_down = GlossaryChange::Move {
            position: 2,
            term: "Ysolde".to_owned(),
            up: false,
        };
        let same = GlossaryChange::Change {
            position: 0,
            from: "Quorvex".to_owned(),
            to: " Quorvex".to_owned(),
        };
        for nothing in [first_up, last_down, same] {
            assert_eq!(changed(&held, &nothing), Ok(None));
        }
        // A list that holds another term at the position changed meanwhile.
        let moved = GlossaryChange::Remove {
            position: 1,
            term: "Ysolde".to_owned(),
        };
        assert_eq!(
            changed(&held, &moved),
            Err(text::admin::glossary::CHANGED_MEANWHILE)
        );
    }

    #[test]
    fn a_term_follows_the_rules_of_the_server() {
        let held = list(&["Quorvex", "Lumitrax"]);
        assert_eq!(
            term_problem("", &held, None),
            Some(text::admin::glossary::TERM_EMPTY)
        );
        assert_eq!(
            term_problem(&"a".repeat(201), &held, None),
            Some(text::admin::glossary::TERM_LONG)
        );
        assert_eq!(term_problem(&"a".repeat(200), &held, None), None);
        for refused in ["two\nlines", "<|en|>", "a|>"] {
            assert_eq!(
                term_problem(refused, &held, None),
                Some(text::admin::glossary::TERM_CHARACTERS),
                "{refused:?}"
            );
        }
        assert_eq!(
            term_problem("Lumitrax", &held, None),
            Some(text::admin::glossary::TERM_TWICE)
        );
        // The term that a change replaces does not count.
        assert_eq!(term_problem("Lumitrax", &held, Some(1)), None);
        let full: Vec<String> = (0..MOST_TERMS).map(|number| format!("t{number}")).collect();
        assert_eq!(
            changed(&full, &GlossaryChange::Add("one more".to_owned())),
            Err(text::admin::glossary::FULL)
        );
    }
}
