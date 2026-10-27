//! The requests of the administration that change the rules of the roles and the people in
//! a channel or a category, and the sync of a channel with its category (ADR 0304).
//!
//! A change of one permission is one request that names that permission and its new
//! state. The server changes it in the rule that it holds and keeps every other
//! permission, so a change of another person to the same rule is not undone, and the
//! grant ceiling is applied to the changed permission only.

use gpui::Context;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, ServerId};
use junto_client_core::protocol::structure::{PermissionRule, RuleSubject, SetChannelOverride};

use super::view::overrides::{Choice, choice_of, private_state, state_of};
use super::{AskKind, RulesOf};
use crate::state::AppState;

impl AppState {
    /// The rules of a place as the state of the server holds them. Absent until they are
    /// loaded.
    fn loaded_rules(&self, server_id: ServerId, at: RulesOf) -> Option<Vec<PermissionRule>> {
        let model = self.client.server(server_id)?.live().view().model?;
        match at {
            RulesOf::Channel(channel_id) => model
                .admin
                .rules(channel_id)
                .loaded()
                .map(|rules| rules.rules.clone()),
            RulesOf::Category(workspace_id, category_id) => model
                .admin
                .category_rules(workspace_id, category_id)
                .loaded()
                .map(|rules| rules.rules.clone()),
        }
    }

    /// Sets one permission of the rule of a role or a person at a place to a choice, with
    /// one request that names that permission. The other permissions of the rule stay as
    /// the server holds them. Nothing is sent while the rules are not loaded, and for the
    /// choice that the rule states already.
    pub fn set_rule_choice(
        &mut self,
        server_id: ServerId,
        (at, subject): (RulesOf, RuleSubject),
        permission: WorkspacePermission,
        choice: Choice,
        cx: &mut Context<Self>,
    ) {
        let Some(rules) = self.loaded_rules(server_id, at) else {
            return;
        };
        let current = rules.iter().find(|rule| rule.subject == subject);
        if choice_of(current, permission) == choice {
            return;
        }
        let state = state_of(choice);
        self.admin_ask(
            server_id,
            AskKind::SetRule(at, subject),
            |api| async move {
                match at {
                    RulesOf::Channel(channel_id) => {
                        api.change_channel_rule(channel_id, subject, permission, state)
                            .await
                    }
                    RulesOf::Category(workspace_id, category_id) => {
                        api.change_category_rule(
                            (workspace_id, category_id),
                            subject,
                            permission,
                            state,
                        )
                        .await
                    }
                }
            },
            |_, _, _| {},
            cx,
        );
    }

    /// Removes the rule of a role or a person at a place (two empty sets). Nothing is
    /// sent while the rules are not loaded, and for a subject without a rule.
    pub fn clear_rule(
        &mut self,
        server_id: ServerId,
        (at, subject): (RulesOf, RuleSubject),
        cx: &mut Context<Self>,
    ) {
        let Some(rules) = self.loaded_rules(server_id, at) else {
            return;
        };
        if !rules.iter().any(|rule| rule.subject == subject) {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::SetRule(at, subject),
            |api| async move {
                let sets = SetChannelOverride::default();
                match at {
                    RulesOf::Channel(channel_id) => {
                        api.set_channel_rule(channel_id, subject, &sets).await
                    }
                    RulesOf::Category(workspace_id, category_id) => {
                        api.set_category_rule((workspace_id, category_id), subject, &sets)
                            .await
                    }
                }
            },
            |_, _, _| {},
            cx,
        );
    }

    /// Lists a role or a person on the open page of rules without a rule, so that the
    /// person can make its first choice (ADR 0304). The page forgets it with the page.
    pub fn add_rule_subject(&mut self, subject: RuleSubject, cx: &mut Context<Self>) {
        if !self.admin.rules_added.contains(&subject) {
            self.admin.rules_added.push(subject);
        }
        self.changed(cx);
    }

    /// Removes the own rules of a channel, which takes the rules of its category again
    /// (ADR 0304). Nothing is sent while the rules are not loaded, and for a channel that
    /// is synced already or has no category.
    pub fn sync_rules(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return;
        };
        let Some(rules) = model.admin.rules(channel_id).loaded() else {
            return;
        };
        let in_category = model
            .readable_channel(channel_id)
            .is_some_and(|channel| channel.category_id.is_some());
        if rules.synced || !in_category {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::SyncRules(channel_id),
            |api| async move { api.sync_channel_rules(channel_id).await },
            |_, _, _| {},
            cx,
        );
    }

    /// Makes a channel private or open: denies "view channel" to the "everyone" role, or
    /// lets the roles decide it again (ADR 0304). The request names that one permission.
    /// Nothing is sent while the rules are not loaded, and for a channel that is private
    /// or open already.
    pub fn set_private(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        private: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return;
        };
        let Some(rules) = model.admin.rules(channel_id).loaded() else {
            return;
        };
        let everyone = model
            .readable_channel(channel_id)
            .and_then(|channel| model.workspace(channel.workspace_id))
            .and_then(|workspace| workspace.roles.iter().find(|role| role.is_everyone));
        let Some(role_id) = everyone.map(|role| role.id) else {
            return;
        };
        let Some(state) = private_state(rules.of_role(role_id), private) else {
            return;
        };
        let subject = RuleSubject::Role { role_id };
        self.admin_ask(
            server_id,
            AskKind::SetRule(RulesOf::Channel(channel_id), subject),
            |api| async move {
                api.change_channel_rule(
                    channel_id,
                    subject,
                    WorkspacePermission::ViewChannel,
                    state,
                )
                .await
            },
            |_, _, _| {},
            cx,
        );
    }
}
