//! The pages of the permission rules in the window against the real server (ADR 0304,
//! ADR 0306, ADR 0761): the rules of the roles and the people of a channel and of a
//! category, the sync of a channel with its category, the workspace permissions of a
//! member with what the member can use, and the server permissions of an account.
//!
//! A test acts on the drawn controls by their names and reads the result from the server
//! and from the state of the client of the person whom a rule concerns.

use gpui::TestAppContext;
use junto_client_core::ServerHandle;
use junto_client_core::protocol::ids::{CategoryId, WorkspaceId, WorkspaceRoleId};
use junto_client_core::protocol::roles::CreateWorkspaceRole;
use junto_client_core::protocol::structure::{
    ChannelKind, CreateCategory, CreateChannel, OverrideState, RuleSubject,
};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use junto_client::state::admin::AskState;
use junto_client::state::admin::view::Scope;
use junto_client::state::admin::view::overrides::SyncedCount;
use junto_client::state::admin::view::person::PersonPageView;

use crate::admin_pages::{
    assert_drawn_label, awaited, click_in_page, default_workspace, grant_server, grant_workspace,
    idle, invite, joined, open_channel_settings, open_page, owner_window, sel, text_channel,
    try_reveal, until_model,
};
use crate::harness::{Window, World, member};
use crate::support::logged_in;

/// The role that every member of a workspace holds.
fn everyone_of(world: &World, handle: &ServerHandle, workspace_id: WorkspaceId) -> WorkspaceRoleId {
    world
        .run(handle.api().workspace_roles(workspace_id))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id
}

fn category(
    world: &World,
    handle: &ServerHandle,
    workspace_id: WorkspaceId,
    name: &str,
) -> CategoryId {
    let category = CreateCategory {
        name: name.to_owned(),
        position: None,
    };
    world
        .run(handle.api().create_category(workspace_id, &category))
        .unwrap()
        .id
}

fn channel_in(
    world: &World,
    handle: &ServerHandle,
    (workspace_id, category_id): (WorkspaceId, CategoryId),
    name: &str,
) -> junto_client_core::protocol::ids::ChannelId {
    let channel = CreateChannel {
        name: name.to_owned(),
        kind: ChannelKind::Text,
        category_id: Some(category_id),
        position: None,
    };
    world
        .run(handle.api().create_channel(workspace_id, &channel))
        .unwrap()
        .id
}

fn role(
    world: &World,
    handle: &ServerHandle,
    workspace_id: WorkspaceId,
    name: &str,
    permissions: &[WorkspacePermission],
) -> WorkspaceRoleId {
    let role = CreateWorkspaceRole {
        name: name.to_owned(),
        permissions: permissions.to_vec(),
    };
    world
        .run(handle.api().create_workspace_role(workspace_id, &role))
        .unwrap()
        .id
}

/// Clicks a choice of the shown permission editor, and waits until the server did it.
#[track_caller]
fn choose(window: &mut Window, permission: &str, word: &str) {
    let refused = choose_answer(window, permission, word);
    assert_eq!(refused, None, "the server refused {permission} {word}");
}

/// The refusal that the page states for its newest request, once no request of the page
/// is on its way.
fn answered(window: &mut Window) -> Option<Option<String>> {
    let view = window.admin()?;
    if view.asks.iter().any(|ask| ask.state == AskState::Sending) {
        return None;
    }
    Some(view.asks.iter().rev().find_map(|ask| match &ask.state {
        AskState::Refused(reason) => Some(reason.clone()),
        AskState::Sending => None,
    }))
}

/// Clicks a choice of the shown permission editor and returns the refusal of the server,
/// once the answer arrived.
#[track_caller]
fn choose_answer(window: &mut Window, permission: &str, word: &str) -> Option<String> {
    window.until("no request of the page is on its way", idle);
    click_in_page(window, sel(&format!("override-{permission}"), word));
    awaited(window, "the choice is answered", answered)
}

/// The button of a choice is brought into view, and its accessible label is drawn.
fn shows_choice(window: &mut Window, selector: &'static str, label: &str) -> bool {
    try_reveal(window, selector).is_some() && drawn(window, label)
}

/// A click on a control that the page shows disabled sends no request.
#[track_caller]
fn sends_nothing(window: &mut Window, selector: &'static str) {
    click_in_page(window, selector);
    assert!(idle(window), "{selector} sent a request");
}

#[gpui::test]
fn the_permissions_page_of_a_channel_adds_a_person_and_gives_three_choices_for_each_permission(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&member);
    let default = default_workspace(&world, &owner);
    let everyone = everyone_of(&world, &owner, default);
    let plans = text_channel(&world, &owner, default, "plans");
    let send = WorkspacePermission::SendMessages;
    until_model(&world, &member, "the member holds the channel", |model| {
        model.holds_in(plans, send)
    });

    // The page lists the "everyone" role. A channel without a category states no sync.
    open_channel_settings(&mut window, "plans", plans);
    open_page(&mut window, "channel-nav-permissions", |window| {
        window.shows(sel("rules-subject-role", everyone))
    });
    assert!(!window.shows("rules-synced") && !window.shows("rules-not-synced"));
    assert!(!window.shows(sel("rules-subject-person", bruno)));

    // A person is added by a part of the name, among the members of the workspace.
    click_in_page(&mut window, "rules-add");
    window.until("the search field is drawn", |window| {
        window.shows("rules-search")
    });
    window.type_into("rules-search", "bru");
    click_in_page(&mut window, sel("rules-add-person", bruno));
    window.until("the person is listed with the permissions", |window| {
        window.shows(sel("rules-subject-person", bruno))
            && window.shows("override-send_messages-deny")
    });

    // Deny: the person can no longer send in the channel.
    choose(&mut window, "send_messages", "deny");
    until_model(&world, &member, "the person cannot send", |model| {
        model.channel(plans).is_some() && !model.holds_in(plans, send)
    });
    let person = RuleSubject::Person { account_id: bruno };
    let stored = world.run(owner.api().channel_rules(plans)).unwrap();
    let rule = stored.of(person).expect("the person rule is stored");
    assert_eq!(
        (rule.allow.as_slice(), rule.deny.as_slice()),
        ([].as_slice(), [send].as_slice())
    );
    window.until("the page shows the deny", |window| {
        shows_choice(
            window,
            "override-send_messages-deny",
            "Send messages: Deny, chosen",
        )
    });

    // Allow, then neutral, which removes the rule.
    choose(&mut window, "send_messages", "allow");
    let stored = world.run(owner.api().channel_rules(plans)).unwrap();
    assert_eq!(
        stored.of(person).map(|rule| rule.allow.clone()),
        Some(vec![send])
    );
    until_model(&world, &member, "the person can send again", |model| {
        model.holds_in(plans, send)
    });
    choose(&mut window, "send_messages", "neutral");
    let stored = world.run(owner.api().channel_rules(plans)).unwrap();
    assert!(stored.of(person).is_none(), "{stored:?}");
    window.until("the page shows neutral", |window| {
        shows_choice(
            window,
            "override-send_messages-neutral",
            "Send messages: Neutral, chosen",
        )
    });
}

#[gpui::test]
fn a_channel_in_a_category_states_whether_it_is_synced_follows_the_events_and_syncs_again(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let default = default_workspace(&world, &owner);
    let everyone = everyone_of(&world, &owner, default);
    let teams = category(&world, &owner, default, "Teams");
    let plans = channel_in(&world, &owner, (default, teams), "plans");
    let api = owner.api();
    let subject = RuleSubject::Role { role_id: everyone };
    let category_rule = |permission, state| {
        world
            .run(api.change_category_rule((default, teams), subject, permission, state))
            .unwrap();
    };
    category_rule(WorkspacePermission::AttachFiles, OverrideState::Deny);

    // A new channel of a category takes the rules of the category.
    open_channel_settings(&mut window, "plans", plans);
    open_page(&mut window, "channel-nav-permissions", |window| {
        window.shows("rules-synced")
    });
    assert_drawn_label(&mut window, "Synced with Teams");
    assert!(!window.shows("rules-sync-again"));
    window.until("the page shows the deny of the category", |window| {
        shows_choice(
            window,
            "override-attach_files-deny",
            "Attach files: Deny, chosen",
        )
    });

    // A change of the rules of the category reaches the page of the synced channel.
    category_rule(WorkspacePermission::MentionEveryone, OverrideState::Allow);
    window.until("the page shows the allow of the category", |window| {
        shows_choice(
            window,
            "override-mention_everyone-allow",
            "Mention everyone: Allow, chosen",
        )
    });

    // An own rule of the channel, made elsewhere, detaches it.
    world
        .run(api.change_channel_rule(
            plans,
            subject,
            WorkspacePermission::AddReactions,
            OverrideState::Deny,
        ))
        .unwrap();
    window.until("the page states that the channel is not synced", |window| {
        window.shows("rules-not-synced") && window.shows("rules-sync-again")
    });
    assert_drawn_label(&mut window, "Not synced with Teams");
    window.until("the page shows the own deny of the channel", |window| {
        shows_choice(
            window,
            "override-add_reactions-deny",
            "Add reactions: Deny, chosen",
        )
    });

    // "Sync again" asks first, then removes the own rules of the channel.
    click_in_page(&mut window, "rules-sync-again");
    click_in_page(&mut window, "rules-sync-cancel");
    assert!(!world.run(api.channel_rules(plans)).unwrap().synced);
    click_in_page(&mut window, "rules-sync-again");
    click_in_page(&mut window, "rules-sync-confirm");
    window.until("the page states that the channel is synced", |window| {
        window.shows("rules-synced")
    });
    let stored = world.run(api.channel_rules(plans)).unwrap();
    assert!(stored.synced);
    let rule = stored.of(subject).unwrap();
    assert!(
        !rule.deny.contains(&WorkspacePermission::AddReactions),
        "{stored:?}"
    );
    window.until("the page shows the rules of the category again", |window| {
        shows_choice(
            window,
            "override-add_reactions-neutral",
            "Add reactions: Neutral, chosen",
        )
    });

    // A change on the page of a synced channel detaches it.
    choose(&mut window, "record_calls", "deny");
    window.until("the page states that the channel is not synced", |window| {
        window.shows("rules-not-synced")
    });
    let category_rules = world.run(api.category_rules(default, teams)).unwrap();
    assert!(
        category_rules
            .rules
            .iter()
            .all(|rule| !rule.deny.contains(&WorkspacePermission::RecordCalls)),
        "the change of the channel changed the category: {category_rules:?}"
    );
}

/// True when the page draws a text or a control with this accessible label.
fn drawn(window: &mut Window, label: &str) -> bool {
    crate::admin_pages::drawn_text(window)
        .iter()
        .any(|text| text.text == label || text.label.as_deref() == Some(label))
}

#[gpui::test]
fn a_category_has_a_permissions_page_whose_rules_its_synced_channels_take(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&member);
    let default = default_workspace(&world, &owner);
    let teams = category(&world, &owner, default, "Teams");
    let alpha = channel_in(&world, &owner, (default, teams), "alpha");
    let beta = channel_in(&world, &owner, (default, teams), "beta");
    let gamma = channel_in(&world, &owner, (default, teams), "gamma");
    let leads = role(&world, &owner, default, "Leads", &[]);
    let api = owner.api();
    world
        .run(api.change_channel_rule(
            gamma,
            RuleSubject::Role { role_id: leads },
            WorkspacePermission::RecordCalls,
            OverrideState::Allow,
        ))
        .unwrap();
    until_model(&world, &member, "the member holds the channels", |model| {
        [alpha, beta, gamma]
            .iter()
            .all(|id| model.channel(*id).is_some())
    });

    // The page of the channels opens the settings of a category.
    window.open_administration();
    open_page(&mut window, "admin-nav-channels", |window| {
        window.shows(sel("category-permissions", teams))
    });
    click_in_page(&mut window, sel("category-permissions", teams));
    window.until("the settings of the category are open", |window| {
        window.shows("category-nav-permissions") && window.shows("category-synced-count")
    });
    window.until("the page counts the synced channels", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Category(settings))
                if settings.page.synced == SyncedCount::Counted { synced: 2, channels: 3 }
        )
    });

    // A role and a person are added; their rules reach the synced channels only.
    click_in_page(&mut window, "rules-add");
    window.until("the search field is drawn", |window| {
        window.shows("rules-search")
    });
    window.type_into("rules-search", "lea");
    click_in_page(&mut window, sel("rules-add-role", leads));
    choose(&mut window, "manage_messages", "allow");
    let stored = world.run(api.category_rules(default, teams)).unwrap();
    let rule = stored
        .rules
        .iter()
        .find(|rule| rule.subject == RuleSubject::Role { role_id: leads })
        .expect("the role rule of the category is stored");
    assert_eq!(rule.allow, [WorkspacePermission::ManageMessages]);
    for synced in [alpha, beta] {
        let rules = world.run(api.channel_rules(synced)).unwrap();
        assert!(
            rules.synced && rules.of_role(leads) == Some(rule),
            "{rules:?}"
        );
    }
    let own = world.run(api.channel_rules(gamma)).unwrap();
    assert!(
        !own.synced
            && own
                .of_role(leads)
                .is_some_and(|rule| rule.allow == [WorkspacePermission::RecordCalls])
    );

    click_in_page(&mut window, "rules-add");
    window.type_into("rules-search", "bru");
    click_in_page(&mut window, sel("rules-add-person", bruno));
    choose(&mut window, "view_channel", "deny");
    until_model(
        &world,
        &member,
        "the synced channels are hidden from the person",
        |model| {
            model.channel(alpha).is_none()
                && model.channel(beta).is_none()
                && model.channel(gamma).is_some()
        },
    );
    choose(&mut window, "view_channel", "neutral");
    until_model(&world, &member, "the synced channels return", |model| {
        model.channel(alpha).is_some() && model.channel(beta).is_some()
    });

    // The settings of the category return to the page of the channels.
    click_in_page(&mut window, "category-nav-back");
    window.until("the page of the channels is shown", |window| {
        window.shows(sel("category-permissions", teams))
    });
}

#[gpui::test]
fn the_page_of_a_member_sets_workspace_permissions_and_lists_what_the_member_can_use(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let ada = logged_in(&owner);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&member);
    let default = default_workspace(&world, &owner);
    let plans = text_channel(&world, &owner, default, "plans");
    let api = owner.api();
    until_model(&world, &member, "the member holds the channel", |model| {
        model.channel(plans).is_some()
    });

    window.open_administration();
    open_page(&mut window, "admin-nav-members", |window| {
        window.shows(sel("member-permissions", bruno))
    });
    click_in_page(&mut window, sel("member-permissions", bruno));
    window.until("the page of the member is open", |window| {
        window.shows("person-nav-permissions")
            && window.shows("person-nav-access")
            && window.shows("override-mention_everyone-allow")
    });

    // An allow and a deny in the workspace reach the member.
    choose(&mut window, "mention_everyone", "allow");
    choose(&mut window, "send_messages", "deny");
    let stored = world.run(api.member_permissions(default, bruno)).unwrap();
    assert_eq!(stored.allow, [WorkspacePermission::MentionEveryone]);
    assert_eq!(stored.deny, [WorkspacePermission::SendMessages]);
    until_model(
        &world,
        &member,
        "the member holds the new permissions",
        |model| {
            model.workspace(default).is_some_and(|workspace| {
                workspace.holds(WorkspacePermission::MentionEveryone)
                    && !workspace.holds(WorkspacePermission::SendMessages)
            })
        },
    );

    // What the member can use: each channel with the permissions of the member there.
    click_in_page(&mut window, "person-nav-access");
    window.until("the page lists the channels", |window| {
        window.shows(sel("access-channel", plans))
    });
    let used = awaited(
        &mut window,
        "the page lists what the member can use",
        |window| {
            let Some(Scope::Person(settings)) = window.admin().map(|view| view.scope) else {
                return None;
            };
            let PersonPageView::Access(page) = settings.page else {
                return None;
            };
            page.channels
                .loaded()?
                .iter()
                .find(|line| line.channel_id == plans)
                .map(|line| line.permissions.clone())
        },
    );
    assert!(
        used.contains(&WorkspacePermission::MentionEveryone),
        "{used:?}"
    );
    assert!(
        !used.contains(&WorkspacePermission::SendMessages),
        "{used:?}"
    );
    assert!(used.contains(&WorkspacePermission::ViewChannel), "{used:?}");

    // Neutral removes both.
    click_in_page(&mut window, "person-nav-permissions");
    choose(&mut window, "mention_everyone", "neutral");
    choose(&mut window, "send_messages", "neutral");
    let stored = world.run(api.member_permissions(default, bruno)).unwrap();
    assert!(
        stored.allow.is_empty() && stored.deny.is_empty(),
        "{stored:?}"
    );

    // The own account takes no change on this page.
    click_in_page(&mut window, "person-nav-back");
    click_in_page(&mut window, sel("member-permissions", ada));
    window.until(
        "the page states that the own account takes no change",
        |window| window.shows("person-locked"),
    );
    sends_nothing(&mut window, "override-mention_everyone-allow");
}

#[gpui::test]
fn a_member_who_manages_roles_sees_no_hidden_channel_and_no_choice_beyond_own_permissions_or_rank(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca = window.view().person.account_id.unwrap();
    let code = invite(&world, &admin.handle, Vec::new());
    let (_bruno_device, bruno_handle) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&bruno_handle);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_cara_device, cara_handle) = joined(&world, &server, &code, "Cara");
    let cara = logged_in(&cara_handle);
    let default = default_workspace(&world, &admin.handle);
    let api = admin.handle.api();
    // A role above the role of Luca, for Cara.
    let seniors = role(&world, &admin.handle, default, "Seniors", &[]);
    world
        .run(api.assign_workspace_role(default, cara, seniors))
        .unwrap();
    grant_workspace(
        &world,
        &admin.handle,
        default,
        luca,
        WorkspacePermission::ManageRoles,
    );
    let everyone = everyone_of(&world, &admin.handle, default);
    let plans = text_channel(&world, &admin.handle, default, "plans");
    let secret = text_channel(&world, &admin.handle, default, "secret");
    world
        .run(api.change_channel_rule(
            secret,
            RuleSubject::Role { role_id: everyone },
            WorkspacePermission::ViewChannel,
            OverrideState::Deny,
        ))
        .unwrap();
    world
        .run(api.change_channel_rule(
            secret,
            RuleSubject::Person { account_id: bruno },
            WorkspacePermission::ViewChannel,
            OverrideState::Allow,
        ))
        .unwrap();
    until_model(
        &world,
        &bruno_handle,
        "Bruno sees the private channel",
        |model| model.channel(secret).is_some(),
    );

    window.open_administration();
    open_page(&mut window, "admin-nav-members", |window| {
        window.shows(sel("member-permissions", bruno))
    });
    click_in_page(&mut window, sel("member-permissions", bruno));
    window.until("the page of the member is open", |window| {
        window.shows("override-manage_messages-allow")
    });

    // Luca does not hold "manage messages": its allow is refused with the reason. A deny
    // needs only "manage roles".
    window.until("the page states the reason", |window| {
        window.shows("override-ceiling")
    });
    sends_nothing(&mut window, "override-manage_messages-allow");
    choose(&mut window, "add_reactions", "deny");
    let stored = world.run(api.member_permissions(default, bruno)).unwrap();
    assert_eq!(stored.deny, [WorkspacePermission::AddReactions]);
    // The removal of the deny would give Bruno "add reactions" in the private channel too,
    // which Luca cannot see: the server refuses it, and the page states the refusal.
    let refused = choose_answer(&mut window, "add_reactions", "neutral");
    assert!(refused.is_some());
    window.until("the page states the refusal", |window| {
        try_reveal(window, "person-refused").is_some()
    });
    let stored = world.run(api.member_permissions(default, bruno)).unwrap();
    assert_eq!(stored.deny, [WorkspacePermission::AddReactions]);

    // What Bruno can use lists the channels that Luca sees, and not the private one.
    click_in_page(&mut window, "person-nav-access");
    window.until("the page lists the open channel", |window| {
        window.shows(sel("access-channel", plans))
    });
    assert!(!window.shows(sel("access-channel", secret)));

    // Cara holds a role above the highest role of Luca: her page takes no change.
    click_in_page(&mut window, "person-nav-back");
    click_in_page(&mut window, sel("member-permissions", cara));
    window.until("the page states the order of the roles", |window| {
        window.shows("person-locked")
    });
    sends_nothing(&mut window, "override-add_reactions-deny");
    let stored = world.run(api.member_permissions(default, cara)).unwrap();
    assert!(stored.deny.is_empty());
}

#[gpui::test]
fn an_administrator_sets_the_server_permissions_of_an_account_and_only_the_owner_denies_administrator(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let ada = logged_in(&owner);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&member);
    let api = owner.api();

    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        window.shows(sel("account-permissions", bruno))
    });
    click_in_page(&mut window, sel("account-permissions", bruno));
    window.until(
        "the server permissions of the account are shown",
        |window| window.shows("person-nav-server") && window.shows("override-create_invites-allow"),
    );
    choose(&mut window, "create_invites", "allow");
    until_model(
        &world,
        &member,
        "the account holds the permission",
        |model| {
            model
                .account
                .server_permissions
                .contains(&ServerPermission::CreateInvites)
        },
    );
    // The owner denies the administrator permission, which a role could give.
    choose(&mut window, "administrator", "deny");
    let stored = world.run(api.server_permissions(bruno)).unwrap();
    assert_eq!(stored.allow, [ServerPermission::CreateInvites]);
    assert_eq!(stored.deny, [ServerPermission::Administrator]);
    choose(&mut window, "administrator", "neutral");
    choose(&mut window, "create_invites", "neutral");
    let stored = world.run(api.server_permissions(bruno)).unwrap();
    assert!(
        stored.allow.is_empty() && stored.deny.is_empty(),
        "{stored:?}"
    );

    // The account of the owner takes no rule.
    click_in_page(&mut window, "person-nav-back");
    click_in_page(&mut window, sel("account-permissions", ada));
    window.until("the page states that the owner takes no rule", |window| {
        window.shows("person-owner-no-rules")
    });
    assert!(!window.shows("override-create_invites-allow"));
}

#[gpui::test]
fn an_administrator_who_is_not_the_owner_cannot_deny_the_administrator_permission(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca = window.view().person.account_id.unwrap();
    grant_server(&world, &admin.handle, luca, ServerPermission::Administrator);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&member);
    let api = admin.handle.api();

    window.open_administration();
    open_page(&mut window, "admin-nav-people", |window| {
        window.shows(sel("account-permissions", bruno))
    });
    click_in_page(&mut window, sel("account-permissions", bruno));
    window.until(
        "the deny of the administrator permission states its reason",
        |window| window.shows("override-administrator-deny") && window.shows("override-ceiling"),
    );
    sends_nothing(&mut window, "override-administrator-deny");
    choose(&mut window, "create_invites", "allow");
    let stored = world.run(api.server_permissions(bruno)).unwrap();
    assert_eq!(stored.allow, [ServerPermission::CreateInvites]);
    assert!(stored.deny.is_empty());
}

#[gpui::test]
fn a_member_who_may_set_the_user_limit_alone_changes_it_on_the_overview(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca = window.view().person.account_id.unwrap();
    let default = default_workspace(&world, &admin.handle);
    let api = admin.handle.api();
    let channel = CreateChannel {
        name: "Huddle".to_owned(),
        kind: ChannelKind::Voice,
        category_id: None,
        position: None,
    };
    let huddle = world.run(api.create_channel(default, &channel)).unwrap().id;
    grant_workspace(
        &world,
        &admin.handle,
        default,
        luca,
        WorkspacePermission::SetCallUserLimit,
    );

    // Without "manage channel" the overview holds the user limit and no other setting.
    open_channel_settings(&mut window, "Huddle", huddle);
    open_page(&mut window, "channel-nav-overview", |window| {
        window.shows("channel-limit")
    });
    assert!(!window.shows("channel-nav-permissions"));
    assert!(!window.shows("channel-rename") && !window.shows("channel-hide"));
    click_in_page(&mut window, "channel-limit-raise");
    window.until("the change is answered", idle);
    let listed = world.run(api.channels(default)).unwrap();
    assert!(
        listed
            .channels
            .iter()
            .any(|channel| channel.id == huddle && channel.user_limit.is_some()),
        "{listed:?}"
    );
}
