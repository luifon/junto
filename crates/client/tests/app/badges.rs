//! The counts of what waits for the person (ADR 0741, ADR 0761): each place draws its
//! number, read from what the window paints, and the number is correct after a read, a
//! removal and a restart.

use gpui::TestAppContext;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::ArchivePeriod;

use crate::admin_pages::owner_window;
use crate::harness::{Window, World, member, open};
use crate::messages::document;
use crate::notices::mention;

impl Window {
    /// The number that a badge or a corner count draws inside the element with this name.
    pub fn drawn_number(&mut self, selector: &'static str) -> Option<String> {
        let _record = junto_client::ui::record_painted_text();
        self.cx.update(|window, _| window.refresh());
        self.settle();
        let text = junto_client::ui::take_painted_text();
        let bounds = self.cx.debug_bounds(selector)?;
        text.into_iter()
            .filter(|text| bounds.contains(&text.bounds.center()))
            .map(|text| text.text.to_string())
            .find(|text| {
                !text.is_empty()
                    && text
                        .chars()
                        .all(|letter| letter.is_ascii_digit() || letter == '+')
            })
    }

    /// The count of the icon of the application and of the title of the main window.
    pub fn badge(&mut self) -> Option<u64> {
        self.in_shell(|shell, _, _| shell.badge())
    }
}

/// Direct messages and mentions are counted on the row of the conversation, on the heading
/// of the direct messages, on the server, on the icon of the application and in the title;
/// a read takes them away, and a restart counts what is still unread.
#[gpui::test]
fn the_messages_for_the_person_are_counted_everywhere_and_a_read_takes_them_away(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let own = window.own_account();
    let api = admin.handle.api();
    let conversation = world.run(api.open_conversation(own)).unwrap();
    for text in ["one", "two"] {
        world
            .run(api.send_message(
                PlaceId::Direct(conversation.id),
                &SendMessage {
                    document: document(text),
                    reply_to: None,
                    attachments: Vec::new(),
                },
            ))
            .unwrap();
    }
    let row = crate::harness::selector(format!("direct-row:{}", conversation.id));
    window.until("the conversation counts two messages", |window| {
        window.drawn_number(row).as_deref() == Some("2")
    });
    assert_eq!(window.drawn_number("directs-heading").as_deref(), Some("2"));
    assert_eq!(window.badge(), Some(2));
    // A mention in a channel that the window does not look at counts too.
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    admin.send(&world, general, mention("look, ", own));
    window.until("the mention counts on the icon", |window| {
        window.badge() == Some(3)
    });
    assert_eq!(junto_client::app_badge::title(3), "(3) Junto");
    // The tray takes the same count for its mark.
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.for_person()),
        3
    );

    // A restart counts what is still unread.
    window.close();
    let mut window = open(cx, directory.path());
    window.until("the counts after a restart", |window| {
        window.badge() == Some(3) && window.drawn_number(row).as_deref() == Some("2")
    });

    // The person reads the conversation and the channel: every count goes.
    window.cx.update(|window, _| window.activate_window());
    window.click_named(format!("direct-row:{}", conversation.id));
    window.until("the conversation is read", |window| {
        window.badge() == Some(1) && window.drawn_number(row).is_none()
    });
    assert!(window.drawn_number("directs-heading").is_none());
    let first = window.channel_names()[0].clone();
    window.click_channel(&first);
    window.until("the mention is read", |window| window.badge() == Some(0));
    assert_eq!(junto_client::app_badge::title(0), "Junto");
}

/// The administration counts the exports that are ready to save and the archived channels
/// whose deletion is due, beside the join requests (ADR 0761).
#[gpui::test]
fn the_administration_counts_the_ready_exports_and_the_channels_deleted_soon(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let api = owner.api();
    let workspace_id = window.view().workspace().unwrap().id;
    let general = window.selected_channel();
    world.run(api.export_channel(general)).unwrap();
    world
        .run(api.set_archive_period(workspace_id, ArchivePeriod { days: Some(7) }))
        .unwrap();
    let doomed = window.channels().into_iter().find(|row| row.id != general).unwrap();
    world.run(api.archive_channel(doomed.id)).unwrap();
    window.open_administration();
    window.until("the navigation counts the export and the archived channel", |window| {
        window.drawn_number("admin-nav-exports").as_deref() == Some("1")
            && window.drawn_number("admin-nav-archive").as_deref() == Some("1")
    });
    // The channel is restored: its count goes.
    world.run(api.restore_channel(doomed.id)).unwrap();
    window.until("the restored channel is not counted", |window| {
        window.drawn_number("admin-nav-archive").is_none()
    });
}
