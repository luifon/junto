//! The page "Glossary" of a workspace (ADR 0616): the names and terms that the speech model
//! receives before it transcribes a recording of the workspace, the most important first
//! (ADR 0603). Every member reads it; a person with "manage glossary" changes it.

use junto_client_core::model::{Collection, ServerModel, WorkspaceModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::{Listed, holds_workspace};
use crate::state::admin::AdminState;
use crate::state::admin::glossary::Answered;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlossaryPage {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    /// The terms, the most important first.
    pub terms: Listed<Vec<String>>,
    /// The server gave more terms than the client holds: a change would remove the
    /// others, so the page offers none.
    pub cut: bool,
    /// The person holds "manage glossary" in the workspace. The server decides.
    pub may_manage: bool,
}

impl GlossaryPage {
    /// Why the page takes no change now. `None`: it takes one.
    pub fn held(&self) -> Option<&'static str> {
        use crate::text::admin::glossary;
        if !self.may_manage {
            Some(glossary::MANAGE_NEEDS)
        } else if self.cut {
            Some(glossary::CUT)
        } else if self.terms.loaded().is_none() {
            Some(glossary::NOT_READ)
        } else {
            None
        }
    }
}

/// The page of the glossary of the shown workspace.
pub fn derive(
    server_id: ServerId,
    model: &ServerModel,
    workspace: &WorkspaceModel,
    admin: &AdminState,
) -> GlossaryPage {
    let workspace_id = workspace.workspace.id;
    let held = model.admin.glossary(workspace_id);
    // The answer to the last change of the person is newer than the state until the state
    // shows a read that started after it (ADR 0616).
    let answered = Answered::over(
        admin.glossary_answered.as_ref(),
        server_id,
        workspace_id,
        model,
    );
    GlossaryPage {
        workspace_id,
        workspace_name: workspace.workspace.name.clone(),
        terms: match (held, answered) {
            (Collection::Loaded(_), Some(terms)) => Listed::Loaded(terms.to_vec()),
            _ => Listed::of(held, |held| held.terms.clone()),
        },
        cut: held.loaded().is_some_and(|held| held.cut),
        may_manage: holds_workspace(workspace, WorkspacePermission::ManageGlossary),
    }
}
