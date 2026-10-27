//! Hidden categories and channels in the channel column of a member (ADR 0304, ADR 0730):
//! a member sees a category only when it holds a channel that the member sees, or when the
//! member can manage the category. The rules change through the API while the window is
//! open.

use gpui::TestAppContext;
use junto_client::state::view::SectionTitle;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{CategoryId, ChannelId, WorkspaceId};
use junto_client_core::protocol::structure::{
    ChannelKind, CreateCategory, CreateChannel, RuleSubject, SetChannelOverride,
};

use crate::admin_pages::{default_workspace, invite, joined};
use crate::channel_use::everyone;
use crate::harness::{Admin, Window, World, member};
use crate::messages::document;

const VIEW: WorkspacePermission = WorkspacePermission::ViewChannel;

fn category(world: &World, admin: &Admin, workspace_id: WorkspaceId, name: &str) -> CategoryId {
    let category = CreateCategory {
        name: name.to_owned(),
        position: None,
    };
    world
        .run(admin.handle.api().create_category(workspace_id, &category))
        .unwrap()
        .id
}

fn channel_in(
    world: &World,
    admin: &Admin,
    (workspace_id, category_id): (WorkspaceId, CategoryId),
    name: &str,
) -> ChannelId {
    let channel = CreateChannel {
        name: name.to_owned(),
        kind: ChannelKind::Text,
        category_id: Some(category_id),
        position: None,
    };
    world
        .run(admin.handle.api().create_channel(workspace_id, &channel))
        .unwrap()
        .id
}

fn sets(allow: &[WorkspacePermission], deny: &[WorkspacePermission]) -> SetChannelOverride {
    SetChannelOverride {
        allow: allow.to_vec(),
        deny: deny.to_vec(),
    }
}

fn channel_rule(
    world: &World,
    admin: &Admin,
    channel_id: ChannelId,
    subject: RuleSubject,
    (allow, deny): (&[WorkspacePermission], &[WorkspacePermission]),
) {
    world
        .run(
            admin
                .handle
                .api()
                .set_channel_rule(channel_id, subject, &sets(allow, deny)),
        )
        .unwrap();
}

fn category_rule(
    world: &World,
    admin: &Admin,
    place: (WorkspaceId, CategoryId),
    subject: RuleSubject,
    (allow, deny): (&[WorkspacePermission], &[WorkspacePermission]),
) {
    world
        .run(
            admin
                .handle
                .api()
                .set_category_rule(place, subject, &sets(allow, deny)),
        )
        .unwrap();
}

/// The sections of the channel column as the window drew them: each title with the names
/// of its channels.
fn column(window: &mut Window) -> Vec<(String, Vec<String>)> {
    window
        .view()
        .workspace()
        .map(|workspace| {
            workspace
                .sections
                .iter()
                .map(|section| {
                    let title = match &section.title {
                        SectionTitle::TextChannels => "Text channels".to_owned(),
                        SectionTitle::VoiceChannels => "Voice channels".to_owned(),
                        SectionTitle::Category(name) => name.clone(),
                    };
                    let mut rows: Vec<String> =
                        section.rows.iter().map(|row| row.name.clone()).collect();
                    rows.sort();
                    (title, rows)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn section<'a>(sections: &'a [(String, Vec<String>)], title: &str) -> Option<&'a [String]> {
    sections
        .iter()
        .find(|(held, _)| held == title)
        .map(|(_, rows)| rows.as_slice())
}

fn lists_channel(sections: &[(String, Vec<String>)], name: &str) -> bool {
    sections
        .iter()
        .any(|(_, rows)| rows.iter().any(|row| row == name))
}

/// The names of the categories that the client core of the window holds in a workspace:
/// the categories that the server listed for the person.
fn held_categories(window: &mut Window, workspace_id: WorkspaceId) -> Vec<String> {
    let server_id = window.view().selected.map(|server| server.id).unwrap();
    window.state.read_with(&window.cx, |state, _| {
        state
            .server(server_id)
            .and_then(|server| server.live().view().model)
            .and_then(|model| {
                model.workspace(workspace_id).map(|workspace| {
                    workspace
                        .categories
                        .iter()
                        .map(|category| category.name.clone())
                        .collect()
                })
            })
            .unwrap_or_default()
    })
}

/// Fails when a part of what the window drew last names one of `names`: the channel
/// column, the header, the messages, the panels and every other part of the drawn state.
#[track_caller]
fn assert_names_nothing_of(window: &mut Window, names: &[&str]) {
    let drawn = format!("{:?}", window.view());
    for name in names {
        assert!(!drawn.contains(name), "the window names {name:?}");
    }
}

/// A category with one channel that the member must not see, with a message in it, and a
/// category with a channel that the member sees.
struct Scene {
    world: World,
    /// The server process lives as long as the scene.
    server: crate::support::TestServer,
    admin: Admin,
    workspace_id: WorkspaceId,
    finance: CategoryId,
    ledger: ChannelId,
}

const FINANCE: &str = "Quarterly Finance";
const LEDGER: &str = "ledger-q3";
const TOTALS: &str = "Totals of the quarter";
const PROJECTS: &str = "Projects";
const PLANS: &str = "plans";

fn scene() -> Scene {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let workspace_id = default_workspace(&world, &admin.handle);
    let finance = category(&world, &admin, workspace_id, FINANCE);
    let ledger = channel_in(&world, &admin, (workspace_id, finance), LEDGER);
    admin.send(&world, ledger, document(TOTALS));
    let projects = category(&world, &admin, workspace_id, PROJECTS);
    channel_in(&world, &admin, (workspace_id, projects), PLANS);
    Scene {
        world,
        server,
        admin,
        workspace_id,
        finance,
        ledger,
    }
}

fn shows_projects(sections: &[(String, Vec<String>)]) -> bool {
    section(sections, PROJECTS) == Some([PLANS.to_owned()].as_slice())
}

fn shows_finance(sections: &[(String, Vec<String>)]) -> bool {
    section(sections, FINANCE) == Some([LEDGER.to_owned()].as_slice())
}

fn hides_finance(sections: &[(String, Vec<String>)]) -> bool {
    section(sections, FINANCE).is_none() && !lists_channel(sections, LEDGER)
}

#[gpui::test]
fn a_category_whose_only_channel_everyone_may_not_see_is_not_in_the_column(
    cx: &mut TestAppContext,
) {
    let scene = scene();
    let Scene {
        world,
        admin,
        workspace_id,
        ledger,
        ..
    } = &scene;
    let role_id = everyone(world, admin, *workspace_id);
    channel_rule(
        world,
        admin,
        *ledger,
        RuleSubject::Role { role_id },
        (&[], &[VIEW]),
    );

    let (_directory, mut window) = member(cx, world, &scene.server.address(), admin, "Bruno");
    let sections = window.until_read(
        "the column shows the category with a visible channel",
        column,
        |sections| shows_projects(sections),
    );
    assert!(hides_finance(&sections), "{sections:?}");
    assert!(
        !held_categories(&mut window, *workspace_id).contains(&FINANCE.to_owned()),
        "the server listed the hidden category"
    );
    assert_names_nothing_of(&mut window, &[FINANCE, LEDGER, TOTALS]);
}

#[gpui::test]
fn a_person_rule_that_hides_the_only_channel_of_a_category_takes_both_from_the_column(
    cx: &mut TestAppContext,
) {
    let scene = scene();
    let Scene {
        world,
        server,
        admin,
        workspace_id,
        finance,
        ledger,
    } = &scene;
    let (_directory, mut window) = member(cx, world, &server.address(), admin, "Bruno");
    let bruno = window.own_account();
    window.until_read("the member sees both categories", column, |sections| {
        shows_finance(sections) && shows_projects(sections)
    });

    channel_rule(
        world,
        admin,
        *ledger,
        RuleSubject::Person { account_id: bruno },
        (&[], &[VIEW]),
    );
    let sections = window.until_read(
        "the category and its channel leave the column",
        column,
        |sections| hides_finance(sections),
    );
    assert!(shows_projects(&sections), "{sections:?}");
    assert_names_nothing_of(&mut window, &[FINANCE, LEDGER, TOTALS]);

    // The rule of one person hides nothing from another member.
    let code = invite(world, &admin.handle, Vec::new());
    let (_device, carla) = joined(world, server, &code, "Carla");
    let listed = world.run(carla.api().channels(*workspace_id)).unwrap();
    assert!(listed.categories.iter().any(|held| held.id == *finance));
    assert!(listed.channels.iter().any(|held| held.id == *ledger));
}

#[gpui::test]
fn a_hidden_category_and_channel_follow_their_rule_while_the_window_is_open(
    cx: &mut TestAppContext,
) {
    let scene = scene();
    let Scene {
        world,
        server,
        admin,
        workspace_id,
        ledger,
        ..
    } = &scene;
    let role_id = everyone(world, admin, *workspace_id);
    let subject = RuleSubject::Role { role_id };
    channel_rule(world, admin, *ledger, subject, (&[], &[VIEW]));
    let (_directory, mut window) = member(cx, world, &server.address(), admin, "Bruno");
    let sections = window.until_read("the column is read", column, |sections| {
        shows_projects(sections)
    });
    assert!(hides_finance(&sections), "{sections:?}");
    assert_names_nothing_of(&mut window, &[FINANCE, LEDGER, TOTALS]);

    // An allow of the "everyone" role: the category and the channel appear.
    channel_rule(world, admin, *ledger, subject, (&[VIEW], &[]));
    window.until_read("the category and its channel appear", column, |sections| {
        shows_finance(sections) && shows_projects(sections)
    });
    window.click_channel(LEDGER);
    window.until_message(TOTALS);
    assert_eq!(window.selected_channel(), *ledger);

    // The deny again: both leave, and the open channel is no longer shown.
    channel_rule(world, admin, *ledger, subject, (&[], &[VIEW]));
    let sections = window.until_read(
        "the category and its channel leave the column and the main area",
        column,
        |sections| hides_finance(sections),
    );
    assert!(shows_projects(&sections), "{sections:?}");
    window.until("the main area shows another channel", |window| {
        window.selected_channel() != *ledger
    });
    assert_names_nothing_of(&mut window, &[FINANCE, LEDGER, TOTALS]);
}

#[gpui::test]
fn a_category_rule_hides_its_synced_channels_and_keeps_a_channel_with_its_own_allow(
    cx: &mut TestAppContext,
) {
    let scene = scene();
    let Scene {
        world,
        server,
        admin,
        workspace_id,
        finance,
        ledger,
    } = &scene;
    let place = (*workspace_id, *finance);
    let payroll = channel_in(world, admin, place, "payroll-q3");
    let budget = channel_in(world, admin, place, "budget-q3");
    let records = category(world, admin, *workspace_id, "Old Records");
    let minutes = channel_in(world, admin, (*workspace_id, records), "minutes-2025");
    let role_id = everyone(world, admin, *workspace_id);
    let subject = RuleSubject::Role { role_id };
    let (_directory, mut window) = member(cx, world, &server.address(), admin, "Bruno");
    window.until_read("the member sees every channel", column, |sections| {
        section(sections, FINANCE).is_some_and(|rows| rows == ["budget-q3", LEDGER, "payroll-q3"])
            && section(sections, "Old Records").is_some_and(|rows| rows == ["minutes-2025"])
    });

    // The own rule of "budget-q3" detaches it from its category.
    channel_rule(world, admin, budget, subject, (&[VIEW], &[]));
    let api = admin.handle.api();
    assert!(!world.run(api.channel_rules(budget)).unwrap().synced);
    for synced in [*ledger, payroll, minutes] {
        assert!(world.run(api.channel_rules(synced)).unwrap().synced);
    }

    // A deny of "view channel" in both categories.
    category_rule(world, admin, place, subject, (&[], &[VIEW]));
    category_rule(
        world,
        admin,
        (*workspace_id, records),
        subject,
        (&[], &[VIEW]),
    );
    let sections = window.until_read(
        "the synced channels and the category without a visible channel leave",
        column,
        |sections| {
            section(sections, FINANCE).is_some_and(|rows| rows == ["budget-q3"])
                && section(sections, "Old Records").is_none()
        },
    );
    assert!(shows_projects(&sections), "{sections:?}");
    assert!(!lists_channel(&sections, "minutes-2025"), "{sections:?}");
    assert_names_nothing_of(
        &mut window,
        &[LEDGER, TOTALS, "payroll-q3", "Old Records", "minutes-2025"],
    );
}

#[gpui::test]
fn a_member_who_manages_a_category_sees_it_without_channels(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let workspace_id = default_workspace(&world, &admin.handle);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Bruno");
    let bruno = window.own_account();
    let board = category(&world, &admin, workspace_id, "Planning Board");
    category(&world, &admin, workspace_id, "Spare Shelf");
    category_rule(
        &world,
        &admin,
        (workspace_id, board),
        RuleSubject::Person { account_id: bruno },
        (&[WorkspacePermission::ManageChannel], &[]),
    );

    // The server lists the category to the member, and the column shows it.
    window.until_read(
        "the client core holds the managed category",
        |window| held_categories(window, workspace_id),
        |held| held.iter().any(|name| name == "Planning Board"),
    );
    let sections = window.until_read(
        "the column shows the managed category without channels",
        column,
        |sections| section(sections, "Planning Board").is_some_and(<[String]>::is_empty),
    );
    assert!(section(&sections, "Spare Shelf").is_none(), "{sections:?}");
    assert!(!held_categories(&mut window, workspace_id).contains(&"Spare Shelf".to_owned()));
    assert_names_nothing_of(&mut window, &["Spare Shelf"]);
}

#[gpui::test]
fn a_category_leaves_the_column_with_its_last_channel_unless_the_member_manages_it(
    cx: &mut TestAppContext,
) {
    let scene = scene();
    let Scene {
        world,
        server,
        admin,
        workspace_id,
        ledger,
        ..
    } = &scene;
    let budgets = category(world, admin, *workspace_id, "Budgets");
    let budget = channel_in(world, admin, (*workspace_id, budgets), "budget-q3");
    let records = category(world, admin, *workspace_id, "Old Records");
    let minutes = channel_in(world, admin, (*workspace_id, records), "minutes-2025");
    let (_directory, mut window) = member(cx, world, &server.address(), admin, "Bruno");
    let bruno = window.own_account();
    category_rule(
        world,
        admin,
        (*workspace_id, records),
        RuleSubject::Person { account_id: bruno },
        (&[WorkspacePermission::ManageChannel], &[]),
    );
    window.until_read("the member sees every category", column, |sections| {
        shows_finance(sections)
            && section(sections, "Budgets").is_some_and(|rows| rows == ["budget-q3"])
            && section(sections, "Old Records").is_some_and(|rows| rows == ["minutes-2025"])
    });

    // The deletion of the last channel of a category: the server lists it no longer.
    let api = admin.handle.api();
    world.run(api.delete_channel(*ledger)).unwrap();
    window.until_read(
        "the client core holds the category no longer",
        |window| held_categories(window, *workspace_id),
        |held| !held.iter().any(|name| name == FINANCE),
    );
    let sections = column(&mut window);
    assert!(hides_finance(&sections), "{sections:?}");

    // The archive of the last channel of a category, which the member does not manage,
    // and of one that the member manages.
    world.run(api.archive_channel(budget)).unwrap();
    world.run(api.archive_channel(minutes)).unwrap();
    let sections = window.until_read(
        "only the managed category stays, without channels",
        column,
        |sections| {
            section(sections, "Budgets").is_none()
                && section(sections, "Old Records").is_some_and(<[String]>::is_empty)
        },
    );
    assert!(shows_projects(&sections), "{sections:?}");
    assert!(!held_categories(&mut window, *workspace_id).contains(&"Budgets".to_owned()));
    assert_names_nothing_of(
        &mut window,
        &[FINANCE, LEDGER, TOTALS, "Budgets", "budget-q3"],
    );
}
