use crate::channel_use::{everyone, rule};
use crate::harness::{World, member};
use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::protocol::{
    WorkspacePermission,
    message::{Block, Inline},
};

#[gpui::test]
fn channel_references_are_tokens_that_open_the_channel(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    window.type_into("composer-input", "See #gen");
    window.until("the channel picker offers general", |window| {
        window
            .in_shell(|shell, _, _| shell.mention_names(Pane::Main))
            .contains(&"general".to_owned())
    });
    assert!(window.in_shell(|shell, window, cx| shell.pick_mention(
        Pane::Main,
        "general",
        window,
        cx
    )));
    window.press("enter");
    window.until("the channel reference is stored", |window| {
        window.messages().iter().any(|row| {
            let value = serde_json::to_value(&row.message.document).unwrap();
            value.to_string().contains("\"kind\":\"channel\"")
        })
    });
    let document = window
        .messages()
        .last()
        .unwrap()
        .message
        .document
        .clone()
        .unwrap();
    let raw = serde_json::to_value(&document).unwrap();
    assert_eq!(
        raw["blocks"][0]["content"][1]["channel_id"],
        general.to_string()
    );
    let id = window.messages().last().unwrap().message.id;
    window.in_shell(|shell, window, cx| shell.edit_message(Pane::Main, id, window, cx));
    assert!(
        window
            .in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx))
            .contains("#general")
    );
    window.press("escape");
    window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    window.type_into("composer-input", "Read #gen");
    window.until("the conversation can name a channel", |window| {
        window
            .in_shell(|shell, _, _| shell.mention_names(Pane::Main))
            .contains(&"general".to_owned())
    });
    assert!(window.in_shell(|shell, window, cx| shell.pick_mention(
        Pane::Main,
        "general",
        window,
        cx
    )));
    window.press("enter");
    window.until_message("Read #general");
    window.settle();
    let point = window
        .in_shell(|shell, _, _| shell.readonly_text_point("Read #general", 7))
        .unwrap();
    window.press_at(point, gpui::MouseButton::Left);
    window.until("the inline reference opens its channel", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.channel.as_ref())
            .is_some_and(|channel| channel.id == general)
    });
    assert!(!window.shows("message-menu"));
    let root = admin.send(
        &world,
        general,
        crate::messages::document("a discussion about channels"),
    );
    window.until_message("a discussion about channels");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the thread composer opens", |window| {
        window.shows("thread-composer-input")
    });
    window.type_into("thread-composer-input", "Thread #gen");
    window.until("the thread offers channels", |window| {
        window
            .in_shell(|shell, _, _| shell.mention_names(Pane::Thread))
            .contains(&"general".to_owned())
    });
    assert!(window.in_shell(|shell, window, cx| shell.pick_mention(
        Pane::Thread,
        "general",
        window,
        cx
    )));
    window.press("enter");
    window.until("the thread sends a channel reference", |window| {
        window.replies().contains(&"Thread #general".to_owned())
    });
    // The count of the replies and the follow state that the reply gives arrive after the
    // reply and change the panel above it: the press waits for them, so that the reply is
    // where the last drawing put it.
    window.until(
        "the thread holds the state that its first reply gives",
        |window| {
            window
                .view()
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| {
                    thread
                        .root
                        .as_ref()
                        .is_some_and(|root| root.message.reply_count == 1)
                        && thread
                            .follow
                            .as_ref()
                            .is_some_and(|follow| follow.following)
                })
        },
    );
    window.settle();
    let point = window
        .in_shell(|shell, _, _| shell.readonly_text_point("Thread #general", 9))
        .unwrap();
    window.press_at(point, gpui::MouseButton::Left);
    window.until("the thread reference opens the main channel", |window| {
        !window.shows("thread-composer-input")
    });
}

#[gpui::test]
fn channel_mentions_need_permission_and_keep_the_existing_notification_node(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.type_into("composer-input", "@channel");
    assert!(
        !window
            .in_shell(|shell, _, _| shell.mention_names(Pane::Main))
            .contains(&"channel".to_owned())
    );
    let workspace = world.run(admin.handle.api().workspaces()).unwrap()[0].id;
    rule(
        &world,
        &admin,
        window.selected_channel(),
        everyone(&world, &admin, workspace),
        &[WorkspacePermission::MentionEveryone],
        &[],
    );
    window.until("the granted channel mention is offered", |window| {
        window.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Main, "@channel", window, cx);
            shell
                .mention_names(Pane::Main)
                .contains(&"channel".to_owned())
        })
    });
    assert!(window.in_shell(|shell, window, cx| shell.pick_mention(
        Pane::Main,
        "channel",
        window,
        cx
    )));
    window.press("enter");
    window.until("the channel mention is sent", |window| window.messages().iter().any(|row| row.message.document.as_ref().is_some_and(|document| document.blocks.iter().any(|block| matches!(block, Block::Paragraph { content } if content.contains(&Inline::Everyone))))));
    window.until("the new mention is painted as @channel", |window| {
        window
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .iter()
            .any(|text| text.contains("@channel"))
    });
}

#[gpui::test]
fn channel_references_lose_hidden_names_and_use_the_shared_refusal(cx: &mut TestAppContext) {
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::{Document, SendMessage};
    use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let channel = window.selected_channel();
    let target = world
        .run(admin.handle.api().snapshot())
        .unwrap()
        .snapshot
        .workspaces[0]
        .channels
        .iter()
        .find(|row| row.id != channel)
        .unwrap()
        .clone();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Channel(channel),
            &SendMessage {
                document: Document {
                    blocks: vec![Block::Paragraph {
                        content: vec![
                            Inline::Text {
                                text: "See ".to_owned(),
                                marks: Vec::new(),
                            },
                            Inline::Channel {
                                channel_id: target.id,
                            },
                        ],
                    }],
                },
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message(&format!("See #{}", target.name));
    let account = window.own_account();
    world
        .run(admin.handle.api().set_channel_rule(
            target.id,
            RuleSubject::Person {
                account_id: account,
            },
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until("the denied target has no name in the document", |window| {
        window
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .iter()
            .any(|text| text == "See #channel")
    });
    let point = window
        .in_shell(|shell, _, _| shell.readonly_text_point("See #channel", 6))
        .unwrap();
    window.press_at(point, gpui::MouseButton::Left);
    window.until("the server refuses the hidden reference", |window| {
        window.shows("message-link-refusal")
    });
    assert!(!window.shows("message-menu"));
}
