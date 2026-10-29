use gpui::{ClipboardItem, TestAppContext, px, size};
use junto_client::shell::Pane;
use junto_client::state::admin::view::Scope;

use crate::admin_pages::{default_workspace, owner_window, text_channel};
use crate::harness::{Window, World, member};
use crate::messages::document;
use crate::threads::with_root;

#[gpui::test]
fn a_channel_menu_marks_its_channel_read_without_selecting_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    admin.send(&world, target, document("A new plan"));
    window.until("the other channel has an unread message", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.id == target && row.unread.messages == 1)
    });
    window.right_click_named(format!("channel-row:{target}"));
    window.click("channel-mark-read");
    window.until("the target channel is read", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.id == target && row.unread.messages == 0)
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("channel-menu"));
    assert_eq!(
        world
            .run(
                window
                    .handle()
                    .api()
                    .message_state(junto_client_core::model::PlaceId::Channel(target))
            )
            .unwrap()
            .unread,
        0
    );
}

#[gpui::test]
fn a_channel_menu_edits_its_target_without_selecting_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let selected = window.selected_channel();
    let workspace = default_workspace(&world, &owner);
    let target = text_channel(&world, &owner, workspace, "planning");
    window.until("the target channel is listed", |window| {
        window.shows_channel("planning")
    });
    window.right_click_named(format!("channel-row:{target}"));
    window.click("channel-edit");
    window.until("the settings of the target are open", |window| {
        matches!(window.admin().map(|view| view.scope), Some(Scope::Channel(settings)) if settings.channel_id == target)
    });
    assert_eq!(window.selected_channel(), selected);
    window.click("channel-rename");
    window.in_shell(|shell, window, cx| shell.type_admin_name("roadmap", window, cx));
    window.click("admin-form-send");
    window.until("the target has its new name", |window| {
        window.channel_names().contains(&"roadmap".to_owned()) && !window.shows("channel-name")
    });
    assert_eq!(window.selected_channel(), selected);
}

#[gpui::test]
fn a_member_without_manage_channel_has_no_edit_in_the_channel_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = window.selected_channel();
    window.right_click_named(format!("channel-row:{target}"));
    assert!(window.shows("channel-menu"));
    assert!(window.shows("channel-mark-read"));
    assert!(!window.shows("channel-edit"));
}

#[gpui::test]
fn a_direct_menu_marks_its_conversation_read_without_opening_it(cx: &mut TestAppContext) {
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::SendMessage;

    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let own = window.own_account();
    let conversation = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(conversation.id),
            &SendMessage {
                document: document("A direct update"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("the direct message is unread", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|row| row.id == conversation.id && row.unread.messages == 1)
        })
    });
    window.right_click_named(format!("direct-row:{}", conversation.id));
    window.click("direct-mark-read");
    window.until("the conversation is read", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|row| row.id == conversation.id && row.unread.messages == 0)
        })
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("direct-menu"));
    assert_eq!(
        world
            .run(
                window
                    .handle()
                    .api()
                    .message_state(PlaceId::Direct(conversation.id))
            )
            .unwrap()
            .unread,
        0
    );
}

#[gpui::test]
fn a_notice_menu_reads_one_notice_and_opens_another_notices_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    let own = window.own_account();
    let first = admin.mention(&world, target, "First notice ", own);
    let second = admin.mention(&world, target, "Second notice ", own);
    window.until("the two mentions make unread notices", |window| {
        window.unread_notices() == 2
    });
    window.open_notices();
    window.until("the notices have their message targets", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 2 && panel.rows.iter().all(|row| row.jump.is_some())
        })
    });
    let first_notice = window
        .notices()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.jump.is_some_and(|jump| jump.message_id == first.id))
        .unwrap()
        .id;
    let second_notice = window
        .notices()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.jump.is_some_and(|jump| jump.message_id == second.id))
        .unwrap()
        .id;
    window.right_click_named(format!("notice:{first_notice}"));
    window.click("notice-mark-read");
    window.until("only the chosen notice is read", |window| {
        window.notices().is_some_and(|panel| {
            panel.unread == 1
                && panel
                    .rows
                    .iter()
                    .any(|row| row.id == first_notice && row.read)
                && panel
                    .rows
                    .iter()
                    .any(|row| row.id == second_notice && !row.read)
        })
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("notice-menu"));
    window.right_click_named(format!("notice:{second_notice}"));
    window.click("notice-go-to");
    window.until("the notice opens its message and is read", |window| {
        window.selected_channel() == target
            && window.unread_notices() == 0
            && window
                .texts()
                .iter()
                .any(|text| text.starts_with("Second notice"))
    });
    assert!(!window.shows("notice-menu"));
}

#[gpui::test]
fn copy_target_links_from_channel_direct_and_notice_menus(cx: &mut TestAppContext) {
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::SendMessage;

    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let origin = window.handle().entry().address.as_str().to_owned();
    let own = window.own_account();
    let conversation = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    let message = world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(conversation.id),
            &SendMessage {
                document: document("A linked update"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("the direct conversation is listed", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.directs.iter().any(|row| row.id == conversation.id))
    });
    window.right_click_named(format!("channel-row:{selected}"));
    let action = window.cx.debug_bounds("channel-copy-link").unwrap();
    window.move_pointer(action.center());
    assert!(!window.shows("copy-text-value"));
    window.click("channel-copy-link");
    assert_target_link(&mut window, "channel", selected, &origin);
    window.right_click_named(format!("direct-row:{}", conversation.id));
    window.click("direct-copy-link");
    assert_target_link(&mut window, "conversation", conversation.id, &origin);
    assert_eq!(window.selected_channel(), selected);
    window.open_notices();
    let notice = window.until_value("the direct notice has its message target", |window| {
        window
            .notices()?
            .rows
            .into_iter()
            .find(|row| row.jump.is_some_and(|jump| jump.message_id == message.id))
            .map(|row| row.id)
    });
    window.right_click_named(format!("notice:{notice}"));
    window.click("notice-copy-link");
    assert_target_link(&mut window, "message", message.id, &origin);
    assert_eq!(window.selected_channel(), selected);
}

fn assert_target_link(
    window: &mut crate::harness::Window,
    kind: &str,
    id: impl std::fmt::Display,
    origin: &str,
) {
    assert!(window.shows("copied-confirmation"));
    let copied = window
        .cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    let parsed = url::Url::parse(&copied).unwrap();
    assert_eq!(parsed.scheme(), "junto");
    assert_eq!(parsed.host_str(), Some(kind));
    assert_eq!(
        parsed.query_pairs().into_owned().collect::<Vec<_>>(),
        [
            ("server".to_owned(), origin.to_owned()),
            ("id".to_owned(), id.to_string()),
        ]
    );
}

#[gpui::test]
fn an_administration_workspace_menu_renames_its_workspace(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let workspace = default_workspace(&world, &owner);
    window.click("server-menu-button");
    window.click("menu-administration");
    crate::admin_pages::open_page(&mut window, "admin-nav-workspaces", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Server {
                page: junto_client::state::admin::view::ServerPage::Workspaces(_),
                ..
            })
        )
    });
    window.until("the workspace row is drawn", |window| {
        window.shows_named(format!("workspace-row-{workspace}"))
    });
    window.right_click_named(format!("workspace-row-{workspace}"));
    window.click_named("context-action:Rename".to_owned());
    window.until("the workspace name form is open", |window| {
        window.shows("workspace-name")
    });
    window.in_shell(|shell, window, cx| shell.type_admin_name("Renamed workspace", window, cx));
    window.click("admin-form-send");
    window.until("the workspace has its new name", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.name == "Renamed workspace")
            && !window.shows("workspace-name")
    });
    assert!(
        world
            .run(owner.api().workspaces())
            .unwrap()
            .iter()
            .any(|row| row.id == workspace && row.name == "Renamed workspace")
    );
}

#[gpui::test]
fn a_role_context_menu_preserves_the_delete_confirmation(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let workspace = default_workspace(&world, &owner);
    let role = world
        .run(owner.api().create_workspace_role(
            workspace,
            &junto_client_core::protocol::roles::CreateWorkspaceRole {
                name: "Planning".to_owned(),
                permissions: Vec::new(),
            },
        ))
        .unwrap();
    window.click("server-menu-button");
    window.click("menu-administration");
    crate::admin_pages::open_page(&mut window, "admin-nav-roles", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Server {
                page: junto_client::state::admin::view::ServerPage::Roles(_),
                ..
            })
        )
    });
    window.until("the role row is drawn", |window| {
        window.shows_named(format!("role-row-{}", role.id))
    });
    window.right_click_named(format!("role-row-{}", role.id));
    window.click_named("context-action:Delete".to_owned());
    window.until("the deletion still requires confirmation", |window| {
        window.shows("role-confirm")
    });
    assert!(
        world
            .run(owner.api().workspace_roles(workspace))
            .unwrap()
            .iter()
            .any(|row| row.id == role.id)
    );
    window.click("role-confirm");
    window.until("the confirmed deletion removes the role", |_| {
        !world
            .run(owner.api().workspace_roles(workspace))
            .unwrap()
            .iter()
            .any(|row| row.id == role.id)
    });
}

#[gpui::test]
fn a_device_menu_renames_its_device_and_keeps_the_last_device(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    window.open_devices();
    let device = window.devices()[0].id;
    window.right_click_named(format!("device-row-{device}"));
    window.click_named("context-action:Remove".to_owned());
    assert!(!window.shows("device-remove-confirm"));
    assert_eq!(world.run(window.handle().api().devices()).unwrap().len(), 1);
    window.click_named("context-action:Rename".to_owned());
    window.in_shell(|shell, window, cx| shell.type_device_name("Desk", window, cx));
    window.click("device-form-send");
    window.until("the context action renamed its device", |window| {
        window
            .devices()
            .iter()
            .any(|row| row.id == device && row.name == "Desk")
            && !window.shows("device-name-field")
    });
    assert!(
        world
            .run(window.handle().api().devices())
            .unwrap()
            .iter()
            .any(|row| { row.id == device && row.name == "Desk" })
    );
    let (_device, _second) = crate::account::second_device(&world, &server, &window.handle());
    let other = window.until_value("the other device is listed", |window| {
        window
            .devices()
            .into_iter()
            .find(|row| row.id != device)
            .map(|row| row.id)
    });
    window.right_click_named(format!("device-row-{other}"));
    window.click_named("context-action:Remove".to_owned());
    assert!(window.shows("device-remove-confirm"));
    assert_eq!(world.run(window.handle().api().devices()).unwrap().len(), 2);
    window.click("device-remove-confirm");
    window.until("the chosen device is removed", |window| {
        window.devices().len() == 1
    });
    assert_eq!(world.run(window.handle().api().devices()).unwrap().len(), 1);
}

#[gpui::test]
fn a_readonly_glossary_row_copies_its_term_without_editing(cx: &mut TestAppContext) {
    use junto_client::state::admin::view::ServerPage;
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let workspace = window.view().workspace().unwrap().id;
    let terms = vec!["Quorvex".to_owned()];
    world
        .run(admin.handle.api().set_glossary(workspace, &terms, None))
        .unwrap();
    window.open_administration();
    crate::admin_pages::open_page(&mut window, "admin-nav-glossary", |window| {
        matches!(window.admin().map(|view| view.scope), Some(Scope::Server {
            page: ServerPage::Glossary(page), ..
        }) if !page.may_manage && page.terms.loaded().is_some_and(|terms| terms.len() == 1))
    });
    window.right_click_named("glossary-row-0".to_owned());
    window.click_named("context-action:Copy text".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Quorvex"
    );
    assert!(window.shows("copied-confirmation"));
    assert!(!window.shows("glossary-term"));
    assert_eq!(
        world
            .run(admin.handle.api().glossary(workspace))
            .unwrap()
            .terms,
        terms
    );
}

#[gpui::test]
fn an_audit_row_copies_its_displayed_entry(cx: &mut TestAppContext) {
    use junto_client::state::admin::view::ServerPage;
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    crate::admin_pages::invite(&world, &owner, Vec::new());
    window.open_administration();
    crate::admin_pages::open_page(&mut window, "admin-nav-audit", |window| {
        matches!(window.admin().map(|view| view.scope), Some(Scope::Server {
            page: ServerPage::AuditLog(page), ..
        }) if !page.rows.is_empty())
    });
    let row = match window.admin().unwrap().scope {
        Scope::Server {
            page: ServerPage::AuditLog(page),
            ..
        } => page.rows[0].clone(),
        _ => unreachable!(),
    };
    window.right_click_named(format!("audit-row-{}", row.id));
    window.click_named("context-action:Copy text".to_owned());
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.contains(&junto_client::text::admin::records::audit_head(&row)));
    assert!(copied.contains(&row.sentence));
    assert!(window.shows("copied-confirmation"));
    assert!(matches!(
        window.admin().unwrap().scope,
        Scope::Server {
            page: ServerPage::AuditLog(_),
            ..
        }
    ));
}

fn edit_menu(window: &mut Window, pane: Pane, name: &'static str) {
    let input = match pane {
        Pane::Main => "composer-input",
        Pane::Thread => "thread-composer-input",
    };
    window.type_into(input, name);
    window.press("secondary-a");
    window.right_click_named(input.to_owned());
    assert!(window.shows("composer-menu"));
    window.click("composer-copy");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        name
    );
    window.press("shift-f10");
    window.click("composer-cut");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(pane, cx)),
        ""
    );
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("β pasted".to_owned()));
    window.right_click_named(input.to_owned());
    window.click("composer-paste");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(pane, cx)),
        "β pasted"
    );
    window.press("shift-f10");
    window.click("composer-select-all");
    window.press("secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "β pasted"
    );
    window.press("shift-f10 escape");
    assert!(!window.shows("composer-menu"));
}

#[gpui::test]
fn the_main_composer_has_one_edit_menu_for_mouse_and_keyboard(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    edit_menu(&mut window, Pane::Main, "café draft");
}

#[gpui::test]
fn the_thread_composer_uses_the_same_edit_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, _admin, _directory, mut window, _channel, root) = with_root(cx, &world);
    window.open_thread_of(root);
    edit_menu(&mut window, Pane::Thread, "thread draft");
}

#[gpui::test]
fn a_message_menu_stays_inside_a_small_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.simulate(|cx| cx.simulate_resize(size(px(720.0), px(480.0))));
    for number in 0..12 {
        window.write(&format!("message {number}"));
    }
    let message = window.until_message("message 11").message.id;
    window.right_click_named(format!("message:{message}"));
    let bounds = window.cx.debug_bounds("message-menu").unwrap();
    assert!(
        bounds.left() >= px(8.0) && bounds.top() >= px(8.0),
        "{bounds:?}"
    );
    assert!(
        bounds.right() <= px(712.0) && bounds.bottom() <= px(472.0),
        "{bounds:?}"
    );
    window.press("escape");
    assert!(!window.shows("message-menu"));
}

#[gpui::test]
fn a_composer_menu_ends_when_navigation_changes_its_place(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let other = admin.text_channel(&world, &mut window, "other");
    let workspace = window.view().workspace().unwrap().id;
    window.type_into("composer-input", "keep this draft");
    window.press("secondary-a shift-f10");
    assert!(window.shows("composer-menu"));
    window.state.update(&mut window.cx, |state, cx| {
        state.select_channel(workspace, other, cx)
    });
    window.until("navigation closes the menu", |window| {
        !window.shows("composer-menu")
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
    window.click_channel("general");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "keep this draft"
    );
}

#[gpui::test]
fn a_rendered_link_has_its_own_menu_and_shared_copy_confirmation(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    window.write("[the guide](https://example.test/guide)");
    let message = window.until_message("the guide");
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!(
            "message:{}",
            message.message.id
        )))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.bottom() - px(12.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("link-menu"));
    assert!(!window.shows("message-menu"));
    assert_eq!(window.cx.opened_url(), None);
    window.click_named("link-copy:0".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "https://example.test/guide"
    );
    assert!(window.shows("copied-confirmation"));
}

#[gpui::test]
fn an_application_link_can_be_sent_and_copied_from_its_rendered_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let address = junto_client_core::links::format(&junto_client_core::links::AppLink::Channel {
        server: junto_client_core::address::ServerAddress::parse(&server.address()).unwrap(),
        id: window.selected_channel(),
    });
    window.write(&format!("[the guide]({address})"));
    let message = window.until_message("the guide");
    assert!(matches!(
        &message.message.document.as_ref().unwrap().blocks[0],
        junto_client_core::protocol::message::Block::Paragraph { content }
            if matches!(&content[0], junto_client_core::protocol::message::Inline::Link { url, .. } if url == &address)
    ));
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!(
            "message:{}",
            message.message.id
        )))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.bottom() - px(12.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("link-menu"));
    window.click_named("link-copy:0".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        address
    );
    assert!(window.shows("copied-confirmation"));
}

#[gpui::test]
fn a_member_row_copies_identity_and_opens_its_existing_direct_action(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let account = admin.handle.account().unwrap();
    window.click("members-button");
    window.until("the member is visible", |window| window.shows_member("Ada"));
    window.right_click_named(format!("member-row:{account}"));
    assert!(window.shows("row-context-menu"));
    window.click_named("context-action:Copy account ID".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        account.to_string()
    );
    assert!(window.shows("copied-confirmation"));
    window.right_click_named(format!("member-row:{account}"));
    window.click_named("context-action:Write a direct message to Ada".to_owned());
    window.until("the selected member's conversation is shown", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .conversation
                .as_ref()
                .is_some_and(|conversation| conversation.account_id == account)
        })
    });
}

#[gpui::test]
fn message_author_names_and_avatars_have_account_actions_instead_of_message_actions(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let account = admin.handle.account().unwrap();
    admin.send(
        &world,
        window.selected_channel(),
        document("An author's plan"),
    );
    let message = window.until_message("An author's plan").message.id;
    window.until("the author's name is read", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == message && row.author == "Ada")
    });
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{message}")))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(10.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    assert!(!window.shows("message-menu"));
    window.click_named(format!("account-copy-id:{account}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        account.to_string()
    );
    window.press_at_count(
        gpui::point(bounds.left() + px(20.0), bounds.top() + px(22.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    window.click_named(format!("account-copy-name:{account}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Ada"
    );
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(10.0)),
        gpui::MouseButton::Right,
        1,
    );
    window.click_named(format!("account-message:{account}"));
    window.until("the author's conversation is shown", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .conversation
                .as_ref()
                .is_some_and(|conversation| conversation.account_id == account)
        })
    });
    window.click_channel("general");
    window.until_message("An author's plan");
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{message}")))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(10.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    let server_id = window.handle().id();
    window
        .state
        .update(&mut window.cx, |state, cx| state.log_out(server_id, cx));
    window.until("the account menu closes at logout", |window| {
        !window.shows("account-menu")
    });
}

#[gpui::test]
fn an_account_mention_has_its_own_menu_beside_links_and_the_message(cx: &mut TestAppContext) {
    use junto_client_core::protocol::message::{Block, Document, Inline};
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let own = window.own_account();
    let author = admin.handle.account().unwrap();
    admin.send(
        &world,
        window.selected_channel(),
        Document {
            blocks: vec![Block::Paragraph {
                content: vec![
                    Inline::Account { account_id: own },
                    Inline::Text {
                        text: " needs the ".to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Link {
                        text: "guide".to_owned(),
                        url: "https://example.test/guide".to_owned(),
                    },
                    Inline::Text {
                        text: " from ".to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Account { account_id: author },
                ],
            }],
        },
    );
    let message = window
        .until_message("@Rin needs the guide from @Ada")
        .message
        .id;
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{message}")))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.bottom() - px(12.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    assert!(!window.shows("message-menu"));
    assert!(!window.shows("link-menu"));
    assert!(!window.shows_named(format!("account-message:{own}")));
    window.click_named(format!("account-copy-id:{own}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        own.to_string()
    );
    window.click("composer-input");
    let mut found = false;
    for _ in 0..48 {
        window.press("tab shift-f10");
        found = window.shows("account-menu")
            && window.shows_named(format!("account-copy-id:{own}"))
            && window.shows_named(format!("account-copy-id:{author}"));
        if found {
            break;
        }
        window.press("escape");
    }
    assert!(
        found,
        "the mention keyboard group did not offer both accounts"
    );
    window.click_named(format!("account-copy-id:{author}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        author.to_string()
    );
}

#[gpui::test]
fn an_account_menu_closes_when_its_source_message_is_deleted_or_hidden(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    admin.send(
        &world,
        window.selected_channel(),
        document("A removable plan"),
    );
    let message = window.until_message("A removable plan").message.id;
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{message}")))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(10.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    world
        .run(admin.handle.api().delete_message(message))
        .unwrap();
    window.until("the source message is removed", |window| {
        window
            .messages()
            .iter()
            .all(|row| row.message.id != message || row.message.deleted)
    });
    assert!(!window.shows("account-menu"));
    let channel = window.selected_channel();
    let workspace = window.view().workspace().unwrap().id;
    admin.send(&world, channel, document("A hidden plan"));
    let message = window.until_message("A hidden plan").message.id;
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{message}")))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(10.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    let everyone = crate::channel_use::everyone(&world, &admin, workspace);
    world
        .run(admin.handle.api().set_channel_rule(
            channel,
            junto_client_core::protocol::structure::RuleSubject::Role { role_id: everyone },
            &junto_client_core::protocol::structure::SetChannelOverride {
                allow: Vec::new(),
                deny: vec![junto_client_core::protocol::WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the source channel is hidden", |window| {
        !window.channels().iter().any(|row| row.id == channel)
    });
    assert!(!window.shows("account-menu"));
}

#[gpui::test]
fn a_pending_mentions_account_menu_closes_when_the_message_is_discarded(cx: &mut TestAppContext) {
    use crate::support::{Fault, FaultProxy};
    use junto_client::state::feed::FeedRow;
    let world = World::new();
    let server = world.server("Example Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    window.until("the channel feed is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let server_id = window.handle().id();
    let channel = window.selected_channel();
    let place = junto_client_core::model::PlaceId::Channel(channel);
    let own = window.own_account();
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    proxy.fail(
        &format!("/api/v1/channels/{channel}/messages"),
        vec![Fault::Busy; attempts],
    );
    let document = crate::notices::mention("", own);
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.send_message(server_id, place, document, None, cx)
    }));
    window.until("the pending mention offers Discard", |window| {
        window.shows("discard-message")
    });
    let id = window
        .feed()
        .unwrap()
        .rows
        .iter()
        .find_map(|row| match row {
            FeedRow::Waiting(row) => Some(row.pending.id),
            _ => None,
        })
        .unwrap();
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("waiting:{id}")))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(33.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    assert!(window.shows_named(format!("account-copy-id:{own}")));
    window.state.update(&mut window.cx, |state, cx| {
        state.discard_message(server_id, id, cx)
    });
    window.until("the pending message is discarded", |window| {
        window.feed().is_some_and(|feed| {
            feed.rows
                .iter()
                .all(|row| !matches!(row, FeedRow::Waiting(_)))
        })
    });
    assert!(!window.shows("account-menu"));
}

#[gpui::test]
fn a_search_result_menu_copies_its_text_and_link_and_opens_its_target(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    let message = admin.send(&world, target, document("A quartz plan"));
    window.search_for("quartz");
    let panel = window.until_searched();
    assert_eq!(panel.messages().len(), 1);
    window.right_click_named(format!("search-result:{}", message.id));
    assert!(window.shows("row-context-menu"));
    window.click_named("context-action:Copy text".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "A quartz plan"
    );
    assert_eq!(window.selected_channel(), selected);
    window.right_click_named(format!("search-result:{}", message.id));
    window.click_named("context-action:Copy link".to_owned());
    let link = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(
        matches!(junto_client_core::links::parse(&link), Some(junto_client_core::links::AppLink::Message { id, .. }) if id == message.id)
    );
    window.right_click_named(format!("search-result:{}", message.id));
    window.click_named("context-action:Go to message".to_owned());
    window.until("the search target is opened", |window| {
        window.selected_channel() == target
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == message.id && row.target)
    });
}

#[gpui::test]
fn search_and_notice_author_targets_have_account_menus_that_end_with_their_panel(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let author = admin.handle.account().unwrap();
    let own = window.own_account();
    let message = admin.send(
        &world,
        window.selected_channel(),
        crate::notices::mention("quartz for ", own),
    );
    window.search_for("quartz");
    window.until_searched();
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!(
            "search-result:{}",
            message.id
        )))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(38.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    assert!(!window.shows("row-context-menu"));
    window.click_named(format!("account-copy-id:{author}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        author.to_string()
    );
    window.press_at_count(
        gpui::point(bounds.left() + px(65.0), bounds.top() + px(38.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    window.in_shell(|shell, window, cx| shell.close_search(window, cx));
    window.until("closing search ends the author menu", |window| {
        !window.shows("account-menu")
    });
    window.click("notices-row");
    window.until("the mention is in the notice list", |window| {
        window
            .view()
            .lists()
            .and_then(|lists| lists.notices.as_ref())
            .is_some_and(|panel| panel.rows.iter().any(|row| row.author_id == Some(author)))
    });
    let notice = window
        .view()
        .lists()
        .unwrap()
        .notices
        .as_ref()
        .unwrap()
        .rows
        .iter()
        .find(|row| row.author_id == Some(author))
        .unwrap()
        .id;
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("notice:{notice}")))
        .unwrap();
    window.press_at_count(
        gpui::point(bounds.left() + px(25.0), bounds.top() + px(45.0)),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("account-menu"));
    assert!(!window.shows("notice-menu"));
    window.click_named(format!("account-copy-name:{author}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Ada"
    );
    window.press_at_count(
        gpui::point(bounds.left() + px(25.0), bounds.top() + px(45.0)),
        gpui::MouseButton::Right,
        1,
    );
    window.in_shell(|shell, _, cx| shell.toggle_notices(cx));
    window.until("closing notices ends the author menu", |window| {
        !window.shows("account-menu")
    });
}

#[gpui::test]
fn thread_menu_actions_read_and_follow_their_target_without_opening_its_panel(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, admin, _directory, mut window, channel, root) = with_root(cx, &world);
    let own = window.own_account();
    admin.reply(&world, root, crate::notices::mention("A plan for ", own));
    window.until("the closed thread is unread", |window| {
        window
            .channels()
            .iter()
            .flat_map(|channel| &channel.threads)
            .any(|thread| thread.root_id == root && thread.unread > 0)
    });
    let owner = window.handle().owner().unwrap();
    let mut another_entry = owner;
    another_entry.entry.generation ^= 1;
    assert!(
        !window
            .state
            .update(&mut window.cx, |state, cx| state.begin_thread_menu(
                another_entry,
                root,
                cx
            ))
    );
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.begin_thread_menu(owner, root, cx)
    }));
    window.until("the target follow state is read", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.thread_menu_control(owner, root)
            })
            .is_some_and(|control| control.following)
    });
    assert!(!window.shows("thread-panel"));
    window.state.update(&mut window.cx, |state, cx| {
        state.end_thread_menu(another_entry, root, cx)
    });
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state
                .thread_menu_control(owner, root))
            .is_some()
    );
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.mark_thread_menu_read(owner, root, cx)
    }));
    window.until("the target is marked read", |window| {
        window
            .channels()
            .iter()
            .flat_map(|channel| &channel.threads)
            .all(|thread| thread.root_id != root || thread.unread == 0)
            && window
                .state
                .read_with(&window.cx, |state, _| {
                    state.thread_menu_control(owner, root)
                })
                .is_some_and(|control| !control.busy)
    });
    assert_eq!(
        world
            .run(window.handle().api().thread_state(root))
            .unwrap()
            .unread,
        0
    );
    for following in [false, true] {
        assert!(window.state.update(&mut window.cx, |state, cx| {
            state.set_thread_menu_follow(owner, root, following, cx)
        }));
        window.until("the target follow choice is stored", |window| {
            window
                .state
                .read_with(&window.cx, |state, _| {
                    state.thread_menu_control(owner, root)
                })
                .is_some_and(|control| !control.busy && control.following == following)
        });
        assert_eq!(
            world
                .run(window.handle().api().thread_state(root))
                .unwrap()
                .follow_setting,
            Some(following)
        );
    }
    assert_eq!(window.selected_channel(), channel);
    assert!(!window.shows("thread-panel"));
    window.state.update(&mut window.cx, |state, cx| {
        state.end_thread_menu(owner, root, cx)
    });
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state
                .thread_menu_control(owner, root))
            .is_none()
    );
    assert!(!window.state.update(&mut window.cx, |state, cx| {
        state.set_thread_menu_follow(owner, root, false, cx)
    }));
}

#[gpui::test]
fn a_thread_row_menu_reads_and_changes_follow_without_selecting_its_channel(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    let own = window.own_account();
    let root = admin.send(&world, target, document("A thread plan")).id;
    admin.reply(&world, root, crate::notices::mention("For ", own));
    let owner = window.handle().owner().unwrap();
    let ready = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.thread_menu_control(owner, root)
            })
            .is_some_and(|control| !control.busy)
    };
    window.until("the other channel lists its unread thread", |window| {
        window.shows_named(format!("thread-row:{root}"))
    });
    window.right_click_named(format!("thread-row:{root}"));
    assert!(window.shows("thread-menu"));
    assert!(!window.shows("channel-menu"));
    assert!(!window.shows("thread-panel"));
    window.click("thread-copy-link");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert_eq!(
        junto_client_core::links::parse(&copied),
        Some(junto_client_core::links::AppLink::Thread {
            server: junto_client_core::address::ServerAddress::parse(&server.address()).unwrap(),
            id: root
        })
    );
    assert!(window.shows("copied-confirmation"));
    assert_eq!(window.selected_channel(), selected);
    window.right_click_named(format!("thread-row:{root}"));
    window.until("the thread menu state is ready", ready);
    window.click("thread-read");
    window.until("the thread was read and the menu closed", |window| {
        !window.shows("thread-menu")
            && world
                .run(window.handle().api().thread_state(root))
                .unwrap()
                .unread
                == 0
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("thread-panel"));
    admin.reply(&world, root, document("Another plan"));
    window.until("the new reply lists the followed thread", |window| {
        window.shows_named(format!("thread-row:{root}"))
    });
    window.right_click_named(format!("thread-row:{root}"));
    window.until("the follow action is ready", ready);
    window.click("thread-follow");
    window.until("stop following was stored", |window| {
        !window.shows("thread-menu")
            && !world
                .run(window.handle().api().thread_state(root))
                .unwrap()
                .following
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("thread-panel"));
    admin.reply(
        &world,
        root,
        crate::notices::mention("A further plan for ", own),
    );
    window.until("the mention lists the unfollowed thread", |window| {
        window.shows_named(format!("thread-row:{root}"))
    });
    window.right_click_named(format!("thread-row:{root}"));
    window.until("follow is ready", ready);
    window.click("thread-follow");
    window.until("following was stored", |window| {
        !window.shows("thread-menu")
            && world
                .run(window.handle().api().thread_state(root))
                .unwrap()
                .following
    });
    assert_eq!(window.selected_channel(), selected);
    window.right_click_named(format!("thread-row:{root}"));
    window.click("thread-open");
    window.until("the chosen thread is opened", |window| {
        window.selected_channel() == target
            && window
                .view()
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| thread.root_id == root)
    });
}

#[gpui::test]
fn a_thread_menu_keeps_failures_visible_and_retries_reads_and_changes(cx: &mut TestAppContext) {
    use crate::support::{Fault, FaultProxy};
    let world = World::new();
    let server = world.server("Example Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    let root = admin.send(&world, target, document("Thread recovery")).id;
    admin.reply(
        &world,
        root,
        crate::notices::mention("For ", window.own_account()),
    );
    window.until("the thread row is shown", |window| {
        window.shows_named(format!("thread-row:{root}"))
    });
    let read = format!("/api/v1/messages/{root}/thread/read");
    proxy.fail(&read, std::iter::repeat_n(Fault::Refused, 64));
    window.right_click_named(format!("thread-row:{root}"));
    window.until("the menu states the failed read", |window| {
        window.shows("thread-menu-failed")
    });
    assert!(window.shows("thread-menu-retry"));
    assert_eq!(window.selected_channel(), selected);
    proxy.clear_faults(&read);
    window.click("thread-menu-retry");
    let owner = window.handle().owner().unwrap();
    window.until("the menu retry loaded its state", |window| {
        !window.shows("thread-menu-failed")
            && window
                .state
                .read_with(&window.cx, |state, _| {
                    state.thread_menu_control(owner, root)
                })
                .is_some_and(|control| !control.busy)
    });
    let follow = format!("/api/v1/messages/{root}/thread/follow");
    proxy.fail(&follow, [Fault::Refused]);
    window.click("thread-follow");
    window.until("the menu states the refused change", |window| {
        window.shows("thread-menu-refused")
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(
        world
            .run(window.handle().api().thread_state(root))
            .unwrap()
            .following
    );
    window.click("thread-follow");
    window.until(
        "the repeated change succeeds and closes the menu",
        |window| {
            !window.shows("thread-menu")
                && !world
                    .run(window.handle().api().thread_state(root))
                    .unwrap()
                    .following
        },
    );
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("thread-panel"));
}

#[gpui::test]
fn a_reply_preview_copies_its_target_and_gives_its_author_a_separate_menu(cx: &mut TestAppContext) {
    use junto_client_core::{model::PlaceId, protocol::message::SendMessage};
    let world = World::new();
    let (_server, admin, _directory, mut window, channel, root) = with_root(cx, &world);
    let answer = world
        .run(admin.handle.api().send_message(
            PlaceId::Channel(channel),
            &SendMessage {
                document: document("A reply preview"),
                reply_to: Some(root),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("A reply preview");
    window.until("the preview is available", |window| {
        window.messages().iter().any(|row| {
            row.message.id == answer.id
                && row
                    .reply
                    .as_ref()
                    .is_some_and(|quote| quote.author_id.is_some())
        })
    });
    window.right_click_named(format!("reply-preview:{}", answer.id));
    window.click("context-action:Copy text");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Tracks or mixdown?"
    );
    window.right_click_named(format!("reply-author:{}", answer.id));
    let ada = admin.handle.account().unwrap();
    window.click_named(format!("account-copy-id:{ada}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        ada.to_string()
    );
    assert!(!window.shows("message-menu"));
    window.right_click_named(format!("reply-preview:{}", answer.id));
    window.click("context-action:Copy link");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.contains(&root.to_string()) && !copied.contains(&answer.id.to_string()));
    window.right_click_named(format!("reply-preview:{}", answer.id));
    window.click("context-action:Go to message");
    window.until("the quoted message is marked", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.target.is_some_and(|(id, _)| id == root))
    });
    window.right_click_named(format!("reply-author:{}", answer.id));
    world
        .run(admin.handle.api().delete_message(answer.id))
        .unwrap();
    window.until("the removed preview closes its author menu", |window| {
        !window.shows("account-menu")
    });
}

#[gpui::test]
fn thread_preview_people_and_the_thread_have_separate_context_targets(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _, root) = with_root(cx, &world);
    let ada = admin.handle.account().unwrap();
    admin.reply(&world, root, document("A thread preview"));
    window.until("the thread picture is listed", |window| {
        window.messages().iter().any(|row| {
            row.message.id == root
                && row
                    .thread
                    .as_ref()
                    .is_some_and(|mark| mark.authors.contains(&ada))
        })
    });
    window.right_click_named(format!("thread-person:{root}:{ada}"));
    window.click_named(format!("account-copy-id:{ada}"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        ada.to_string()
    );
    assert!(!window.shows("message-menu"));
    window.right_click_named(format!("thread-chip:{root}"));
    window.click("context-action:Copy link");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.starts_with("junto://thread?") && copied.contains(&root.to_string()));
    window.right_click_named(format!("thread-chip:{root}"));
    window.click("context-action:Open the thread");
    window.until("the thread is open", |window| window.shows("thread-panel"));
    window.click("close-thread");
    window.right_click_named(format!("thread-person:{root}:{ada}"));
    world.run(admin.handle.api().delete_message(root)).unwrap();
    window.until("the removed thread closes its person menu", |window| {
        !window.shows("account-menu")
    });
}
