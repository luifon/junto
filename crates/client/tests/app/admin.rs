//! The administration in the window against the real server (ADR 0761, ADR 0762): what an
//! administrator does reaches the window of a member without a reload, and an
//! administrator manages workspaces, categories and channels in the window.

use gpui::TestAppContext;
use junto_client::state::admin::view::{AdminView, ChannelPageView, Privacy, Scope, ServerPage};
use junto_client::state::admin::{AdminOpen, AdminPage, ChannelPage};
use junto_client::state::view::ServerStatus;
use junto_client_core::ServerHandle;
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::ids::{ChannelId, WorkspaceId};
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel, UpdateChannel};

use crate::harness::{Admin, Window, World, member, named, selector};
use crate::support::{Device, FaultProxy, join, logged_in};

impl Admin {
    fn default_workspace(&self, world: &World) -> WorkspaceId {
        world
            .run(self.handle.api().workspaces())
            .unwrap()
            .into_iter()
            .find(|workspace| workspace.is_default)
            .unwrap()
            .id
    }

    fn channel(&self, world: &World, workspace_id: WorkspaceId, name: &str) -> ChannelId {
        let channel = CreateChannel {
            name: name.to_owned(),
            kind: ChannelKind::Text,
            category_id: None,
            position: None,
        };
        world
            .run(self.handle.api().create_channel(workspace_id, &channel))
            .unwrap()
            .id
    }
}

impl Window {
    /// The administration as the window holds it now.
    pub(crate) fn admin(&mut self) -> Option<AdminView> {
        self.cx.run_until_parked();
        self.state
            .read_with(&self.cx, |state, _| state.admin_view())
    }

    /// Opens the administration through the menu of the server.
    #[track_caller]
    pub(crate) fn open_administration(&mut self) {
        self.click("server-menu-button");
        self.click("menu-administration");
        assert!(self.shows("admin"));
    }

    fn workspace_names(&mut self) -> Vec<String> {
        self.view()
            .servers
            .first()
            .map(|server| {
                server
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Types a name into the open form of the administration and sends the form.
    #[track_caller]
    fn send_form(&mut self, name: &str) {
        self.in_shell(|shell, window, cx| shell.type_admin_name(name, window, cx));
        self.click("admin-form-send");
    }
}

/// The state of a client without a window, once it holds what `holds` asks for.
fn until_model(
    world: &World,
    handle: &ServerHandle,
    what: &str,
    holds: impl Fn(&ServerModel) -> bool,
) {
    let found = world.run(async {
        let mut watch = handle.live().watch();
        let wait = watch.wait_for(|view| view.model.as_deref().is_some_and(&holds));
        tokio::time::timeout(crate::harness::PATIENCE, wait)
            .await
            .is_ok_and(|view| view.is_some())
    });
    assert!(found, "the state of the client never held this: {what}");
}

#[gpui::test]
fn what_an_administrator_does_reaches_the_window_of_a_member(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let api = admin.handle.api();
    let default = admin.default_workspace(&world);

    // A new channel.
    let plans = admin.channel(&world, default, "plans");
    window.until("the new channel is in the channel list", |window| {
        window.shows_channel("plans")
    });

    // The member shows the channel while the administrator archives it: the channel
    // leaves the list, and the main area says that it is archived.
    window.click_channel("plans");
    window.until("the channel is shown", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.channel.as_ref())
            .is_some_and(|channel| channel.id == plans)
            && window.shows("composer")
    });
    world.run(api.archive_channel(plans)).unwrap();
    window.until("the channel is shown as archived", |window| {
        window.shows("archived-bar") && window.shows("archived-mark")
    });
    assert!(
        !window.shows("composer"),
        "an archived channel has no composer"
    );
    assert!(!window.shows_channel("plans"));
    let view = window.view();
    let workspace = view.workspace().unwrap();
    assert_eq!(workspace.channel.as_ref().unwrap().id, plans);
    let archived = workspace.archived.unwrap();
    assert!(!archived.may_restore, "a member may not restore");
    assert!(archived.deletes_at.is_some());
    assert!(!window.shows("archived-restore"));

    // The member finds the channel in the archived list of the administration.
    window.open_administration();
    window.click("admin-nav-archive");
    window.until("the archived list holds the channel", |window| {
        window.shows(Box::leak(format!("archived-row-{plans}").into_boxed_str()))
    });
    window.click("admin-close");
    assert!(!window.shows("admin"));

    // A restored channel takes messages again.
    world.run(api.restore_channel(plans)).unwrap();
    window.until("the channel is a channel of the list again", |window| {
        window.shows_channel("plans") && window.shows("composer") && !window.shows("archived-bar")
    });
    assert!(window.view().workspace().unwrap().archived.is_none());

    // A further workspace that the member is added to, and its deletion.
    let own = window.view().person.account_id.unwrap();
    let lab = world.run(api.create_workspace("Lab".to_owned())).unwrap();
    world.run(api.add_workspace_member(lab.id, own)).unwrap();
    window.until("the server book lists the new workspace", |window| {
        window.workspace_names().contains(&"Lab".to_owned())
    });
    world.run(api.delete_workspace(lab.id)).unwrap();
    window.until("the deleted workspace is gone", |window| {
        !window.workspace_names().contains(&"Lab".to_owned())
    });
    assert_eq!(window.status(), Some(ServerStatus::Ready));

    // A ban ends the session of the member on screen.
    world.run(api.ban_account(own)).unwrap();
    window.until("the window shows the ban", |window| {
        window.status() == Some(ServerStatus::Banned)
    });
    assert!(!window.shows("composer"));
}

#[gpui::test]
fn an_administrator_manages_workspaces_and_channels_in_the_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    // The window claims the server; a client without a window is a member.
    let (_directory, mut window) = named(cx, "Ada");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    let owner = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    let code = world
        .run(owner.api().create_invite(&Default::default()))
        .unwrap()
        .code;
    let device = Device::new();
    let other = world.run(async {
        let handle = device.add(&server.address()).await;
        handle.dispatch(join(&code, "Bruno")).await.unwrap();
        logged_in(&handle);
        device.client.start_event_streams().unwrap();
        device.client.select(Some(handle.id()));
        handle
    });
    until_model(&world, &other, "the first state", |_| true);

    // The pages of the owner.
    window.open_administration();
    let view = window.admin().unwrap();
    let Scope::Server { nav, page, .. } = &view.scope else {
        panic!("the administration of the server is open");
    };
    assert!(
        nav.iter()
            .any(|group| group.pages.contains(&AdminPage::Tokens))
    );
    let ServerPage::Workspaces(page) = page else {
        panic!("the first page is the page of the workspaces");
    };
    assert!(page.may_create && page.may_manage && page.is_owner);
    let default = page.rows.iter().find(|row| row.is_default).unwrap().id;

    // A name is asked for before a workspace is created.
    window.click("workspace-new");
    window.click("admin-form-send");
    assert!(window.shows("admin-form-invalid"));
    window.send_form("Lab");
    window.until("the new workspace is in the list", |window| {
        window.workspace_names().contains(&"Lab".to_owned()) && !window.shows("workspace-name")
    });
    let lab = window
        .view()
        .servers
        .first()
        .unwrap()
        .workspaces
        .iter()
        .find(|workspace| workspace.name == "Lab")
        .unwrap()
        .id;

    // A new name.
    window.click(Box::leak(
        format!("workspace-rename-{lab}").into_boxed_str(),
    ));
    window.send_form("Laboratory");
    window.until("the workspace has its new name", |window| {
        window.workspace_names().contains(&"Laboratory".to_owned())
    });

    // The deletion asks first, and the default workspace offers none.
    window.click(Box::leak(
        format!("workspace-delete-{lab}").into_boxed_str(),
    ));
    assert!(window.shows("workspace-delete-confirm"));
    window.click("workspace-delete-cancel");
    assert!(!window.shows("workspace-delete-confirm"));
    window.click(Box::leak(
        format!("workspace-delete-{default}").into_boxed_str(),
    ));
    assert!(
        !window.shows("workspace-delete-confirm"),
        "the default workspace cannot be deleted"
    );
    window.click(Box::leak(
        format!("workspace-delete-{lab}").into_boxed_str(),
    ));
    window.click("workspace-delete-confirm");
    window.until("the workspace is deleted", |window| {
        !window.workspace_names().contains(&"Laboratory".to_owned())
    });

    // A category and a channel in it; the member gets both.
    window.click("admin-nav-channels");
    window.click("category-new");
    window.send_form("Projects");
    window.until("the category is on the page", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Server { page: ServerPage::Channels(page), .. })
                if page.categories().any(|category| category.name == "Projects")
        )
    });
    let Some(Scope::Server {
        page: ServerPage::Channels(page),
        ..
    }) = window.admin().map(|view| view.scope)
    else {
        panic!("the channels page is open");
    };
    let projects = page.categories().next().unwrap().id;
    window.click(Box::leak(
        format!("channel-new-{projects}").into_boxed_str(),
    ));
    window.send_form("plans");
    until_model(&world, &other, "the new channel in its category", |model| {
        model.workspace(default).is_some_and(|workspace| {
            workspace.channels.iter().any(|held| {
                held.channel.name == "plans" && held.channel.category_id == Some(projects)
            }) && workspace.categories.iter().any(|held| held.id == projects)
        })
    });
    let plans = world
        .run(owner.api().channels(default))
        .unwrap()
        .channels
        .into_iter()
        .find(|channel| channel.name == "plans")
        .unwrap()
        .id;

    // The channel moves out of its category.
    window.until("the channel has its row", |window| {
        window.shows(Box::leak(format!("channel-move-{plans}").into_boxed_str()))
    });
    window.click(Box::leak(format!("channel-move-{plans}").into_boxed_str()));
    window.click("channel-group-none");
    window.click("channel-place-0");
    until_model(&world, &other, "the channel without a category", |model| {
        model
            .channel(plans)
            .is_some_and(|held| held.channel.category_id.is_none())
    });
    // The row shows its actions again when the window has the answer to its own request,
    // which can come after the member has the event of the server.
    let settings: &'static str = Box::leak(format!("channel-settings-{plans}").into_boxed_str());
    window.until(
        "the window shows the channel without a category",
        |window| {
            matches!(
                window.admin().map(|view| view.scope),
                Some(Scope::Server { page: ServerPage::Channels(page), .. })
                    if page.channel(plans).is_some_and(|line| line.category_id.is_none())
            ) && window.shows(settings)
        },
    );

    // The settings of the channel: a new name, permanent messages, and the archive.
    window.click(settings);
    window.until("the settings of the channel are open", |window| {
        window.shows("channel-nav-overview") && window.shows("channel-rename")
    });
    window.click("channel-rename");
    window.send_form("roadmap");
    until_model(&world, &other, "the new name of the channel", |model| {
        model
            .channel(plans)
            .is_some_and(|held| held.channel.name == "roadmap")
    });
    window.click("channel-permanent");
    until_model(&world, &other, "permanent messages", |model| {
        model
            .channel(plans)
            .is_some_and(|held| held.channel.permanent_messages)
    });
    window.click("channel-nav-archive");
    window.click("channel-archive");
    window.click("channel-archive-confirm");
    until_model(&world, &other, "the channel left the list", |model| {
        model.channel(plans).is_none()
    });
    window.until("the settings show the archive", |window| {
        window.shows("channel-archived-state") && window.shows("channel-restore")
    });
    let view = window.admin().unwrap();
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.admin_open()),
        Some(AdminOpen::Channel {
            server_id: view.server_id,
            workspace_id: default,
            channel_id: plans,
            page: ChannelPage::Archive,
        })
    );
    // The permissions of the account in an archived channel are read from its route:
    // the restore is offered when they arrived.
    window.until("the restore is offered", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Channel(settings)) if settings.archived && settings.may_manage
        )
    });
    window.click("channel-restore");
    until_model(&world, &other, "the restored channel", |model| {
        model.channel(plans).is_some()
    });

    // The deletion of the channel closes its settings.
    window.until("the channel can be deleted", |window| {
        window.shows("channel-delete") && window.shows("channel-archive")
    });
    window.click("channel-delete");
    window.click("channel-delete-confirm");
    until_model(&world, &other, "the deleted channel", |model| {
        model.channel(plans).is_none()
    });
    window.until("the settings of the deleted channel are closed", |window| {
        !window.shows("admin")
    });
}

#[gpui::test]
fn a_form_that_is_sent_twice_makes_one_request(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    // The window reaches the server through a proxy that holds the creation of a
    // workspace, so that the second Enter falls into the time of the first request.
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = named(cx, "Ada");
    window.find_server(&proxy.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    let owner = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    window.open_administration();

    let created = "/api/v1/workspaces";
    let posts = |proxy: &FaultProxy| {
        proxy
            .seen(created)
            .iter()
            .filter(|seen| seen.method == "POST")
            .count()
    };
    proxy.hold(created);
    window.click("workspace-new");
    window.in_shell(|shell, window, cx| {
        shell.type_admin_name("Lab", window, cx);
        shell.submit_admin_form(window, cx);
        shell.submit_admin_form(window, cx);
    });
    window.until("the request reached the proxy", |_| posts(&proxy) >= 1);
    // A request of the same client that is sent after the two Enters and answered is
    // behind every request that they made.
    world.run(owner.api().own_account()).unwrap();
    assert_eq!(posts(&proxy), 1, "one form made one request");
    assert!(
        window.shows("workspace-name"),
        "the form waits for its answer"
    );

    proxy.release();
    window.until("the workspace is created", |window| {
        window.workspace_names().contains(&"Lab".to_owned()) && !window.shows("workspace-name")
    });
    let labs = world
        .run(owner.api().workspaces())
        .unwrap()
        .into_iter()
        .filter(|workspace| workspace.name == "Lab")
        .count();
    assert_eq!(labs, 1);
    assert_eq!(posts(&proxy), 1);
}

/// A press that starts on a control while the answer to a form above it is on its way, and
/// ends after the window has the answer, reaches that control: the answer does not move
/// the controls under the form (ADR 0730).
#[gpui::test]
fn an_answer_to_a_form_during_a_press_keeps_the_pressed_control_in_place(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = named(cx, "Ada");
    window.find_server(&proxy.address());
    window.join_with_code(&server.owner_code().unwrap());
    window.until_ready();
    let owner = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    let default = world
        .run(owner.api().workspaces())
        .unwrap()
        .into_iter()
        .find(|workspace| workspace.is_default)
        .unwrap()
        .id;
    let channel = CreateChannel {
        name: "plans".to_owned(),
        kind: ChannelKind::Text,
        category_id: None,
        position: None,
    };
    let plans = world
        .run(owner.api().create_channel(default, &channel))
        .unwrap()
        .id;
    window.open_administration();
    window.click("admin-nav-channels");
    let settings = selector(format!("channel-settings-{plans}"));
    window.until("the channel has its row", |window| window.shows(settings));
    window.click(settings);
    window.until("the settings of the channel are open", |window| {
        window.shows("channel-rename") && window.shows("channel-permanent")
    });

    // The new name reaches the server, and its answer waits at the proxy.
    window.click("channel-rename");
    proxy.hold_answer_ending("PATCH", &format!("/channels/{plans}"));
    window.send_form("roadmap");
    window.until("the answer to the rename waits at the proxy", |_| {
        proxy.answers_waiting() == 1
    });
    let before = window.cx.debug_bounds("channel-permanent").unwrap();

    // The press starts while the answer waits and ends after the window has drawn it.
    let position = before.center();
    window.dispatch(gpui::MouseDownEvent {
        position,
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    proxy.release();
    window.until("the window has the answer to the rename", |window| {
        !window.shows("channel-name")
    });
    assert_eq!(
        window.cx.debug_bounds("channel-permanent").unwrap(),
        before,
        "the answer moved the switch under the form"
    );
    window.dispatch(gpui::MouseUpEvent {
        position,
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 1,
    });
    window.until("the switch is on", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Channel(settings))
                if matches!(&settings.page, ChannelPageView::Overview(page) if page.permanent)
        )
    });
}

/// The channels page of the open administration.
fn channels_page(window: &mut Window) -> Option<junto_client::state::admin::view::ChannelsPage> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Channels(page),
            ..
        }) => Some(page),
        _ => None,
    }
}

/// The channels of one group of the channels page, in their order.
fn group_order(
    window: &mut Window,
    category_id: Option<junto_client_core::protocol::ids::CategoryId>,
) -> Vec<ChannelId> {
    channels_page(window)
        .and_then(|page| {
            page.groups
                .into_iter()
                .find(|group| group.category.as_ref().map(|category| category.id) == category_id)
        })
        .map(|group| group.channels.into_iter().map(|line| line.id).collect())
        .unwrap_or_default()
}

/// Drags the handle of a channel onto another place of the channels page, as the pointer
/// does. Returns whether a drag started.
fn drag_channel(window: &mut Window, from: &'static str, to: &'static str) -> bool {
    use gpui::{Modifiers, MouseButton, point, px};
    let start = crate::admin_pages::reveal_in_page(window, from).center();
    window
        .cx
        .simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    window.cx.simulate_mouse_move(
        start + point(px(0.0), px(12.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    let started = window.cx.update(|_, cx| cx.has_active_drag());
    let end = crate::admin_pages::reveal_in_page(window, to).center();
    window
        .cx
        .simulate_mouse_move(end, MouseButton::Left, Modifiers::none());
    window
        .cx
        .simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    window.cx.run_until_parked();
    started
}

#[gpui::test]
fn an_administrator_drags_channels_to_another_place_and_category(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = crate::admin_pages::owner_window(cx, &server);
    let default = crate::admin_pages::default_workspace(&world, &owner);
    let projects = world
        .run(owner.api().create_category(
            default,
            &junto_client_core::protocol::structure::CreateCategory {
                name: "Projects".to_owned(),
                position: None,
            },
        ))
        .unwrap()
        .id;
    let create = |name: &str, category_id| {
        let channel = CreateChannel {
            name: name.to_owned(),
            kind: ChannelKind::Text,
            category_id,
            position: None,
        };
        world
            .run(owner.api().create_channel(default, &channel))
            .unwrap()
            .id
    };
    let plans = create("plans", None);
    let notes = create("notes", None);
    let roadmap = create("roadmap", Some(projects));
    let handle = |id: ChannelId| selector(format!("channel-handle-{id}"));
    let row = |id: ChannelId| selector(format!("channel-line-{id}"));

    window.open_administration();
    window.click("admin-nav-channels");
    window.until("the page shows the channels", |window| {
        group_order(window, Some(projects)) == [roadmap]
            && group_order(window, None).ends_with(&[plans, notes])
    });

    // A channel dropped on a channel of another category takes its place there.
    assert!(drag_channel(&mut window, handle(notes), row(roadmap)));
    // The page shows the move at the drop; the server has done it at the answer.
    window.until("the channel is in the other category", |window| {
        group_order(window, Some(projects)) == [notes, roadmap] && crate::admin_pages::idle(window)
    });
    let held = world.run(owner.api().channels(default)).unwrap().channels;
    let moved = held.iter().find(|channel| channel.id == notes).unwrap();
    assert_eq!(moved.category_id, Some(projects));
    assert!(!group_order(&mut window, None).contains(&notes));

    // Inside a group the channel takes the place of the row that it is dropped on, and
    // the channels between move one place.
    let before = group_order(&mut window, None);
    let first = before[0];
    assert!(drag_channel(&mut window, handle(first), row(plans)));
    let mut expected = before.clone();
    expected.remove(0);
    let at = expected.iter().position(|id| *id == plans).unwrap() + 1;
    expected.insert(at, first);
    window.until("the channel took the place", |window| {
        group_order(window, None) == expected
    });

    // A channel dropped on the heading of a category goes to its end.
    assert!(drag_channel(
        &mut window,
        handle(plans),
        selector(format!("category-row-{projects}"))
    ));
    window.until("the channel is at the end of the category", |window| {
        group_order(window, Some(projects)) == [notes, roadmap, plans]
    });
    assert!(!window.shows("channel-refused"));
}

/// A window of the member "Luca", who holds "manage channel" in the default workspace
/// through a role, with the page of the channels open. The window reaches the server at
/// `address`. Returns the window, the owner and the default workspace.
fn arranger(
    cx: &mut TestAppContext,
    world: &World,
    server: &crate::support::TestServer,
    address: &str,
) -> (tempfile::TempDir, Window, Admin, WorkspaceId) {
    use junto_client_core::protocol::WorkspacePermission;
    use junto_client_core::protocol::roles::CreateWorkspaceRole;
    let owner = world.owner(server);
    let default = owner.default_workspace(world);
    let (directory, mut window) = member(cx, world, address, &owner, "Luca");
    let luca = window.own_account();
    let api = owner.handle.api();
    let role = world
        .run(api.create_workspace_role(
            default,
            &CreateWorkspaceRole {
                name: "Arrangers".to_owned(),
                permissions: vec![WorkspacePermission::ManageChannel],
            },
        ))
        .unwrap();
    world
        .run(api.assign_workspace_role(default, luca, role.id))
        .unwrap();
    // The page of the channels joins the list of pages when the role arrives.
    window.open_administration();
    crate::admin_pages::open_page(&mut window, "admin-nav-channels", |window| {
        channels_page(window).is_some()
    });
    (directory, window, owner, default)
}

/// Takes "manage channel" in one channel from every member.
fn deny_managing(world: &World, owner: &Admin, workspace_id: WorkspaceId, channel_id: ChannelId) {
    use junto_client_core::protocol::WorkspacePermission;
    use junto_client_core::protocol::structure::SetChannelOverride;
    let api = owner.handle.api();
    let everyone = world
        .run(api.workspace_roles(workspace_id))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap();
    world
        .run(api.set_channel_override(
            channel_id,
            everyone.id,
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ManageChannel],
            },
        ))
        .unwrap();
}

/// The category and the position of each channel of a workspace on the server.
fn server_places(
    world: &World,
    owner: &Admin,
    workspace_id: WorkspaceId,
) -> std::collections::HashMap<ChannelId, (Option<junto_client_core::protocol::ids::CategoryId>, i32)>
{
    world
        .run(owner.handle.api().channels(workspace_id))
        .unwrap()
        .channels
        .into_iter()
        .map(|channel| (channel.id, (channel.category_id, channel.position)))
        .collect()
}

#[gpui::test]
fn the_keyboard_reaches_every_place_that_a_drag_reaches(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner, default) = arranger(cx, &world, &server, &server.address());
    let api = owner.handle.api();
    let projects = world
        .run(api.create_category(
            default,
            &junto_client_core::protocol::structure::CreateCategory {
                name: "Projects".to_owned(),
                position: None,
            },
        ))
        .unwrap()
        .id;
    let create = |name: &str, category_id, position| {
        let channel = CreateChannel {
            name: name.to_owned(),
            kind: ChannelKind::Text,
            category_id,
            position,
        };
        world.run(api.create_channel(default, &channel)).unwrap().id
    };
    let x = create("x", None, None);
    let b = create("b", Some(projects), Some(10));
    let c = create("c", Some(projects), Some(20));
    // The member may change "x", and neither "b" nor "c".
    deny_managing(&world, &owner, default, b);
    deny_managing(&world, &owner, default, c);
    window.until("the page shows what the member manages", |window| {
        channels_page(window).is_some_and(|page| {
            page.channel(x).is_some_and(|line| line.may_manage)
                && [b, c]
                    .iter()
                    .all(|id| page.channel(*id).is_some_and(|line| !line.may_manage))
        })
    });
    // A drag between "b" and "c" is offered: it changes "x" alone.
    assert!(
        channels_page(&mut window)
            .unwrap()
            .takes(x, Some(projects), 1)
    );

    // "Move to" offers the same place: the category, and then the place after "b".
    window.click_named(format!("channel-move-{x}"));
    window.click_named(format!("channel-group-{projects}"));
    assert!(window.shows("channel-place-1"));
    window.click("channel-place-1");
    window.until("the channel is between the two", |window| {
        group_order(window, Some(projects)) == [b, x, c] && crate::admin_pages::idle(window)
    });
    let places = server_places(&world, &owner, default);
    assert_eq!(places[&x], (Some(projects), 11));
    assert_eq!(
        (places[&b], places[&c]),
        ((Some(projects), 10), (Some(projects), 20))
    );
    assert!(!window.shows("channel-refused") && !window.shows("channel-place-1"));
}

#[gpui::test]
fn a_move_from_an_old_order_is_refused_and_stays_drawn_until_the_person_acts(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    // The window reaches the server through a proxy, which holds the events of a move by
    // another administrator while the page still shows the order of before.
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner, default) = arranger(cx, &world, &server, &proxy.address());
    let x = owner.channel(&world, default, "x");
    let next = owner.channel(&world, default, "next");
    let last = owner.channel(&world, default, "last");
    window.until("the page shows the three channels", |window| {
        group_order(window, None).ends_with(&[x, next, last])
            && channels_page(window)
                .is_some_and(|page| page.channel(x).is_some_and(|line| line.moves_down.is_ok()))
    });
    let before = server_places(&world, &owner, default);

    // Another administrator moves "last" far down; the window does not know it yet. "Down"
    // puts "x" between "next" and "last" as the page read them, and checks both: the
    // server refuses the move, because "last" is no longer where the page read it.
    proxy.hold_events();
    let moved = UpdateChannel {
        position: Some(before[&last].1 + 50),
        ..UpdateChannel::default()
    };
    world
        .run(owner.handle.api().update_channel(last, &moved))
        .unwrap();
    proxy.hold_request("PATCH", "/channels/positions");
    window.click_named(format!("channel-down-{x}"));
    window.until("the page shows the move", |window| {
        group_order(window, None).ends_with(&[next, x, last])
    });
    let controls = [x, next, last].map(|id| selector(format!("channel-line-{id}")));
    let bounds = |window: &mut Window| {
        window.settle();
        controls.map(|selector| window.cx.debug_bounds(selector).unwrap())
    };
    let pressed = bounds(&mut window);
    let snapshots = |proxy: &FaultProxy| {
        proxy
            .all()
            .iter()
            .filter(|seen| seen.method == "GET" && seen.path.ends_with("/snapshot"))
            .count()
    };
    let read = snapshots(&proxy);
    proxy.release_requests();
    window.until("the server refused the move", |window| {
        window.shows("channel-move-refused")
    });
    // The refusal says that the page read an old order: the state is read again, while the
    // events still wait at the proxy.
    window.until("the state is read again", |_| snapshots(&proxy) > read);

    // The move stays where it is drawn, with the refusal at its row: no control moved, and
    // the page offers no move until the person puts the refusal away.
    assert_eq!(bounds(&mut window), pressed);
    let page = channels_page(&mut window).unwrap();
    assert_eq!(
        page.channel(next).unwrap().moves_up,
        Err(junto_client::state::admin::view::Unplanned::RefusalShown)
    );
    let mut server_now = before.clone();
    server_now.insert(last, (before[&last].0, before[&last].1 + 50));
    assert_eq!(server_places(&world, &owner, default), server_now);
    assert_eq!(bounds(&mut window), pressed);
    // Close shows the order of the state that was read again, with the events still held.
    // The order before the read is the same, so the wait is for the place that only the
    // read state holds.
    crate::admin_pages::click_in_page(&mut window, "channel-move-refused-dismiss");
    window.until(
        "the page shows the order of the state that was read",
        |window| {
            !window.shows("channel-move-refused")
                && group_order(window, None).ends_with(&[x, next, last])
                && channels_page(window).is_some_and(|page| {
                    page.channel(last)
                        .is_some_and(|line| line.place.position == before[&last].1 + 50)
                })
        },
    );
    proxy.release_events();
    assert_eq!(server_places(&world, &owner, default), server_now);
}

#[gpui::test]
fn a_move_changes_the_order_at_the_press_and_no_row_moves_at_its_answer(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    // The request of the move waits at the proxy, so that the answer and the events come
    // at a time that the test chooses.
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner, default) = arranger(cx, &world, &server, &proxy.address());
    let a = owner.channel(&world, default, "a");
    let b = owner.channel(&world, default, "b");
    let c = owner.channel(&world, default, "c");
    window.until("the page shows the three channels", |window| {
        let order = group_order(window, None);
        order.ends_with(&[a, b, c])
    });

    // "Move to" of "a" puts it after "b". The order changes at the press: "b" takes the
    // place of "a", and the form of "a" goes with "a".
    proxy.hold_request("PATCH", "/channels/positions");
    window.click_named(format!("channel-move-{a}"));
    window.click("channel-group-none");
    let place = {
        let order = group_order(&mut window, None);
        let after_b = order.iter().filter(|id| **id != a).position(|id| *id == b);
        after_b.unwrap() + 1
    };
    let place = crate::admin_pages::sel("channel-place", place);
    window.click(place);
    window.until("the page shows the asked order", |window| {
        group_order(window, None).ends_with(&[b, a, c])
    });
    window.settle();
    let controls = [
        selector(format!("channel-settings-{b}")),
        selector(format!("channel-settings-{c}")),
    ];
    let bounds = |window: &mut Window| {
        window.settle();
        controls.map(|selector| window.cx.debug_bounds(selector).unwrap())
    };
    let pressed = bounds(&mut window);

    // The server does the move, and the state holds its events: no control moves.
    proxy.release();
    window.until("the form of the move is closed", |window| {
        !window.shows(place) && channels_page(window).is_some()
    });
    // The channel list of the server follows the state alone.
    window.until("the state holds the move and its answer", |window| {
        let names = window.channel_names();
        let at = |name: &str| names.iter().position(|held| held == name);
        at("b") < at("a")
            && window.admin().is_some_and(|view| {
                !view.sending(junto_client::state::admin::AskKind::MoveChannels(a))
            })
    });
    assert_eq!(bounds(&mut window), pressed);
    assert!(group_order(&mut window, None).ends_with(&[b, a, c]));
    assert!(!window.shows("channel-refused"));
}

#[gpui::test]
fn a_refused_move_to_keeps_the_height_of_its_form(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner, default) = arranger(cx, &world, &server, &proxy.address());
    let a = owner.channel(&world, default, "a");
    // Long names, so that the places of "Move to" take several lines.
    let others: Vec<ChannelId> = (0..10)
        .map(|number| {
            owner.channel(
                &world,
                default,
                &format!("a channel with a long name, number {number}"),
            )
        })
        .collect();
    window.until("the page shows the channels", |window| {
        group_order(window, None).ends_with(&others)
    });

    // "Move to" of "a", after the third of the others.
    window.click_named(format!("channel-move-{a}"));
    window.click("channel-group-none");
    let place = {
        let order = group_order(&mut window, None);
        let rest: Vec<ChannelId> = order.into_iter().filter(|id| *id != a).collect();
        rest.iter().position(|id| *id == others[2]).unwrap() + 1
    };
    let place = crate::admin_pages::sel("channel-place", place);

    // Another administrator moves the channel after the place far down while the events
    // wait: the page sends a move from an old order, which the server refuses.
    proxy.hold_events();
    let position = server_places(&world, &owner, default)[&others[3]].1;
    let moved = UpdateChannel {
        position: Some(position + 100),
        ..UpdateChannel::default()
    };
    world
        .run(owner.handle.api().update_channel(others[3], &moved))
        .unwrap();
    proxy.hold_request("PATCH", "/channels/positions");
    window.click(place);
    window.until("the page shows the move", |window| {
        let order = group_order(window, None);
        let at = |id: ChannelId| order.iter().position(|held| *held == id);
        at(a) == at(others[2]).map(|at| at + 1)
    });
    let rows = [others[3], others[4]].map(|id| selector(format!("channel-line-{id}")));
    // The page draws only the rows in view: the two rows under the form are brought into
    // view with the row of the form.
    crate::admin_pages::reveal_in_page(&mut window, rows[1]);
    let form_row = format!("channel-{a}");
    let bounds = |window: &mut Window| {
        window.settle();
        crate::admin_pages::under_form(window, &form_row, rows[1]);
        rows.map(|selector| window.cx.debug_bounds(selector).unwrap())
    };
    let pressed = bounds(&mut window);
    let form = window
        .cx
        .debug_bounds(selector(format!("channel-line-{a}")))
        .unwrap();
    assert!(
        form.size.height > gpui::px(80.0),
        "the places of the form take several lines: {form:?}"
    );

    // The refusal closes the form; its row keeps the height of the form, and no row under
    // it moves.
    proxy.release_requests();
    window.until("the server refused the move", |window| {
        window.shows("channel-move-refused") && !window.shows(place)
    });
    assert_eq!(bounds(&mut window), pressed);
    crate::admin_pages::click_in_page(&mut window, "channel-move-refused-dismiss");
    window.until("the refusal is put away", |window| {
        !window.shows("channel-move-refused")
    });
    proxy.release_events();
}

#[gpui::test]
fn a_refused_move_of_a_channel_that_is_gone_is_stated_on_the_page(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner, default) = arranger(cx, &world, &server, &proxy.address());
    let x = owner.channel(&world, default, "x");
    let next = owner.channel(&world, default, "next");
    let last = owner.channel(&world, default, "last");
    window.until("the page shows the three channels", |window| {
        group_order(window, None).ends_with(&[x, next, last])
            && channels_page(window)
                .is_some_and(|page| page.channel(x).is_some_and(|line| line.moves_down.is_ok()))
    });

    // "Down" of "x" waits at the proxy while another administrator deletes "x"; the page
    // loses its row, and then the server refuses the move.
    proxy.hold_request("PATCH", "/channels/positions");
    window.click_named(format!("channel-down-{x}"));
    window.until("the request waits", |_| {
        proxy
            .all()
            .iter()
            .any(|seen| seen.method == "PATCH" && seen.path.ends_with("/channels/positions"))
    });
    world.run(owner.handle.api().delete_channel(x)).unwrap();
    window.until("the page lost the channel", |window| {
        channels_page(window).is_some_and(|page| page.channel(x).is_none())
    });
    proxy.release_requests();

    // The refusal has no row: the page states it at its end, with Close, and offers no move
    // until the person puts it away.
    window.until("the page states the refusal", |window| {
        window.shows("channel-move-refused-page")
    });
    assert_eq!(
        channels_page(&mut window)
            .unwrap()
            .channel(last)
            .unwrap()
            .moves_up,
        Err(junto_client::state::admin::view::Unplanned::RefusalShown)
    );
    crate::admin_pages::click_in_page(&mut window, "channel-move-refused-page-dismiss");
    window.until("the page offers moves again", |window| {
        !window.shows("channel-move-refused-page")
            && channels_page(window)
                .is_some_and(|page| page.channel(last).is_some_and(|line| line.moves_up.is_ok()))
    });
}

/// The categories of the default workspace on the server, in their order, with their names.
fn server_categories(
    world: &World,
    owner: &ServerHandle,
    workspace_id: WorkspaceId,
) -> Vec<String> {
    let mut categories = world
        .run(owner.api().channels(workspace_id))
        .unwrap()
        .categories;
    categories.sort_by_key(|category| category.position);
    categories
        .into_iter()
        .map(|category| category.name)
        .collect()
}

/// The names of the categories of the channels page, in the order of the page.
fn page_categories(window: &mut Window) -> Vec<String> {
    channels_page(window)
        .map(|page| {
            page.categories()
                .map(|category| category.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[gpui::test]
fn an_administrator_renames_reorders_and_deletes_categories_in_the_window(cx: &mut TestAppContext) {
    use junto_client_core::protocol::structure::CreateCategory;
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = crate::admin_pages::owner_window(cx, &server);
    let default = crate::admin_pages::default_workspace(&world, &owner);
    let category = |name: &str| {
        world
            .run(owner.api().create_category(
                default,
                &CreateCategory {
                    name: name.to_owned(),
                    position: None,
                },
            ))
            .unwrap()
            .id
    };
    let projects = category("Projects");
    let later = category("Later");
    let plans = world
        .run(owner.api().create_channel(
            default,
            &CreateChannel {
                name: "plans".to_owned(),
                kind: ChannelKind::Text,
                category_id: Some(projects),
                position: None,
            },
        ))
        .unwrap()
        .id;
    window.open_administration();
    window.click("admin-nav-channels");
    window.until("the page shows both categories", |window| {
        page_categories(window) == ["Projects", "Later"]
    });

    // A new name: the form holds the old name, and Save sends the new one.
    crate::admin_pages::click_in_page(&mut window, selector(format!("category-rename-{projects}")));
    assert!(window.shows("category-name"));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.admin_name(cx)),
        "Projects"
    );
    window.send_form("Current projects");
    window.until("the page shows the new name", |window| {
        page_categories(window) == ["Current projects", "Later"] && !window.shows("category-name")
    });
    assert_eq!(
        server_categories(&world, &owner, default),
        ["Current projects", "Later"]
    );

    // Down and Up change the order of the categories on the server and on the page.
    crate::admin_pages::click_in_page(&mut window, selector(format!("category-down-{projects}")));
    window.until("the category is second", |window| {
        page_categories(window) == ["Later", "Current projects"] && crate::admin_pages::idle(window)
    });
    assert_eq!(
        server_categories(&world, &owner, default),
        ["Later", "Current projects"]
    );
    crate::admin_pages::click_in_page(&mut window, selector(format!("category-up-{projects}")));
    window.until("the category is first again", |window| {
        page_categories(window) == ["Current projects", "Later"] && crate::admin_pages::idle(window)
    });
    assert_eq!(
        server_categories(&world, &owner, default),
        ["Current projects", "Later"]
    );

    // The deletion asks first; Cancel keeps the category.
    crate::admin_pages::click_in_page(&mut window, selector(format!("category-delete-{later}")));
    assert!(window.shows("category-delete-confirm"));
    crate::admin_pages::click_in_page(&mut window, "category-delete-cancel");
    assert!(!window.shows("category-delete-confirm"));
    assert_eq!(page_categories(&mut window), ["Current projects", "Later"]);

    // A deleted category with a channel: the channel stays, without a category.
    crate::admin_pages::click_in_page(&mut window, selector(format!("category-delete-{projects}")));
    crate::admin_pages::click_in_page(&mut window, "category-delete-confirm");
    window.until("the category is gone from the page", |window| {
        page_categories(window) == ["Later"]
            && channels_page(window)
                .and_then(|page| page.channel(plans).map(|line| line.category_id))
                == Some(None)
    });
    assert_eq!(server_categories(&world, &owner, default), ["Later"]);
    let held = world.run(owner.api().channels(default)).unwrap().channels;
    let moved = held.iter().find(|channel| channel.id == plans).unwrap();
    assert_eq!(moved.category_id, None);
    assert!(!window.shows("category-refused"));
}

#[gpui::test]
fn the_private_switch_hides_a_channel_from_the_members_and_shows_it_again(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let (_directory, mut window, owner) = crate::admin_pages::owner_window(cx, &server);
    let default = crate::admin_pages::default_workspace(&world, &owner);
    let code = crate::admin_pages::invite(&world, &owner, Vec::new());
    let (_device, member) = crate::admin_pages::joined(&world, &server, &code, "Luca");
    let plans = world
        .run(owner.api().create_channel(
            default,
            &CreateChannel {
                name: "plans".to_owned(),
                kind: ChannelKind::Text,
                category_id: None,
                position: None,
            },
        ))
        .unwrap()
        .id;
    until_model(&world, &member, "the member has the channel", |model| {
        model.channel(plans).is_some()
    });
    let privacy = |window: &mut Window| match window.admin().map(|view| view.scope) {
        Some(Scope::Channel(settings)) => match settings.page {
            ChannelPageView::Overview(page) => Some(page.privacy),
            _ => None,
        },
        _ => None,
    };
    let everyone_denied = |world: &World| {
        let roles = world.run(owner.api().workspace_roles(default)).unwrap();
        let everyone = roles.iter().find(|role| role.is_everyone).unwrap().id;
        world
            .run(owner.api().channel_overrides(plans))
            .unwrap()
            .iter()
            .any(|held| {
                held.role_id == everyone
                    && held
                        .deny
                        .contains(&junto_client_core::protocol::WorkspacePermission::ViewChannel)
            })
    };

    window.open_administration();
    window.click("admin-nav-channels");
    let settings = selector(format!("channel-settings-{plans}"));
    window.until("the channel has its row", |window| window.shows(settings));
    window.click(settings);
    window.until("the switch knows that the channel is open", |window| {
        privacy(window) == Some(Privacy::Open)
    });

    // On: the role of everyone may not see the channel, and the member loses it.
    window.click("channel-private");
    window.until("the switch shows a private channel", |window| {
        privacy(window) == Some(Privacy::Private)
    });
    assert!(everyone_denied(&world));
    until_model(&world, &member, "the member lost the channel", |model| {
        model.channel(plans).is_none()
    });

    // Off: the member has the channel again.
    window.click("channel-private");
    window.until("the switch shows an open channel", |window| {
        privacy(window) == Some(Privacy::Open)
    });
    assert!(!everyone_denied(&world));
    until_model(
        &world,
        &member,
        "the member has the channel again",
        |model| model.channel(plans).is_some(),
    );
}
