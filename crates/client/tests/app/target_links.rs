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
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_file_link(&origin, file, cx)
    }));
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
