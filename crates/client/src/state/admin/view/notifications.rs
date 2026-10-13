//! The page "Notifications" of a workspace (ADR 0412): the notification level that the
//! workspace sets for its members, or that it sets none.

use junto_client_core::model::WorkspaceModel;
use junto_client_core::protocol::discovery::NotificationLevel;
use junto_client_core::protocol::ids::WorkspaceId;

/// What the page offers: no level of the workspace, or one of the three levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultChoice {
    /// The workspace sets no level: the own default of each member holds.
    NotSet,
    Level(NotificationLevel),
}

impl DefaultChoice {
    pub const ALL: [Self; 4] = [
        Self::NotSet,
        Self::Level(NotificationLevel::All),
        Self::Level(NotificationLevel::Mentions),
        Self::Level(NotificationLevel::Nothing),
    ];

    pub const fn of(level: Option<NotificationLevel>) -> Self {
        match level {
            Some(level) => Self::Level(level),
            None => Self::NotSet,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationsPage {
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    /// What the workspace sets now, as the state of the server holds it.
    pub choice: DefaultChoice,
}

/// The page of the notification default of the shown workspace.
pub fn derive(workspace: &WorkspaceModel) -> NotificationsPage {
    NotificationsPage {
        workspace_id: workspace.workspace.id,
        workspace_name: workspace.workspace.name.clone(),
        choice: DefaultChoice::of(workspace.notification_default),
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::structure::Workspace;

    use super::*;

    fn workspace(level: Option<NotificationLevel>) -> WorkspaceModel {
        WorkspaceModel {
            workspace: Workspace {
                id: WorkspaceId::generate(),
                name: "Studio".to_owned(),
                is_default: true,
            },
            own_role_ids: Vec::new(),
            roles: Vec::new(),
            permissions: Vec::new(),
            channel_permissions: Vec::new(),
            notification_default: level,
            categories: Vec::new(),
            channels: Vec::new(),
            members: junto_client_core::model::Loadable::NotLoaded,
        }
    }

    #[test]
    fn the_page_states_that_the_workspace_sets_no_level_or_which_one_it_sets() {
        let unset = workspace(None);
        let page = derive(&unset);
        assert_eq!(page.choice, DefaultChoice::NotSet);
        assert_eq!(page.workspace_id, unset.workspace.id);
        assert_eq!(page.workspace_name, "Studio");
        for level in [
            NotificationLevel::All,
            NotificationLevel::Mentions,
            NotificationLevel::Nothing,
        ] {
            assert_eq!(
                derive(&workspace(Some(level))).choice,
                DefaultChoice::Level(level)
            );
        }
        // "Not set" is a choice of its own: it is not the level "mentions only", which
        // holds for a member without any default.
        assert_ne!(
            DefaultChoice::of(None),
            DefaultChoice::of(Some(NotificationLevel::Mentions))
        );
        assert_eq!(DefaultChoice::ALL[0], DefaultChoice::NotSet);
        assert_eq!(DefaultChoice::ALL.len(), 4);
    }
}
