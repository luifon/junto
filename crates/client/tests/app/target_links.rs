use gpui::TestAppContext;
use junto_client_core::address::ServerAddress;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::ChannelId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};

use crate::harness::{Window, World, member};
use crate::messages::document;

fn channel(window: &mut Window, origin: &ServerAddress, id: ChannelId) -> bool {
    window.state.update(&mut window.cx, |state, cx| {
        state.open_channel_link(origin, id, cx)
    })
}

#[gpui::test]
fn channel_links_open_active_and_archived_channels_and_conversation_links_open_directs(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = admin.text_channel(&world, &mut window, "planning");
    assert!(channel(&mut window, &origin, target));
    window.until("the linked channel is open", |window| {
        window.selected_channel() == target
    });
    window.click_channel("general");
    world
        .run(admin.handle.api().archive_channel(target))
        .unwrap();
    window.until("the target leaves the active list", |window| {
        !window.channels().iter().any(|row| row.id == target)
    });
    assert!(channel(&mut window, &origin, target));
    window.until("the linked archive is open", |window| {
        window.selected_channel() == target
            && window
                .view()
                .workspace()
                .is_some_and(|workspace| workspace.archived.is_some())
    });
    let own = window.own_account();
    let direct = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(direct.id),
            &SendMessage {
                document: document("A linked conversation"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation_link(&origin, direct.id, cx)
    }));
    window.until("the linked direct conversation is open", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.place == PlaceId::Direct(direct.id) && feed.loaded)
    });
}

#[gpui::test]
fn channel_links_refuse_hidden_and_missing_targets_identically_and_return_unknown_origins(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    let workspace = window.view().workspace().unwrap().id;
    let everyone = world
        .run(admin.handle.api().workspace_roles(workspace))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    world
        .run(admin.handle.api().set_channel_rule(
            target,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the target is hidden", |window| {
        !window.channels().iter().any(|row| row.id == target)
    });
    for id in [target, ChannelId::generate()] {
        assert!(channel(&mut window, &origin, id));
        window.until("the target is refused", |window| {
            window.shows("message-link-refusal")
        });
        assert_eq!(window.selected_channel(), selected);
        assert_eq!(
            window
                .state
                .read_with(&window.cx, |state, _| state.message_link_refusal()),
            Some(junto_client::text::MESSAGE_LINK_UNAVAILABLE)
        );
        window.click("dismiss-message-link-refusal");
    }
    let unknown = ServerAddress::parse("https://unknown.example.test").unwrap();
    assert!(!channel(&mut window, &unknown, target));
    assert_eq!(window.selected_channel(), selected);
}

#[gpui::test]
fn a_late_channel_link_does_not_take_over_navigation_or_keep_archive_readers(
    cx: &mut TestAppContext,
) {
    use crate::support::FaultProxy;
    let world = World::new();
    let server = world.server("Example Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let origin = ServerAddress::parse(&proxy.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    let workspace = window.view().workspace().unwrap().id;
    let target = admin.text_channel(&world, &mut window, "planning");
    let next = admin.text_channel(&world, &mut window, "next");
    world
        .run(admin.handle.api().archive_channel(target))
        .unwrap();
    window.until("the linked channel is archived", |window| {
        !window.channels().iter().any(|row| row.id == target)
    });
    proxy.hold_answer_ending("GET", &format!("/channels/{target}"));
    assert!(channel(&mut window, &origin, target));
    window.until("the link lookup waits for its answer", |_| {
        proxy.answers_waiting() == 1
    });
    window.click_channel("next");
    proxy.release();
    let permissions = format!("/api/v1/channels/{target}/permissions");
    window.until(
        "the stale lookup released its temporary readers",
        |window| {
            proxy
                .seen(&permissions)
                .iter()
                .any(|request| request.answer.is_some())
                && window
                    .handle()
                    .live()
                    .view()
                    .model
                    .is_some_and(|model| model.admin.archived(workspace).loaded().is_none())
        },
    );
    assert_eq!(window.selected_channel(), next);
    assert!(!window.shows("message-link-refusal"));
}

#[gpui::test]
fn a_file_link_opens_its_message_and_shows_the_file_with_copy_and_download_actions(
    cx: &mut TestAppContext,
) {
    use junto_client_core::links::{self, AppLink};
    use junto_client_core::transfers::UploadSource;
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = admin.text_channel(&world, &mut window, "planning");
    let place = PlaceId::Channel(target);
    let bytes = b"The plan for the week";
    let (file, message) = world.run(async {
        let file = admin
            .handle
            .transfers()
            .upload(
                place,
                UploadSource::Bytes {
                    name: "planning.txt".to_owned(),
                    bytes: std::sync::Arc::new(bytes.to_vec()),
                },
            )
            .finished()
            .await
            .unwrap();
        let message = admin
            .handle
            .api()
            .send_message(
                place,
                &SendMessage {
                    document: document("Linked report"),
                    reply_to: None,
                    attachments: vec![file],
                },
            )
            .await
            .unwrap();
        (file, message)
    });
    let downloads = tempfile::tempdir().unwrap();
    let directory = downloads.path().to_path_buf();
    window.state.update(&mut window.cx, |state, _| {
        state.set_download_directory(directory)
    });
    open_rendered(
        &mut window,
        AppLink::File {
            server: origin.clone(),
            id: file,
        },
    );
    window.until("the linked file is shown", |window| {
        window.shows("linked-file-dialog")
    });
    assert_eq!(window.selected_channel(), target);
    window.right_click_named("linked-file-row".to_owned());
    window.click_named("context-action:Copy file name".to_owned());
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "planning.txt"
    );
    assert!(window.shows("linked-file-dialog") && window.shows("copied-confirmation"));
    window.click("file-copy-link");
    assert_eq!(
        links::parse(&window.cx.read_from_clipboard().unwrap().text().unwrap()),
        Some(AppLink::File {
            server: origin,
            id: file
        })
    );
    window.click_named(format!("linked-download:{file}"));
    window.until("the linked file is downloaded", |_| {
        std::fs::read(downloads.path().join("planning.txt")).is_ok_and(|read| read == bytes)
    });
    window.press("escape");
    window.until("the source message is marked", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == message.id && row.target)
    });
}

#[gpui::test]
fn a_linked_picture_opens_the_viewer_and_a_file_dialog_closes_when_access_is_removed(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = admin.text_channel(&world, &mut window, "planning");
    let workspace = window.view().workspace().unwrap().id;
    let message = crate::pictures::send_files(
        &world,
        &admin,
        PlaceId::Channel(target),
        "A diagram",
        vec![("diagram.png", crate::pictures::png(32, 24))],
    );
    let file = message.attachments[0].id;
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_file_link(&origin, file, cx)
    }));
    window.until("the linked picture has its file dialog", |window| {
        window.shows("linked-file-dialog")
    });
    window.click("file-preview");
    window.until("the picture viewer is open", |window| {
        window.shows("picture-viewer")
    });
    window.press("escape");
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_file_link(&origin, file, cx)
    }));
    window.until("the file dialog is open again", |window| {
        window.shows("linked-file-dialog")
    });
    let everyone = crate::channel_use::everyone(&world, &admin, workspace);
    world
        .run(admin.handle.api().set_channel_rule(
            target,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the inaccessible file dialog is closed", |window| {
        !window.shows("linked-file-dialog") && !window.channels().iter().any(|row| row.id == target)
    });
}

#[gpui::test]
fn file_boundaries_a_replaced_target_closes_the_previous_files_context_menu(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let message = crate::pictures::send_files(
        &world,
        &admin,
        PlaceId::Channel(window.selected_channel()),
        "Two files",
        vec![
            ("first.txt", b"first".to_vec()),
            ("second.txt", b"second".to_vec()),
        ],
    );
    let first = message.attachments[0].id;
    let second = message.attachments[1].id;
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_file_link(&origin, first, cx)
    }));
    window.until("the first file is shown", |window| {
        window.shows_named(format!("linked-file-name:{first}"))
    });
    window.right_click_named("linked-file-row".to_owned());
    assert!(window.shows("row-context-menu"));
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_file_link(&origin, second, cx)
    }));
    window.until("the second file is shown", |window| {
        window.shows_named(format!("linked-file-name:{second}"))
    });
    assert!(!window.shows("row-context-menu"));
}

#[gpui::test]
fn file_boundaries_a_picture_preview_closes_when_its_account_logs_out(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let message = crate::pictures::send_files(
        &world,
        &admin,
        PlaceId::Channel(window.selected_channel()),
        "A picture",
        vec![("diagram.png", crate::pictures::png(32, 24))],
    );
    let file = message.attachments[0].id;
    let server_id = window.handle().id();
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_file_link(&origin, file, cx)
    }));
    window.until("the file dialog is shown", |window| {
        window.shows("linked-file-dialog")
    });
    window.click("file-preview");
    assert!(window.shows("picture-viewer"));
    window
        .state
        .update(&mut window.cx, |state, cx| state.log_out(server_id, cx));
    window.until("the picture viewer closes at logout", |window| {
        !window.shows("picture-viewer")
    });
}

#[gpui::test]
fn thread_links_open_the_panel_and_archived_roots_but_refuse_replies_and_missing_roots(
    cx: &mut TestAppContext,
) {
    use junto_client_core::protocol::ids::MessageId;
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = admin.text_channel(&world, &mut window, "planning");
    let root = admin
        .send(&world, target, document("Thread destination"))
        .id;
    let reply = admin.reply(&world, root, document("The linked reply"));
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_thread_link(&origin, root, cx)
    }));
    window.until("the linked thread panel is loaded", |window| {
        window.selected_channel() == target
            && window
                .view()
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| {
                    thread.root_id == root && thread.feed.as_ref().is_some_and(|feed| feed.loaded)
                })
    });
    window.click_channel("general");
    world
        .run(admin.handle.api().archive_channel(target))
        .unwrap();
    window.until("the target is archived", |window| {
        !window.channels().iter().any(|row| row.id == target)
    });
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_thread_link(&origin, root, cx)
    }));
    window.until("the archived thread panel is loaded", |window| {
        window.selected_channel() == target
            && window.view().workspace().is_some_and(|workspace| {
                workspace.archived.is_some()
                    && workspace.thread.as_ref().is_some_and(|thread| {
                        thread.root_id == root
                            && thread.feed.as_ref().is_some_and(|feed| feed.loaded)
                    })
            })
    });
    window.click_channel("general");
    let deleted = admin
        .send(
            &world,
            window.selected_channel(),
            document("Removed thread"),
        )
        .id;
    world
        .run(admin.handle.api().delete_message(deleted))
        .unwrap();
    let workspace = window.view().workspace().unwrap().id;
    let everyone = world
        .run(admin.handle.api().workspace_roles(workspace))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    world
        .run(admin.handle.api().set_channel_rule(
            target,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    for id in [root, reply, deleted, MessageId::generate()] {
        assert!(window.state.update(&mut window.cx, |state, cx| {
            state.open_thread_link(&origin, id, cx)
        }));
        window.until("the thread target is refused", |window| {
            window.shows("message-link-refusal")
        });
        assert!(!window.shows("thread-panel"));
        assert_eq!(
            window
                .state
                .read_with(&window.cx, |state, _| state.message_link_refusal()),
            Some(junto_client::text::MESSAGE_LINK_UNAVAILABLE)
        );
        window.click("dismiss-message-link-refusal");
    }
    let unknown = ServerAddress::parse("https://unknown.example.test").unwrap();
    assert!(!window.state.update(&mut window.cx, |state, cx| {
        state.open_thread_link(&unknown, root, cx)
    }));
}

#[gpui::test]
fn a_late_thread_link_does_not_replace_a_newly_opened_thread_on_the_same_channel(
    cx: &mut TestAppContext,
) {
    use crate::support::FaultProxy;
    let world = World::new();
    let server = world.server("Example Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let origin = ServerAddress::parse(&proxy.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Rin");
    let target = window.selected_channel();
    let first = admin.send(&world, target, document("First thread")).id;
    admin.reply(&world, first, document("First answer"));
    let second = admin.send(&world, target, document("Second thread")).id;
    admin.reply(&world, second, document("Second answer"));
    window.until_message("Second thread");
    proxy.hold_answer_ending("GET", &format!("/messages/{first}"));
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_thread_link(&origin, first, cx)
    }));
    window.until("the linked root lookup is held", |_| {
        proxy.answers_waiting() == 1
    });
    let server_id = window.handle().id();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_thread(server_id, target, second, cx)
    });
    window.until("the newly chosen thread is loaded", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .is_some_and(|thread| {
                thread.root_id == second && thread.feed.as_ref().is_some_and(|feed| feed.loaded)
            })
    });
    proxy.release();
    let route = format!("/api/v1/channels/{target}");
    window.until("the old link lookup completed", |_| {
        proxy
            .seen(&route)
            .iter()
            .any(|request| request.answer.is_some())
    });
    assert_eq!(
        window
            .view()
            .workspace()
            .unwrap()
            .thread
            .as_ref()
            .unwrap()
            .root_id,
        second
    );
    assert!(!window.shows("message-link-refusal"));
}

pub(crate) fn open_rendered(window: &mut Window, link: junto_client_core::links::AppLink) {
    let address = junto_client_core::links::format(&link);
    let label = format!("Open {}", uuid::Uuid::new_v4());
    window.write(&format!("[{label}]({address})"));
    let message = window.until_message(&label).message.id;
    let bounds = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{message}")))
        .unwrap();
    window.press_at_count(
        gpui::point(
            bounds.left() + gpui::px(65.0),
            bounds.bottom() - gpui::px(12.0),
        ),
        gpui::MouseButton::Right,
        1,
    );
    assert!(window.shows("link-menu"));
    window.click_named("link-open:0".to_owned());
}

#[gpui::test]
fn rendered_links_open_channels_conversations_messages_and_threads(cx: &mut TestAppContext) {
    use junto_client_core::links::AppLink;
    let world = World::new();
    let server = world.server("Example Studio");
    let origin = ServerAddress::parse(&server.address()).unwrap();
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = admin.text_channel(&world, &mut window, "planning");
    let root = admin.send(&world, target, document("The destination"));
    open_rendered(
        &mut window,
        AppLink::Channel {
            server: origin.clone(),
            id: target,
        },
    );
    window.until(
        "the rendered channel link opens its destination",
        |window| window.selected_channel() == target,
    );
    window.click_channel("general");
    open_rendered(
        &mut window,
        AppLink::Message {
            server: origin.clone(),
            id: root.id,
        },
    );
    window.until(
        "the rendered message link marks its destination",
        |window| {
            window
                .messages()
                .iter()
                .any(|row| row.message.id == root.id && row.target)
        },
    );
    window.click_channel("general");
    open_rendered(
        &mut window,
        AppLink::Thread {
            server: origin.clone(),
            id: root.id,
        },
    );
    window.until("the rendered thread link opens its panel", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .is_some_and(|thread| thread.root_id == root.id)
    });
    window.press("escape");
    window.click_channel("general");
    let direct = world
        .run(admin.handle.api().open_conversation(window.own_account()))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(direct.id),
            &SendMessage {
                document: document("Direct destination"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    open_rendered(
        &mut window,
        AppLink::Conversation {
            server: origin,
            id: direct.id,
        },
    );
    window.until(
        "the rendered conversation link opens its destination",
        |window| {
            window
                .feed()
                .is_some_and(|feed| feed.place == PlaceId::Direct(direct.id) && feed.loaded)
        },
    );
}

#[gpui::test]
fn an_unknown_application_origin_shows_the_address_field_in_collapsed_and_narrow_books(
    cx: &mut TestAppContext,
) {
    use junto_client_core::links::AppLink;
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let unknown = ServerAddress::parse("https://unknown.example.test").unwrap();
    for width in [1440.0, 720.0] {
        window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0))));
        window.state.update(&mut window.cx, |state, cx| {
            state.set_server_book_collapsed(true, cx)
        });
        window.settle();
        assert!(!window.shows("address-field"));
        let link = AppLink::Channel {
            server: unknown.clone(),
            id: ChannelId::generate(),
        };
        window.in_shell(|shell, window, cx| shell.open_app_link(link, window, cx));
        window.settle();
        assert!(window.shows("address-field"));
        window.click("address-field");
        window.press("secondary-a secondary-c");
        assert_eq!(
            window.cx.read_from_clipboard().unwrap().text().unwrap(),
            unknown.as_str()
        );
        assert_eq!(window.selected_channel(), selected);
        assert_eq!(window.server_names(), ["Example Studio"]);
    }
}
