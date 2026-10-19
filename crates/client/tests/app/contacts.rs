//! A new direct conversation with an account that is in no member list the person can open
//! (ADR 0307): a member writes to an administrator, and an administrator to every account.

use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ServerPermission;
use junto_client_core::protocol::ids::AccountId;
use junto_client_core::protocol::roles::CreateServerRole;

use crate::admin_pages::{invite, joined};
use crate::harness::{Window, World, member};
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

    /// The account of the direct conversation that the main area shows.
    fn conversation_with(&mut self) -> Option<(AccountId, Option<String>)> {
        let conversation = self.view().workspace()?.conversation.clone()?;
        Some((conversation.account_id, conversation.name))
    }
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
    window.until("Eve is offered", |window| {
        window.shows_named(format!("contact-{eve_id}"))
    });
    window.in_shell(|shell, window, cx| shell.type_contact_part("Carl", window, cx));
    window.until("nobody is offered for Carl", |window| {
        window.shows("contacts-none")
    });
    assert!(!window.shows_named(format!("contact-{carl_id}")));

    // Choosing Eve opens the conversation, with her name.
    window.in_shell(|shell, window, cx| shell.type_contact_part("Eve", window, cx));
    window.until("Eve is offered again", |window| {
        window.shows_named(format!("contact-{eve_id}"))
    });
    window.click_named(format!("contact-{eve_id}"));
    assert!(!window.shows("new-direct-dialog"));
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
    window.search_contacts("Carl");
    window.until("Carl is offered", |window| {
        window.shows_named(format!("contact-{carl_id}"))
    });
    window.click_named(format!("contact-{carl_id}"));
    window.until(
        "the conversation with Carl is shown by his name",
        |window| window.conversation_with() == Some((carl_id, Some("Carl".to_owned()))),
    );
}
