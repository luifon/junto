//! The page "Notifications" of the account group (ADR 0742): the levels that the person
//! chose on this server, for the server, for each workspace, and the channels and group
//! conversations with a setting of their own.

use junto_client_core::api::SettingsScope;
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::discovery::{NotificationLevel, NotificationSettings};
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use crate::state::notices::{LevelView, Mute, inherited_level_of, mute_of, stored_settings};
use crate::state::view::Known;
use crate::text;

/// The level of the person in one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLevel {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub level: LevelView,
}

/// A channel or a group conversation with a setting of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnException {
    pub scope: SettingsScope,
    pub name: String,
    /// The workspace of a channel; absent for a group conversation.
    pub workspace: Option<String>,
    /// What the setting is, in words.
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnNotificationsPage {
    /// The level of the server: "mentions only" while the person stored none.
    pub server_level: NotificationLevel,
    pub workspaces: Vec<WorkspaceLevel>,
    pub exceptions: Vec<OwnException>,
    /// A change is on its way to the server.
    pub busy: bool,
    /// Why the server did not take the last change of this page.
    pub refused: Option<String>,
}

/// True when the setting of a channel or a conversation is not its default.
fn own(settings: &NotificationSettings, mute: &Mute) -> bool {
    settings.level.is_some()
        || mute.is_on()
        || settings.role_mentions == Some(false)
        || settings.everyone_mentions == Some(false)
}

/// The words of a setting of its own: its level, its mute and its switches.
fn state_of(settings: &NotificationSettings, mute: &Mute) -> String {
    let mut parts = Vec::new();
    if let Some(level) = settings.level {
        parts.push(text::level_name(level).to_owned());
    }
    if let Some(muted) = text::mute_state(mute) {
        parts.push(muted);
    }
    if settings.role_mentions == Some(false) {
        parts.push(text::NO_ROLE_MENTIONS.to_owned());
    }
    if settings.everyone_mentions == Some(false) {
        parts.push(text::NO_EVERYONE_MENTIONS.to_owned());
    }
    parts.join(" · ")
}

pub fn derive(server_id: ServerId, model: &ServerModel, known: &Known) -> OwnNotificationsPage {
    let settings = |scope| stored_settings(server_id, scope, model, known);
    let member_of = |workspace_id: &WorkspaceId| model.account.workspace_ids.contains(workspace_id);
    let workspaces = model
        .workspaces
        .iter()
        .filter(|workspace| member_of(&workspace.workspace.id))
        .map(|workspace| {
            let scope = SettingsScope::Workspace(workspace.workspace.id);
            WorkspaceLevel {
                workspace_id: workspace.workspace.id,
                name: text::label(&workspace.workspace.name),
                level: LevelView {
                    stored: settings(scope).level,
                    inherited: inherited_level_of(server_id, scope, model, known),
                },
            }
        })
        .collect();
    let mut exceptions = Vec::new();
    for workspace in model
        .workspaces
        .iter()
        .filter(|workspace| member_of(&workspace.workspace.id))
    {
        for channel in &workspace.channels {
            let scope = SettingsScope::Channel(channel.channel.id);
            let held = settings(scope);
            let mute = mute_of(&held, known);
            if own(&held, &mute) {
                exceptions.push(OwnException {
                    scope,
                    name: text::label(&channel.channel.name),
                    workspace: Some(text::label(&workspace.workspace.name)),
                    state: state_of(&held, &mute),
                });
            }
        }
    }
    for group in &model.group_conversations {
        let scope = SettingsScope::Conversation(group.id);
        let held = settings(scope);
        let mute = mute_of(&held, known);
        if own(&held, &mute) {
            exceptions.push(OwnException {
                scope,
                name: if group.name.trim().is_empty() {
                    text::GROUP_CONVERSATION.to_owned()
                } else {
                    text::label(&group.name)
                },
                workspace: None,
                state: state_of(&held, &mute),
            });
        }
    }
    let keys: Vec<_> = std::iter::once(SettingsScope::Account)
        .chain(
            model
                .workspaces
                .iter()
                .map(|workspace| SettingsScope::Workspace(workspace.workspace.id)),
        )
        .chain(exceptions.iter().map(|exception| exception.scope))
        .map(|scope| (server_id, scope.id()))
        .collect();
    OwnNotificationsPage {
        server_level: settings(SettingsScope::Account)
            .level
            .unwrap_or(NotificationLevel::Mentions),
        workspaces,
        exceptions,
        busy: keys.iter().any(|key| known.notify_busy.contains(key)),
        refused: known
            .notify_refused
            .as_ref()
            .filter(|(server, scope_id, _)| {
                *server == server_id && keys.iter().any(|(_, id)| id == scope_id)
            })
            .map(|(_, _, reason)| reason.clone()),
    }
}
