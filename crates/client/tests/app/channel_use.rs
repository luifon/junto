//! The permissions of using a channel in the window (ADR 0790): an action that the server
//! refuses in a channel is shown as not available, with its reason. The rules change
//! through the API while the window is open.

use gpui::{ClipboardItem, Image, ImageFormat, TestAppContext};
use junto_client::shell::Pane;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, MessageId, WorkspaceId, WorkspaceRoleId};
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::roles::CreateWorkspaceRole;
use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};

use crate::admin_pages::default_workspace;
use crate::harness::{Admin, Window, World, member};
use crate::messages::document;

const NO_ATTACH: &str = "You cannot attach files in this channel.";

/// The "everyone" role of a workspace.
pub(crate) fn everyone(world: &World, admin: &Admin, workspace_id: WorkspaceId) -> WorkspaceRoleId {
    world
        .run(admin.handle.api().workspace_roles(workspace_id))
        .unwrap()
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap()
        .id
}

/// Sets the rule of a role in a channel.
pub(crate) fn rule(
    world: &World,
    admin: &Admin,
    channel_id: ChannelId,
    role_id: WorkspaceRoleId,
    allow: &[WorkspacePermission],
    deny: &[WorkspacePermission],
) {
    world
        .run(admin.handle.api().set_channel_rule(
            channel_id,
            RuleSubject::Role { role_id },
            &SetChannelOverride {
                allow: allow.to_vec(),
                deny: deny.to_vec(),
            },
        ))
        .unwrap();
}

fn reply(world: &World, admin: &Admin, channel_id: ChannelId, root_id: MessageId, text: &str) {
    let message = SendMessage {
        document: document(text),
        reply_to: None,
        attachments: Vec::new(),
    };
    world
        .run(admin.handle.api().send_message(
            PlaceId::Thread {
                channel_id,
                root_id,
            },
            &message,
        ))
        .unwrap();
}

fn png() -> Vec<u8> {
    let picture = image::RgbaImage::from_pixel(8, 8, image::Rgba([40, 90, 200, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

impl Window {
    fn problem(&mut self, pane: Pane) -> Option<String> {
        self.in_shell(|shell, _, _| shell.composer_problem(pane).map(str::to_owned))
    }

    fn draft_len(&mut self) -> usize {
        self.view().lists().map_or(0, |lists| lists.draft.len())
    }

    fn row(&mut self, message_id: MessageId) -> junto_client::state::feed::MessageRow {
        self.messages()
            .into_iter()
            .find(|row| row.message.id == message_id)
            .unwrap()
    }
}

/// A member and a channel in which the owner writes.
struct Scene {
    world: World,
    /// The server process lives as long as the scene.
    _server: crate::support::TestServer,
    admin: Admin,
    workspace_id: WorkspaceId,
    everyone: WorkspaceRoleId,
    general: ChannelId,
}

fn scene(cx: &mut TestAppContext, name: &str) -> (Scene, tempfile::TempDir, Window) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let workspace_id = default_workspace(&world, &admin.handle);
    let everyone = everyone(&world, &admin, workspace_id);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, name);
    window.until("the messages are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let general = window.selected_channel();
    (
        Scene {
            world,
            _server: server,
            admin,
            workspace_id,
            everyone,
            general,
        },
        directory,
        window,
    )
}

#[gpui::test]
fn a_channel_without_send_messages_shows_a_read_only_line_in_place_of_the_composer(
    cx: &mut TestAppContext,
) {
    let (scene, _directory, mut window) = scene(cx, "Bruno");
    let Scene {
        world,
        admin,
        everyone,
        general,
        ..
    } = &scene;
    window.write("Before the rule");
    let own = window.until_message("Before the rule");
    assert!(own.can_edit && own.can_reply);
    assert!(window.shows("composer") && !window.shows("read-only-bar"));
    let other = admin.send(world, *general, document("From the owner")).id;
    window.until_message("From the owner");

    rule(
        world,
        admin,
        *general,
        *everyone,
        &[],
        &[WorkspacePermission::SendMessages],
    );
    window.until("the composer is replaced by the read-only line", |window| {
        window.shows("read-only-bar") && !window.shows("composer")
    });
    // The server takes no edit and no reply from a person who may not send there.
    let own = window.row(own.message.id);
    assert!(!own.can_edit && !own.can_reply && own.can_delete);
    assert!(!window.row(other).can_reply);
    // A reply in a thread is still allowed: "send messages in threads" is held.
    assert!(window.row(other).can_thread);
    assert!(window.row(other).can_react);
}

#[gpui::test]
fn attach_react_and_create_threads_are_refused_with_their_reason(cx: &mut TestAppContext) {
    let (scene, _directory, mut window) = scene(cx, "Carla");
    let Scene {
        world,
        admin,
        everyone,
        general,
        ..
    } = &scene;
    let plain = admin.send(world, *general, document("No thread yet")).id;
    let reacted = admin.send(world, *general, document("With reactions")).id;
    world
        .run(admin.handle.api().add_reaction(reacted, "👍"))
        .unwrap();
    world
        .run(admin.handle.api().add_reaction(reacted, "🎉"))
        .unwrap();
    let threaded = admin.send(world, *general, document("With a thread")).id;
    reply(world, admin, *general, threaded, "First reply");
    window.until("the reactions are drawn", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == reacted && row.message.reactions.len() == 2)
    });
    // The member adds a reaction of its own before the rule.
    window.click_named(format!("reaction:{reacted}:0"));
    window.until("the own reaction is held", |window| {
        window.row(reacted).message.reactions[0].own
    });
    assert!(window.shows("attach-file"));

    rule(
        world,
        admin,
        *general,
        *everyone,
        &[],
        &[
            WorkspacePermission::AttachFiles,
            WorkspacePermission::AddReactions,
            WorkspacePermission::CreateThreads,
        ],
    );
    window.until("the attach control is not available", |window| {
        window.shows("attach-file-refused") && !window.shows("attach-file")
    });

    // A dropped file and a pasted picture are refused with the same reason.
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("plan.txt");
    std::fs::write(&path, b"plan").unwrap();
    window.drop_files("composer", vec![path]);
    assert_eq!(window.problem(Pane::Main).as_deref(), Some(NO_ATTACH));
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            png(),
        )));
    window.press("secondary-v");
    window.settle();
    assert_eq!(window.problem(Pane::Main).as_deref(), Some(NO_ATTACH));
    assert_eq!(window.draft_len(), 0);

    // Reacting is not available; the own reaction can still be removed.
    window.hover_message(reacted);
    assert!(window.shows_named(format!("react-refused:{reacted}")));
    assert!(!window.shows_named(format!("react:{reacted}")));
    assert!(window.shows_named(format!("add-reaction-refused:{reacted}")));
    window.click_named(format!("reaction:{reacted}:1"));
    window.settle();
    assert!(!window.row(reacted).message.reactions[1].own);
    window.click_named(format!("reaction:{reacted}:0"));
    window.until("the own reaction is removed", |window| {
        !window.row(reacted).message.reactions[0].own
    });

    // A thread is not created; an existing thread opens.
    window.hover_message(plain);
    assert!(window.shows_named(format!("thread-refused:{plain}")));
    assert!(!window.shows_named(format!("thread:{plain}")));
    window.click_named(format!("thread-chip:{threaded}"));
    window.until("the existing thread takes a reply", |window| {
        window.shows("thread-composer") && !window.shows("thread-read-only-bar")
    });
}

#[gpui::test]
fn the_thread_composer_follows_send_messages_in_threads_and_create_threads(
    cx: &mut TestAppContext,
) {
    let (scene, _directory, mut window) = scene(cx, "Dara");
    let Scene {
        world,
        admin,
        everyone,
        general,
        ..
    } = &scene;
    let root = admin.send(world, *general, document("A root")).id;
    window.until_message("A root");
    window.hover_message(root);
    window.click_named(format!("thread:{root}"));
    window.until("the thread of the root is open", |window| {
        window.shows("thread-composer")
    });

    // The thread has no reply yet: its first reply creates it.
    rule(
        world,
        admin,
        *general,
        *everyone,
        &[],
        &[WorkspacePermission::CreateThreads],
    );
    window.until("the new thread takes no reply", |window| {
        window.shows("thread-read-only-bar") && !window.shows("thread-composer")
    });
    assert_eq!(
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .map(|thread| thread.root_id),
        Some(root)
    );
    assert!(window.shows("composer"));

    // Once the thread has a reply, "send messages in threads" decides.
    reply(world, admin, *general, root, "The owner replies");
    window.until("the created thread takes a reply", |window| {
        window.shows("thread-composer") && !window.shows("thread-read-only-bar")
    });
    rule(
        world,
        admin,
        *general,
        *everyone,
        &[],
        &[WorkspacePermission::SendMessagesInThreads],
    );
    window.until("the thread takes no reply", |window| {
        window.shows("thread-read-only-bar") && !window.shows("thread-composer")
    });
    assert!(window.shows("composer"));
}

#[gpui::test]
fn mention_suggestions_offer_only_what_the_person_may_mention(cx: &mut TestAppContext) {
    let (scene, _directory, mut window) = scene(cx, "Emil");
    let Scene {
        world,
        admin,
        workspace_id,
        everyone,
        general,
        ..
    } = &scene;
    let role = CreateWorkspaceRole {
        name: "Editors".to_owned(),
        permissions: Vec::new(),
    };
    world
        .run(
            admin
                .handle
                .api()
                .create_workspace_role(*workspace_id, &role),
        )
        .unwrap();
    window.until("the role is in the workspace", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.roles.iter().any(|(_, name)| name == "Editors"))
    });
    let names =
        |window: &mut Window| window.in_shell(|shell, _, _| shell.mention_names(Pane::Main));
    let type_at = |window: &mut Window| {
        window.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Main, "", window, cx);
            shell.type_message(Pane::Main, "@", window, cx);
            shell.focus_composer(Pane::Main, window, cx);
        });
    };
    type_at(&mut window);
    let offered = names(&mut window);
    assert!(offered.iter().any(|name| name == "Editors"), "{offered:?}");
    assert!(
        !offered.iter().any(|name| name == "everyone"),
        "{offered:?}"
    );

    // "mention everyone" gives @everyone; without "mention roles" no role is offered.
    rule(
        world,
        admin,
        *general,
        *everyone,
        &[WorkspacePermission::MentionEveryone],
        &[WorkspacePermission::MentionRoles],
    );
    window.until_read(
        "the picker offers everyone and no role",
        |window| {
            type_at(window);
            names(window)
        },
        |offered| {
            offered.iter().any(|name| name == "everyone")
                && !offered.iter().any(|name| name == "Editors")
                && offered.iter().any(|name| name == "Ada")
        },
    );
}
