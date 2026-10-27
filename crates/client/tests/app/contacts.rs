//! A new direct conversation with an account that is in no member list the person can open
//! (ADR 0307): a member writes to an administrator, and an administrator to every account.

use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::ServerHandle;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ServerPermission;
use junto_client_core::protocol::discovery::NotificationKind;
use junto_client_core::protocol::ids::{AccountId, ConversationId};
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::roles::CreateServerRole;

use crate::admin_pages::{default_workspace, invite, joined};
use crate::harness::{Window, World, member, named};
use crate::messages::document;
use crate::support::{FaultProxy, logged_in};

impl Window {
    /// Opens the chooser of a new direct conversation and types a part of a name.
    fn search_contacts(&mut self, part: &str) {
        self.click("new-direct");
        self.until("the chooser is open", |window| {
            window.shows("new-direct-dialog")
        });
        self.in_shell(|shell, window, cx| shell.type_contact_part(part, window, cx));
    }

    /// Waits until the chooser has the answer to the typed part and offers the account.
    /// A row of an earlier answer can be replaced while it is pressed, and such a press
    /// chooses nobody (ADR 0741).
    #[track_caller]
    fn until_offered(&mut self, account_id: AccountId) {
        let row = format!("contact-{account_id}");
        self.until(&format!("{row} is offered for the typed part"), |window| {
            let settled = window.state.read_with(&window.cx, |state, _| {
                state.contacts_view().is_some_and(|view| !view.reading)
            });
            settled && window.shows_named(row.clone())
        });
    }

    /// Chooses an offered account in the chooser, which closes then.
    #[track_caller]
    fn choose_contact(&mut self, account_id: AccountId) {
        self.until_offered(account_id);
        self.click_named(format!("contact-{account_id}"));
        assert!(!self.shows("new-direct-dialog"));
    }

    /// The account of the direct conversation that the main area shows.
    fn conversation_with(&mut self) -> Option<(AccountId, Option<String>)> {
        let conversation = self.view().lists()?.conversation.clone()?;
        Some((conversation.account_id, conversation.name))
    }

    /// Types a text into the composer of the main area and sends it with Enter.
    fn send(&mut self, text: &str) {
        self.in_shell(|shell, window, cx| {
            shell.type_message(Pane::Main, text, window, cx);
            shell.focus_composer(Pane::Main, window, cx);
        });
        self.press("enter");
        self.until_message(text);
    }
}

/// Writes from an account to another one in their direct conversation.
fn write_to(world: &World, from: &ServerHandle, to: AccountId, text: &str) -> ConversationId {
    let api = from.api();
    let conversation = world.run(api.open_conversation(to)).unwrap();
    let message = SendMessage {
        document: document(text),
        reply_to: None,
        attachments: Vec::new(),
    };
    world
        .run(api.send_message(PlaceId::Direct(conversation.id), &message))
        .unwrap();
    conversation.id
}

/// The number of the messages of a direct conversation, as an account reads them from the
/// server.
fn message_count(world: &World, reader: &ServerHandle, conversation_id: ConversationId) -> usize {
    world
        .run(
            reader
                .api()
                .messages(PlaceId::Direct(conversation_id), &Default::default()),
        )
        .unwrap()
        .messages
        .len()
}

#[gpui::test]
fn a_person_writes_to_an_account_that_is_in_no_member_list_of_the_person(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let api = admin.handle.api();

    // Eve administers the server and is a member of a second workspace only; Carl is a
    // member of that workspace and administers nothing.
    let eve_code = invite(&world, &admin.handle, Vec::new());
    let (_eve_device, eve) = joined(&world, &server, &eve_code, "Eve");
    let eve_id = logged_in(&eve);
    let operators = world
        .run(api.create_server_role(&CreateServerRole {
            name: "Operators".to_owned(),
            permissions: vec![ServerPermission::Administrator],
        }))
        .unwrap();
    world
        .run(api.assign_server_role(eve_id, operators.id))
        .unwrap();
    let second = world
        .run(eve.api().create_workspace("Second".to_owned()))
        .unwrap();
    let default = crate::admin_pages::default_workspace(&world, &eve);
    world
        .run(api.remove_workspace_member(default, eve_id, &Default::default()))
        .unwrap();
    let carl_code = invite(&world, &eve, vec![second.id]);
    let (_carl_device, carl) = joined(&world, &server, &carl_code, "Carl");
    let carl_id = logged_in(&carl);

    // Luca is a member of the default workspace. Of the two only Eve is offered: she
    // administers the server.
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca_id = window.view().person.account_id.unwrap();
    window.search_contacts("E");
    window.until_offered(eve_id);
    window.in_shell(|shell, window, cx| shell.type_contact_part("Carl", window, cx));
    window.until("nobody is offered for Carl", |window| {
        window.shows("contacts-none")
    });
    assert!(!window.shows_named(format!("contact-{carl_id}")));

    // Choosing Eve opens the conversation, with her name.
    window.in_shell(|shell, window, cx| shell.type_contact_part("Eve", window, cx));
    window.choose_contact(eve_id);
    window.until("the conversation with Eve is shown by her name", |window| {
        window.conversation_with() == Some((eve_id, Some("Eve".to_owned())))
    });
    window.until("the empty conversation", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "Hello, operator", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    window.press("enter");
    window.until_message("Hello, operator");
    let conversation = world.run(eve.api().conversations()).unwrap();
    assert_eq!(conversation.len(), 1);
    let received = world
        .run(
            eve.api()
                .messages(PlaceId::Direct(conversation[0].id), &Default::default()),
        )
        .unwrap();
    assert_eq!(received.messages.len(), 1);

    // As an administrator Luca reaches Carl too, who shares no workspace with him.
    world
        .run(api.assign_server_role(luca_id, operators.id))
        .unwrap();
    window.until("Luca administers the server", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.client().servers()[0]
                .live()
                .view()
                .model
                .is_some_and(|model| model.is_administrator())
        })
    });
    // "Show more" keeps its place whatever the number of rows that an answer brings.
    window.search_contacts("");
    let settled = |window: &mut Window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .contacts_view()
                .is_some_and(|view| !view.reading && view.rows.is_some())
        })
    };
    window.until("every contact is listed", settled);
    let rows = |window: &mut Window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .contacts_view()
                .and_then(|view| view.rows)
                .map_or(0, |rows| rows.len())
        })
    };
    let listed = rows(&mut window);
    window.run_tasks();
    let place = window.cx.debug_bounds("contacts-more");
    assert!(place.is_some());
    window.in_shell(|shell, window, cx| shell.type_contact_part("Carl", window, cx));
    window.until_offered(carl_id);
    assert!(rows(&mut window) < listed);
    window.run_tasks();
    assert_eq!(window.cx.debug_bounds("contacts-more"), place);
    window.choose_contact(carl_id);
    window.until(
        "the conversation with Carl is shown by his name",
        |window| window.conversation_with() == Some((carl_id, Some("Carl".to_owned()))),
    );
}

#[gpui::test]
fn an_account_without_a_workspace_keeps_its_direct_conversations(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let ada_id = logged_in(&admin.handle);
    let api = admin.handle.api();

    // Eve administers the server as well.
    let eve_code = invite(&world, &admin.handle, Vec::new());
    let (_eve_device, eve) = joined(&world, &server, &eve_code, "Eve");
    let eve_id = logged_in(&eve);
    let operators = world
        .run(api.create_server_role(&CreateServerRole {
            name: "Operators".to_owned(),
            permissions: vec![ServerPermission::Administrator],
        }))
        .unwrap();
    world
        .run(api.assign_server_role(eve_id, operators.id))
        .unwrap();

    // Luca loses the membership of the only workspace of the server. The window states
    // that he sees no workspace, and keeps the notices and the direct conversations.
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca_id = window.view().person.account_id.unwrap();
    let default = default_workspace(&world, &admin.handle);
    world
        .run(api.remove_workspace_member(default, luca_id, &Default::default()))
        .unwrap();
    window.until("the window shows no workspace", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.workspace.is_none())
            && window.shows("no-workspace")
    });
    assert!(window.shows("no-workspace-note") && window.shows("notices-row"));

    // He writes to an administrator from "New direct message", whose name the window reads
    // from the profile.
    window.search_contacts("Ada");
    window.choose_contact(ada_id);
    window.until("the conversation with Ada is shown by her name", |window| {
        window.conversation_with() == Some((ada_id, Some("Ada".to_owned())))
    });
    window.until("the empty conversation", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    window.send("Hello without a workspace");
    let with_ada = world.run(admin.handle.api().conversations()).unwrap();
    assert_eq!(with_ada.len(), 1);
    assert_eq!(message_count(&world, &admin.handle, with_ada[0].id), 1);

    // The other administrator writes to him: the conversation is listed with its unread
    // sign, the server and the notices count it, and "New direct message" stays where it
    // is (ADR 0730).
    window.run_tasks();
    let new_direct = window.cx.debug_bounds("new-direct");
    assert!(new_direct.is_some());
    let with_eve = write_to(&world, &eve, luca_id, "A question from Eve");
    window.until("the conversation with Eve is listed as unread", |window| {
        let view = window.view();
        view.lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|row| row.id == with_eve && row.unread.any())
        }) && view.servers[0].unread.any()
            && window.unread_notices() == 1
    });
    assert!(window.shows_named(format!("direct-row:{with_eve}")) && window.shows("notices-count"));
    window.run_tasks();
    assert_eq!(window.cx.debug_bounds("new-direct"), new_direct);

    // The notice opens beside the shown conversation and leads to the other one, and is
    // read then.
    window.open_notices();
    let notice = window.notices().unwrap().rows[0].clone();
    assert_eq!(notice.kind, NotificationKind::Direct);
    window.click_named(format!("notice:{}", notice.id));
    window.until("the conversation with Eve is shown", |window| {
        window.conversation_with() == Some((eve_id, Some("Eve".to_owned())))
    });
    window.until_message("A question from Eve");
    window.until("the notice is read", |window| window.unread_notices() == 0);

    // The answer goes from the composer of that conversation.
    window.send("An answer from Luca");
    assert_eq!(message_count(&world, &eve, with_eve), 2);
}

impl Window {
    /// The bounds of the parts of the window that a change of the workspaces of the account
    /// must not move, by their names.
    fn bounds_of(
        &mut self,
        names: &[&'static str],
    ) -> Vec<(&'static str, Option<gpui::Bounds<gpui::Pixels>>)> {
        self.run_tasks();
        names
            .iter()
            .map(|name| (*name, self.cx.debug_bounds(name)))
            .collect()
    }
}

#[gpui::test]
fn the_direct_part_keeps_its_place_when_the_last_workspace_goes_and_comes_back(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca_id = window.view().person.account_id.unwrap();
    let default = default_workspace(&world, &admin.handle);

    // Luca reads the conversation that Ada started, with the member list beside it.
    let with_ada = write_to(&world, &admin.handle, luca_id, "Hello from Ada");
    let row = crate::harness::selector(format!("direct-row:{with_ada}"));
    window.until("the conversation is listed", |window| window.shows(row));
    window.click(row);
    window.until_message("Hello from Ada");
    window.click("members-button");
    window.until("the member list is beside the conversation", |window| {
        window.shows("side-panel")
            && window.view().workspace().is_some_and(|workspace| {
                matches!(
                    workspace.members,
                    junto_client::state::view::Members::Loaded(_)
                )
            })
    });
    let parts = [
        "notices-row",
        "new-direct",
        row,
        "channel-title",
        "export-conversation",
        "members-button",
        "message-list",
        "composer",
        "side-panel",
    ];
    let before = window.bounds_of(&parts);
    assert!(
        before.iter().all(|(_, bounds)| bounds.is_some()),
        "{before:?}"
    );

    // The last workspace goes: the direct section, the conversation, its header and the
    // side panel stay where they are (ADR 0730).
    world
        .run(api.remove_workspace_member(default, luca_id, &Default::default()))
        .unwrap();
    window.until("the window shows no workspace", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.workspace.is_none())
    });
    assert_eq!(window.bounds_of(&parts), before);
    assert!(window.shows("no-workspace-note"));

    // A workspace comes back: nothing of it moves either.
    world
        .run(api.add_workspace_member(default, luca_id))
        .unwrap();
    window.until("the window shows the workspace again", |window| {
        window.view().workspace().is_some()
    });
    assert_eq!(window.bounds_of(&parts), before);
    assert!(!window.shows("no-workspace-note"));
}

#[gpui::test]
fn a_conversation_that_the_server_does_not_open_without_a_workspace_is_stated_with_a_retry(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let ada_id = logged_in(&admin.handle);
    let api = admin.handle.api();
    // Eve administers the server and is a member of a second workspace only, so that Luca
    // reaches her only while she administers it.
    let second = world
        .run(api.create_workspace("Second".to_owned()))
        .unwrap();
    let eve_code = invite(&world, &admin.handle, vec![second.id]);
    let (_eve_device, eve) = joined(&world, &server, &eve_code, "Eve");
    let eve_id = logged_in(&eve);
    let operators = world
        .run(api.create_server_role(&CreateServerRole {
            name: "Operators".to_owned(),
            permissions: vec![ServerPermission::Administrator],
        }))
        .unwrap();
    // Luca reaches the server through a proxy that can hold the opening of a conversation.
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = named(cx, "Luca");
    window.find_server(&proxy.address());
    window.join_with_code(admin.invite(&world, 1).expose());
    window.until_ready();
    let luca_id = window.view().person.account_id.unwrap();
    let general = window.selected_channel();
    let with_ada = write_to(&world, &admin.handle, luca_id, "Hello from Ada");
    let ada_row = crate::harness::selector(format!("direct-row:{with_ada}"));
    window.until("the conversation with Ada is listed", |window| {
        window.shows(ada_row)
    });
    window.click(ada_row);
    window.until_message("Hello from Ada");
    // Eve is offered while she administers the server, and stops before Luca chooses her.
    let choose_eve = |window: &mut Window| {
        world
            .run(api.assign_server_role(eve_id, operators.id))
            .unwrap();
        window.search_contacts("Eve");
        window.until_offered(eve_id);
        world
            .run(api.unassign_server_role(eve_id, operators.id))
            .unwrap();
        window.choose_contact(eve_id);
        // The window reads the name of Eve from her profile.
        window.until("the refusal is stated with the name", |window| {
            window.shows("conversation-refused-notice")
                && window.view().lists().is_some_and(|lists| {
                    lists
                        .conversation_refused
                        .as_ref()
                        .is_some_and(|refusal| refusal.name.is_some())
                })
        });
        window
            .view()
            .lists()
            .and_then(|lists| lists.conversation_refused.clone())
            .map(|refusal| (refusal.account_id, refusal.name, refusal.reason))
    };
    let of_eve = Some((
        eve_id,
        Some("Eve".to_owned()),
        junto_client::text::NO_CONTACT.to_owned(),
    ));

    // The refusal names Eve under the conversation with Ada, and belongs to that place: it
    // goes when Luca opens a channel.
    assert_eq!(choose_eve(&mut window), of_eve);
    assert_eq!(
        window.conversation_with(),
        Some((ada_id, Some("Ada".to_owned())))
    );
    window.click_channel("general");
    window.until("the channel is shown", |window| {
        window.view().shown().channel == Some(general)
    });
    assert!(!window.shows("conversation-refused-notice"));

    // Without a workspace the refusal is stated as well, and "Close" puts it away.
    let default = default_workspace(&world, &admin.handle);
    world
        .run(api.remove_workspace_member(default, luca_id, &Default::default()))
        .unwrap();
    window.until("the window shows no workspace", |window| {
        window.shows("no-workspace")
    });
    // The removal is stated by a notice of its own, which the refusal does not move.
    window.until("the notice of the removal", |window| {
        window.shows("moderation-notice")
    });
    window.run_tasks();
    let removal = window.cx.debug_bounds("moderation-notice");
    assert_eq!(choose_eve(&mut window), of_eve);
    window.run_tasks();
    assert_eq!(window.cx.debug_bounds("moderation-notice"), removal);
    window.click("conversation-refusal-close");
    assert!(!window.shows("conversation-refused-notice"));

    // "Try again" while the server holds the answer: the statement stays at its place, and
    // its controls take no click.
    assert_eq!(choose_eve(&mut window), of_eve);
    let opens = |proxy: &FaultProxy| {
        proxy
            .seen("/api/v1/conversations")
            .iter()
            .filter(|seen| seen.method == "POST")
            .count()
    };
    let before = opens(&proxy);
    window.run_tasks();
    let place = window.cx.debug_bounds("conversation-refused-notice");
    proxy.hold_request("POST", "/conversations");
    window.click("conversation-retry");
    window.until("the request reached the proxy", |_| {
        opens(&proxy) == before + 1
    });
    window.click("conversation-retry");
    window.click("conversation-refusal-close");
    window.run_tasks();
    assert_eq!(window.cx.debug_bounds("conversation-refused-notice"), place);
    assert_eq!(opens(&proxy), before + 1);
    proxy.release();
    window.until("the answer is the same refusal", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.conversation_asked.is_none())
    });
    assert!(window.shows("conversation-refused-notice"));

    // Once Eve administers the server again, "Try again" opens the conversation.
    world
        .run(api.assign_server_role(eve_id, operators.id))
        .unwrap();
    window.click("conversation-retry");
    window.until("the conversation with Eve is shown", |window| {
        window.conversation_with().map(|(account_id, _)| account_id) == Some(eve_id)
    });
    assert!(!window.shows("conversation-refused-notice"));
}

#[gpui::test]
fn the_direct_part_shrinks_and_keeps_the_call_controls_in_the_window_of_the_least_size(
    cx: &mut TestAppContext,
) {
    use junto_client_core::protocol::call::CallPlace;
    use junto_media::calls::{ActiveCall, CallTarget, Phase};
    use junto_media::engine::{MicrophoneState, OutputState, ShareState};

    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let ada_id = logged_in(&admin.handle);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca_id = window.view().person.account_id.unwrap();
    let with_ada = write_to(&world, &admin.handle, luca_id, "Hello from Ada");
    let ada_row = crate::harness::selector(format!("direct-row:{with_ada}"));
    window.until("the conversation is listed", |window| window.shows(ada_row));
    window.click(ada_row);
    window.until_message("Hello from Ada");
    window.run_tasks();
    let top = window.cx.debug_bounds("server-lists").unwrap().origin;

    // A direct call with Ada whose microphone the system refuses: the strip of the call and
    // the warning stand under the lists of the column.
    let server_id = window.view().selected.unwrap().id;
    crate::calls::show_call(
        &mut window,
        ActiveCall {
            camera: junto_media::camera::CameraState::Off,
            cameras: Vec::new(),
            target: CallTarget {
                server_id,
                place: CallPlace::direct(luca_id, ada_id),
            },
            identity: "seat".to_owned(),
            phase: Phase::Connected,
            interrupted_at: None,
            microphone: MicrophoneState::NoPermission,
            access: junto_media::permission::MicrophoneAccess::Refused,
            input_device: None,
            output: OutputState::Running,
            output_device: None,
            microphone_block: None,
            sound_block: None,
            share: ShareState::Off,
            shares: Vec::new(),
            watched: None,
            withheld: Default::default(),
        },
    );
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(480.0))));
    window.until("the call and the warning are drawn", |window| {
        window.shows("leave-call") && window.shows("sound-warning")
    });
    window.run_tasks();
    let bounds = |window: &mut Window, name: &'static str| {
        window
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not drawn"))
    };
    // Every control of the column stays in the window, the part of the server keeps its top,
    // and its list of conversations keeps at least one row.
    let height = gpui::px(480.0);
    for name in ["profile-strip", "leave-call", "sound-warning", "new-direct"] {
        let drawn = bounds(&mut window, name);
        assert!(
            drawn.bottom() <= height,
            "{name} ends at {:?}",
            drawn.bottom()
        );
    }
    assert_eq!(bounds(&mut window, "server-lists").origin.y, top.y);
    let list = bounds(&mut window, "direct-list");
    assert!(list.size.height >= gpui::px(28.0), "{list:?}");
    // The channels keep at least one row too.
    let channels = bounds(&mut window, "channel-list");
    assert!(channels.size.height >= gpui::px(28.0), "{channels:?}");
    assert!(bounds(&mut window, ada_row).bottom() <= list.bottom());
}
