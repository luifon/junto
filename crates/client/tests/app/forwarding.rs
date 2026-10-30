use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::api::PageQuery;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};

use crate::harness::{Window, World, member};
use crate::messages::document;

fn drawn(window: &mut Window) -> String {
    window.settle();
    window
        .in_shell(|shell, _, _| shell.painted_readonly_text())
        .join("\n")
}

#[gpui::test]
fn forwarding_copies_the_body_and_rechecks_only_origin_access(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let source_channel = window.selected_channel();
    let source = admin.send(&world, source_channel, document("the private plan"));
    let target = admin.text_channel(&world, &mut window, "delivery");
    let server_id = window.view().selected.unwrap().id;
    window.until_message("the private plan");
    window.right_click_named(format!("message:{}", source.id));
    window.click("message-forward");
    window.click_named(format!("forward-destination:{server_id}:channel:{target}"));
    window.click("forward-send");
    window.until(
        "the forwarded message and its origin are visible",
        |window| {
            window.selected_channel() == target
                && !window.shows("forward-dialog")
                && window.shows_named(format!("forward-message:{}", source.id))
        },
    );
    assert!(drawn(&mut window).contains("the private plan"));
    window.until("the copied body is stored in the destination", |window| {
        window.messages().iter().any(|row| {
            serde_json::to_string(&row.message.document)
                .unwrap()
                .contains("the private plan")
        })
    });

    window.click_named(format!("forward-origin:{}", source.id));
    window.until("the origin opens at its message", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == source.id && row.target)
    });
    world
        .run(
            admin
                .handle
                .api()
                .edit_message(source.id, document("the revised private plan")),
        )
        .unwrap();
    window.until_message("the revised private plan");
    window.click_channel("delivery");
    window.until(
        "the stored copy is shown after the original edit",
        |window| window.selected_channel() == target && drawn(window).contains("the private plan"),
    );
    assert!(!drawn(&mut window).contains("the revised private plan"));
    let api = admin.handle.api();
    let workspace = window.view().workspace().unwrap().id;
    let everyone = world
        .run(api.workspace_roles(workspace))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id;
    world
        .run(api.set_channel_rule(
            source_channel,
            RuleSubject::Role { role_id: everyone },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the forward hides an inaccessible origin", |window| {
        window.shows_named(format!("forward-unavailable:{}", source.id))
    });
    assert!(drawn(&mut window).contains("the private plan"));
    assert!(!window.shows_named(format!("forward-origin:{}", source.id)));
}

#[gpui::test]
fn forwarding_to_a_conversation_keeps_its_existing_draft(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let direct = window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    window.type_into("composer-input", "unfinished draft");
    window.click_channel("general");
    let source = admin.send(&world, window.selected_channel(), document("shared work"));
    window.until_message("shared work");
    let PlaceId::Direct(conversation) = direct else {
        panic!("not a direct conversation")
    };
    let server_id = window.view().selected.unwrap().id;
    window.right_click_named(format!("message:{}", source.id));
    window.click("message-forward");
    window.click_named(format!(
        "forward-destination:{server_id}:direct:{conversation}"
    ));
    window.click("forward-send");
    window.until("the forward is shown in the conversation", |window| {
        window.feed().is_some_and(|feed| feed.place == direct)
            && !window.shows("forward-dialog")
            && window.shows_named(format!("forward-message:{}", source.id))
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "unfinished draft"
    );
    assert!(drawn(&mut window).contains("shared work"));
}

#[gpui::test]
fn a_forward_between_servers_copies_body_for_a_reader_without_source_access(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let source_server = world.server("Source Studio");
    let source_admin = world.owner(&source_server);
    let target_server = world.server("Delivery Studio");
    let target_admin = world.owner(&target_server);
    let (_directory, mut window) =
        member(cx, &world, &source_server.address(), &source_admin, "Luca");
    let source_id = window.view().selected.unwrap().id;
    let mut source_body = document("a protected source for ");
    if let junto_client_core::protocol::message::Block::Paragraph { content } =
        &mut source_body.blocks[0]
    {
        content.push(junto_client_core::protocol::message::Inline::Account {
            account_id: source_admin.handle.account().unwrap(),
        });
    }
    let content = std::sync::Arc::new(vec![b'x'; 2 * 1024 * 1024 + 17]);
    let source = world.run(async {
        let attachment = source_admin
            .handle
            .transfers()
            .upload(
                PlaceId::Channel(window.selected_channel()),
                junto_client_core::transfers::UploadSource::Bytes {
                    name: "shared-plan.txt".to_owned(),
                    bytes: content.clone(),
                },
            )
            .finished()
            .await
            .unwrap();
        source_admin
            .handle
            .api()
            .send_message(
                PlaceId::Channel(window.selected_channel()),
                &junto_client_core::protocol::message::SendMessage {
                    document: source_body,
                    reply_to: None,
                    attachments: vec![attachment],
                },
            )
            .await
            .unwrap()
    });
    window.until("the protected source is visible", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == source.id)
    });
    let invite = target_admin.invite(&world, 1);
    window.find_server(&target_server.address());
    window.join_with_code(invite.expose());
    let target_id = target_admin.handle.id();
    window.until("the target server lists its channels", |window| {
        window
            .view()
            .selected
            .is_some_and(|server| server.id == target_id)
            && window.shows_channel("general")
    });
    window.click_channel("general");
    let channel = window.selected_channel();
    window.state.update(&mut window.cx, |state, cx| {
        state.select_server(source_id, cx)
    });
    window.until("the source server lists its channels", |window| {
        window
            .view()
            .selected
            .is_some_and(|server| server.id == source_id)
            && window.shows_channel("general")
    });
    window.click_channel("general");
    window.until("the protected source is visible", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == source.id)
    });
    window.right_click_named(format!("message:{}", source.id));
    window.click("message-forward");
    window.click_named(format!("forward-destination:{target_id}:channel:{channel}"));
    window.click("forward-send");
    window.until("the cross-server forward is shown", |window| {
        window
            .view()
            .selected
            .is_some_and(|server| server.id == target_id)
            && !window.shows("forward-dialog")
            && window.shows_named(format!("forward-message:{}", source.id))
    });
    assert!(drawn(&mut window).contains("a protected source"));
    let shown = drawn(&mut window);
    assert!(shown.contains("Not verified"), "{shown}");
    assert!(!shown.contains("Forwarded message · Ada"), "{shown}");
    let history = world
        .run(
            target_admin
                .handle
                .api()
                .messages(PlaceId::Channel(channel), &PageQuery::default()),
        )
        .unwrap();
    let stored = serde_json::to_string(&history.messages).unwrap();
    assert!(stored.contains("forward"));
    assert!(stored.contains("a protected source"));
    assert!(!stored.contains(&source_admin.handle.account().unwrap().to_string()));
    let (_reader_directory, mut reader) =
        member(cx, &world, &target_server.address(), &target_admin, "Mara");
    reader.until(
        "a reader without the source server sees an unavailable origin",
        |window| window.shows_named(format!("forward-unavailable:{}", source.id)),
    );
    assert!(drawn(&mut reader).contains("a protected source"));
    assert!(!reader.shows_named(format!("forward-origin:{}", source.id)));
    let forwarded = history
        .messages
        .iter()
        .find(|message| {
            message.document.as_ref().is_some_and(|document| {
                document.blocks.iter().any(|block| {
                    matches!(
                        block,
                        junto_client_core::protocol::message::Block::Forward { .. }
                    )
                })
            })
        })
        .unwrap();
    reader.right_click_named(format!("message:{}", forwarded.id));
    reader.click("message-copy-text");
    let copied_text = reader.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied_text.contains("a protected source"));
    assert!(!copied_text.contains(&source_server.address()));
    assert_eq!(forwarded.attachments.len(), 1);
    assert_ne!(forwarded.attachments[0].id, source.attachments[0].id);
    let downloads = tempfile::tempdir().unwrap();
    let folder = downloads.path().to_path_buf();
    reader.state.update(&mut reader.cx, |state, _| {
        state.set_download_directory(folder)
    });
    reader.click_named(format!("download:{}", forwarded.attachments[0].id));
    reader.until(
        "the copied cross-server file downloads without a source account",
        |window| {
            window
                .view()
                .downloads
                .get(&(target_id, forwarded.attachments[0].id))
                .is_some_and(|download| {
                    matches!(
                        download.stage,
                        junto_client_core::transfers::DownloadStage::Saved(_)
                    )
                })
        },
    );
    assert_eq!(
        std::fs::read(downloads.path().join("shared-plan.txt")).unwrap(),
        *content
    );
    let direct = reader.open_conversation_with(target_admin.handle.account().unwrap(), "Ada");
    reader.type_into("composer-input", "keep this draft");
    reader.click_channel("general");
    reader.until("the copy is visible without source access", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == forwarded.id)
    });
    reader.right_click_named(format!("message:{}", forwarded.id));
    reader.click("message-forward");
    let PlaceId::Direct(conversation) = direct else {
        panic!("not a direct conversation")
    };
    reader.click_named(format!(
        "forward-destination:{target_id}:direct:{conversation}"
    ));
    reader.click("forward-send");
    reader.until(
        "the reader can pass on the copy without reading its earlier origin",
        |window| {
            window.feed().is_some_and(|feed| feed.place == direct)
                && drawn(window).contains("a protected source")
                && !window.shows("forward-dialog")
        },
    );
    assert_eq!(
        reader.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "keep this draft"
    );
}

#[gpui::test]
fn a_thread_link_can_choose_a_destination_outside_the_thread_list(cx: &mut TestAppContext) {
    use junto_client::state::feed::FeedRow;
    use junto_client_core::protocol::message::Block;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let channel = window.selected_channel();
    let source = admin.send(&world, channel, document("send this into the discussion"));
    let root = admin.send(&world, channel, document("the discussion"));
    window.until_message("the discussion");
    let server_id = window.view().selected.unwrap().id;
    let link = window
        .state
        .read_with(&window.cx, |state, _| {
            state.message_link(server_id, root.id)
        })
        .unwrap();
    window.right_click_named(format!("message:{}", source.id));
    window.click("message-forward");
    window.type_into("forward-find", &link);
    window.click("forward-thread-link");
    window.until("the thread is selected", |window| {
        window.shows_named(format!(
            "forward-destination:{server_id}:thread:{}",
            root.id
        ))
    });
    window.click("forward-send");
    window.until("the server accepts the forward in the chosen thread", |window| {
        window.view().workspace().and_then(|workspace| workspace.thread.as_ref())
            .filter(|thread| thread.root_id == root.id).and_then(|thread| thread.feed.as_ref())
            .is_some_and(|feed| feed.rows.iter().any(|row| matches!(row, FeedRow::Message(row) if row.message.document.as_ref().is_some_and(|document| document.blocks.iter().any(|block| matches!(block, Block::Forward { message_id, .. } if *message_id == source.id))))))
            && window.shows_named(format!("forward-message:{}", source.id))
    });
}

#[gpui::test]
fn a_forward_keeps_code_whitespace_and_downloads_the_sources_file(cx: &mut TestAppContext) {
    use junto_client_core::protocol::message::{Block, Document, SendMessage};
    use junto_client_core::transfers::{DownloadStage, UploadSource};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let place = PlaceId::Channel(window.selected_channel());
    let code = "fn run() {\n    work();\n}";
    let (source, attachment) = world.run(async {
        let attachment = admin
            .handle
            .transfers()
            .upload(
                place,
                UploadSource::Bytes {
                    name: "plan.txt".to_owned(),
                    bytes: std::sync::Arc::new(b"protected attachment".to_vec()),
                },
            )
            .finished()
            .await
            .unwrap();
        let source = admin
            .handle
            .api()
            .send_message(
                place,
                &SendMessage {
                    document: Document {
                        blocks: vec![Block::Code {
                            text: code.to_owned(),
                            language: Some("rust".to_owned()),
                        }],
                    },
                    reply_to: None,
                    attachments: vec![attachment],
                },
            )
            .await
            .unwrap();
        (source, attachment)
    });
    let target = admin.text_channel(&world, &mut window, "delivery");
    let server_id = window.view().selected.unwrap().id;
    window.until("the source file is shown", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == source.id)
    });
    window.right_click_named(format!("message:{}", source.id));
    window.click("message-forward");
    window.click_named(format!("forward-destination:{server_id}:channel:{target}"));
    window.click("forward-send");
    window.until("the forwarded message is stored", |window| window.messages().iter().any(|row| row.message.document.as_ref().is_some_and(|document| document.blocks.iter().any(|block| matches!(block, Block::Forward { message_id, .. } if *message_id == source.id)))));
    let copied = window.messages().into_iter().find(|row| row.message.document.as_ref().is_some_and(|document| document.blocks.iter().any(|block| matches!(block, Block::Forward { message_id, .. } if *message_id == source.id)))).unwrap();
    assert_eq!(copied.message.attachments.len(), 1);
    let copied_attachment = copied.message.attachments[0].id;
    assert_ne!(copied_attachment, attachment);
    let workspace = window.view().workspace().unwrap().id;
    let everyone = crate::channel_use::everyone(&world, &admin, workspace);
    crate::channel_use::rule(
        &world,
        &admin,
        match place {
            PlaceId::Channel(channel) => channel,
            _ => unreachable!(),
        },
        everyone,
        &[],
        &[WorkspacePermission::ViewChannel],
    );
    window.until("the source is unavailable", |window| {
        window.shows_named(format!("forward-unavailable:{}", source.id))
    });
    window.until("the copied file remains available", |window| {
        window.shows_named(format!("download:{copied_attachment}"))
    });
    world
        .run(admin.handle.api().delete_message(source.id))
        .unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let folder = downloads.path().to_path_buf();
    window.state.update(&mut window.cx, |state, _| {
        state.set_download_directory(folder)
    });
    window.click_named(format!("download:{copied_attachment}"));
    window.until("the source file is saved", |window| {
        window
            .view()
            .downloads
            .get(&(server_id, copied_attachment))
            .is_some_and(|download| matches!(download.stage, DownloadStage::Saved(_)))
    });
    let saved = window
        .view()
        .downloads
        .get(&(server_id, copied_attachment))
        .unwrap()
        .stage
        .clone();
    let DownloadStage::Saved(path) = saved else {
        panic!("file was not saved")
    };
    assert_eq!(std::fs::read(path).unwrap(), b"protected attachment");
    window.until("the forwarded message is confirmed", |window| window.messages().iter().any(|row| row.message.id != source.id && row.message.document.as_ref().is_some_and(|document| document.blocks.iter().any(|block| matches!(block, Block::Forward { message_id, .. } if *message_id == source.id)))));
    let forwarded = window.messages().into_iter().find(|row| row.message.document.as_ref().is_some_and(|document| document.blocks.iter().any(|block| matches!(block, Block::Forward { message_id, .. } if *message_id == source.id)))).unwrap();
    window.right_click_named(format!("message:{}", forwarded.message.id));
    window.click("message-copy-text");
    assert!(
        window
            .cx
            .read_from_clipboard()
            .unwrap()
            .text()
            .unwrap()
            .contains(code)
    );
}
