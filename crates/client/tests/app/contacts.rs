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
use crate::harness::{Window, World, member};
use crate::messages::document;
use crate::support::logged_in;

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
    window.cx.run_until_parked();
    let place = window.cx.debug_bounds("contacts-more");
    assert!(place.is_some());
    window.in_shell(|shell, window, cx| shell.type_contact_part("Carl", window, cx));
    window.until_offered(carl_id);
    assert!(rows(&mut window) < listed);
    window.cx.run_until_parked();
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
    window.cx.run_until_parked();
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
    window.cx.run_until_parked();
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
