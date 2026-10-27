//! The screens of the permission rules (ADR 0304, ADR 0306): the rules of the roles and the
//! people of a channel and of a category, the sync of a channel with its category, the
//! workspace permissions of a member with what the member can use, and the server
//! permissions of an account.

use junto_client::shell::admin::Mode;
use junto_client::state::admin::{AdminOpen, ChannelPage, PersonPage};
use junto_client_core::model::{Collection, ServerModel};
use junto_client_core::protocol::ids::{AccountId, CategoryId, ChannelId, WorkspaceId};
use junto_client_core::protocol::roles::ServerPersonPermissions;
use junto_client_core::protocol::structure::{
    CategoryRules, ChannelPermissions, ChannelRules, PermissionRule, PersonPermissions, RuleSubject,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::admin::{
    ACME, ALPHA, DEPLOYS, EVERYONE, LEADS, PROJECTS, STUDIO, accounts, administrator, collections,
    lead, open_channel, owner, server_roles, servers, structure,
};
use super::{Screen, Studio, id, person};

fn bruno() -> RuleSubject {
    RuleSubject::Person {
        account_id: person("Bruno"),
    }
}

fn everyone_rule() -> PermissionRule {
    PermissionRule {
        subject: RuleSubject::Role {
            role_id: id(EVERYONE),
        },
        allow: Vec::new(),
        deny: vec![WorkspacePermission::AttachFiles],
    }
}

fn leads_rule() -> PermissionRule {
    PermissionRule {
        subject: RuleSubject::Role { role_id: id(LEADS) },
        allow: vec![WorkspacePermission::ManageMessages],
        deny: Vec::new(),
    }
}

fn bruno_rule() -> PermissionRule {
    PermissionRule {
        subject: bruno(),
        allow: vec![WorkspacePermission::RecordCalls],
        deny: vec![WorkspacePermission::SendMessages],
    }
}

/// The rules of `#client-alpha`, a channel of "Projects", and whether it takes the rules
/// of the category.
fn alpha_rules(model: &mut ServerModel, synced: bool) {
    let rules = if synced {
        vec![everyone_rule(), leads_rule()]
    } else {
        vec![everyone_rule(), leads_rule(), bruno_rule()]
    };
    collections(model).rules = vec![(
        id(ALPHA),
        Collection::Loaded(ChannelRules {
            channel_id: id(ALPHA),
            category_id: Some(id(PROJECTS)),
            synced,
            rules,
        }),
    )];
}

/// The rules of "Projects", with `#client-alpha` synced and `#deploys` with its own rules.
fn projects_rules(model: &mut ServerModel) {
    let admin = collections(model);
    admin.category_rules = vec![(
        (id(STUDIO), id(PROJECTS)),
        Collection::Loaded(CategoryRules {
            category_id: id(PROJECTS),
            rules: vec![everyone_rule(), leads_rule(), bruno_rule()],
        }),
    )];
    let channel = |channel: u32, synced| {
        (
            id::<ChannelId>(channel),
            Collection::Loaded(ChannelRules {
                channel_id: id(channel),
                category_id: Some(id(PROJECTS)),
                synced,
                rules: Vec::new(),
            }),
        )
    };
    admin.rules = vec![channel(ALPHA, true), channel(DEPLOYS, false)];
}

fn open_category(
    shell: &mut junto_client::shell::Shell,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<junto_client::shell::Shell>,
) {
    let open = AdminOpen::Category {
        server_id: id(ACME),
        workspace_id: id(STUDIO),
        category_id: id::<CategoryId>(PROJECTS),
    };
    shell.open_admin(open, window, cx);
}

fn open_person(
    shell: &mut junto_client::shell::Shell,
    account: AccountId,
    page: PersonPage,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<junto_client::shell::Shell>,
) {
    let open = AdminOpen::Person {
        server_id: id(ACME),
        account_id: account,
        page,
    };
    shell.open_admin(open, window, cx);
}

/// The workspace permissions of Bruno in "Studio": "mention everyone" allowed, "send
/// messages" denied.
fn bruno_permissions(model: &mut ServerModel) {
    collections(model).member_permissions = vec![(
        (id(STUDIO), person("Bruno")),
        Collection::Loaded(PersonPermissions {
            account_id: person("Bruno"),
            allow: vec![WorkspacePermission::MentionEveryone],
            deny: vec![WorkspacePermission::SendMessages],
        }),
    )];
}

/// What Bruno can use in the channels of "Studio".
fn bruno_access(model: &mut ServerModel) {
    let studio: WorkspaceId = id(STUDIO);
    let channels: Vec<ChannelId> = model
        .workspace(studio)
        .map(|workspace| {
            workspace
                .channels
                .iter()
                .map(|held| held.channel.id)
                .collect()
        })
        .unwrap_or_default();
    let used = |index: usize| -> Vec<WorkspacePermission> {
        match index {
            0 => Vec::new(),
            1 => vec![
                WorkspacePermission::ViewChannel,
                WorkspacePermission::ReadMessageHistory,
            ],
            _ => vec![
                WorkspacePermission::ViewChannel,
                WorkspacePermission::SendMessagesInThreads,
                WorkspacePermission::ReadMessageHistory,
                WorkspacePermission::AddReactions,
                WorkspacePermission::Connect,
                WorkspacePermission::Speak,
            ],
        }
    };
    collections(model).member_access = vec![(
        (studio, person("Bruno")),
        Collection::Loaded(
            channels
                .into_iter()
                .enumerate()
                .map(|(index, channel_id)| ChannelPermissions {
                    channel_id,
                    permissions: used(index % 3),
                })
                .collect(),
        ),
    )];
}

/// The server permissions of Bruno: "create invites" allowed, "administrator" denied.
fn server_permissions(model: &mut ServerModel, account: &str) {
    let admin = collections(model);
    admin.accounts = Collection::Loaded(accounts("Ana"));
    admin.server_roles = Collection::Loaded(server_roles());
    admin.server_permissions = vec![(
        person(account),
        Collection::Loaded(ServerPersonPermissions {
            account_id: person(account),
            allow: vec![ServerPermission::CreateInvites],
            deny: vec![ServerPermission::Administrator],
        }),
    )];
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    // A channel of a category with its own rules: the roles and Bruno, whose rule is shown.
    studio.draw(
        Screen::new(
            "admin-rules-channel-person",
            servers(|model| {
                owner(model);
                structure(model);
                alpha_rules(model, false);
            }),
        ),
        |shell, window, cx| {
            open_channel(shell, ALPHA, ChannelPage::Permissions, window, cx);
            shell.show_rule_subject(Some(bruno()), cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "admin-rules-channel-synced",
            servers(|model| {
                owner(model);
                structure(model);
                alpha_rules(model, true);
            }),
        ),
        |shell, window, cx| open_channel(shell, ALPHA, ChannelPage::Permissions, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-rules-channel-sync-question",
            servers(|model| {
                owner(model);
                structure(model);
                alpha_rules(model, false);
            }),
        ),
        |shell, window, cx| {
            open_channel(shell, ALPHA, ChannelPage::Permissions, window, cx);
            shell.set_admin_mode(Some(Mode::SyncRules), "", window, cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "admin-rules-channel-adding",
            servers(|model| {
                owner(model);
                structure(model);
                alpha_rules(model, true);
            }),
        ),
        |shell, window, cx| {
            open_channel(shell, ALPHA, ChannelPage::Permissions, window, cx);
            shell.set_rules_adding(true, window, cx);
            shell.type_rules_search("a", window, cx);
        },
    )?;
    // A member who manages the channel and holds few permissions: the ceiling refuses
    // what the member does not hold, and the rule of an equal member is locked.
    studio.draw(
        Screen::new(
            "admin-rules-channel-ceiling",
            servers(|model| {
                lead(
                    model,
                    &[
                        WorkspacePermission::ManageChannel,
                        WorkspacePermission::ViewChannel,
                    ],
                );
                structure(model);
                alpha_rules(model, false);
            }),
        ),
        |shell, window, cx| {
            open_channel(shell, ALPHA, ChannelPage::Permissions, window, cx);
            shell.show_rule_subject(Some(bruno()), cx);
        },
    )?;

    // The settings of a category.
    studio.draw(
        Screen::new(
            "admin-rules-category",
            servers(|model| {
                owner(model);
                structure(model);
                projects_rules(model);
            }),
        ),
        open_category,
    )?;

    // The page of a member and what the member can use.
    studio.draw(
        Screen::new(
            "admin-rules-member",
            servers(|model| {
                owner(model);
                bruno_permissions(model);
            }),
        ),
        |shell, window, cx| {
            open_person(
                shell,
                person("Bruno"),
                PersonPage::Permissions(id(STUDIO)),
                window,
                cx,
            );
        },
    )?;
    studio.draw(
        Screen::new(
            "admin-rules-member-ceiling",
            servers(|model| {
                lead(
                    model,
                    &[
                        WorkspacePermission::ManageRoles,
                        WorkspacePermission::ViewChannel,
                    ],
                );
                bruno_permissions(model);
            }),
        ),
        |shell, window, cx| {
            open_person(
                shell,
                person("Bruno"),
                PersonPage::Permissions(id(STUDIO)),
                window,
                cx,
            );
        },
    )?;
    studio.draw(
        Screen::new(
            "admin-rules-member-access",
            servers(|model| {
                owner(model);
                structure(model);
                bruno_access(model);
            }),
        ),
        |shell, window, cx| {
            open_person(
                shell,
                person("Bruno"),
                PersonPage::Access(id(STUDIO)),
                window,
                cx,
            );
        },
    )?;

    // The server permissions of an account.
    studio.draw(
        Screen::new(
            "admin-rules-server",
            servers(|model| {
                owner(model);
                server_permissions(model, "Marta");
            }),
        ),
        |shell, window, cx| open_person(shell, person("Marta"), PersonPage::Server, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-rules-server-administrator",
            servers(|model| {
                administrator(model);
                server_permissions(model, "Marta");
            }),
        ),
        |shell, window, cx| open_person(shell, person("Marta"), PersonPage::Server, window, cx),
    )?;
    studio.draw(
        Screen::new(
            "admin-rules-server-owner",
            servers(|model| {
                administrator(model);
                server_permissions(model, "Ana");
            }),
        ),
        |shell, window, cx| open_person(shell, person("Ana"), PersonPage::Server, window, cx),
    )?;
    Ok(())
}
