use crate::{
    admin_pages::{invite, joined},
    harness::{World, member},
    messages::document,
    support::logged_in,
};
use gpui::TestAppContext;
use junto_client::state::profile::ProfileRead;
use junto_client_core::{
    model::PlaceId,
    protocol::{
        message::SendMessage,
        presence::{PresenceChoice, SetPresence},
    },
};

#[gpui::test]
fn group_reactors_keep_profiles_presence_and_menu_lifetime(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_writer_directory, mut writer) = member(cx, &world, &server.address(), &admin, "Writer");
    let (_reader_directory, mut reader) = member(cx, &world, &server.address(), &admin, "Reader");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let writer_id = writer.own_account();
    let third_id = logged_in(&third);
    let group = world
        .run(
            reader
                .handle()
                .api()
                .create_group("Planning".to_owned(), vec![writer_id, third_id]),
        )
        .unwrap();
    reader.until("the group is listed", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
    });
    reader.click_named(format!("direct-row:{}", group.id));
    let message = world
        .run(writer.handle().api().send_message(
            PlaceId::Direct(group.id),
            &SendMessage {
                document: document("a group review"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    world
        .run(writer.handle().api().set_presence(&SetPresence {
            choice: PresenceChoice::DoNotDisturb,
        }))
        .unwrap();
    world
        .run(writer.handle().api().add_reaction(message.id, "👍"))
        .unwrap();
    world
        .run(third.api().add_reaction(message.id, "👍"))
        .unwrap();
    world
        .run(reader.handle().api().add_reaction(message.id, "👍"))
        .unwrap();
    reader.until_message("a group review");
    reader.until("the group roster control is drawn", |window| {
        window.shows_named(format!("reaction-people:{}", message.id))
    });
    reader.click_named(format!("reaction-people:{}", message.id));
    reader.until(
        "all three group reactors have identity controls",
        |window| {
            [writer_id, third_id, window.own_account()]
                .into_iter()
                .all(|account| window.shows_named(format!("reaction-profile:{account}")))
        },
    );
    reader.until("the group reactor's current presence is shown", |window| {
        window.shows_named(format!("reaction-presence:{writer_id}:DoNotDisturb"))
    });
    reader.click_named(format!("reaction-profile:{writer_id}"));
    reader.until("the group reactor's profile opens", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.profile_view(window.shell.entity_id())
            })
            .is_some_and(|view| {
                view.account == writer_id && matches!(view.read, ProfileRead::Shown(_))
            })
    });
    reader.press("escape");
    reader.press("shift-f10");
    assert!(reader.shows_named(format!("account-copy-id:{writer_id}")));
    reader.click_named(format!("account-copy-id:{writer_id}"));
    assert_eq!(
        reader.cx.read_from_clipboard().unwrap().text().unwrap(),
        writer_id.to_string()
    );
    assert!(reader.shows("reaction-people-dialog"));
    reader.press("enter");
    reader.until("the same reactor opens with the keyboard", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.profile_view(window.shell.entity_id())
            })
            .is_some_and(|view| {
                view.account == writer_id && matches!(view.read, ProfileRead::Shown(_))
            })
    });
    reader.press("escape");
    reader.press("menu");
    assert!(reader.shows("account-menu"));
    world
        .run(reader.handle().api().leave_group(group.id))
        .unwrap();
    reader.until(
        "leaving removes the current roster and its menu",
        |window| !window.shows("account-menu") && !window.shows("reaction-people-dialog"),
    );
    assert!(
        world
            .run(reader.handle().api().profile_card(writer_id))
            .is_ok()
    );
    assert!(
        world
            .run(
                admin
                    .handle
                    .api()
                    .messages(PlaceId::Direct(group.id), &Default::default())
            )
            .is_err()
    );
}

#[gpui::test]
fn group_typing_tracks_multiple_current_members_and_withdraws_departed_sources(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_writer_directory, mut writer) = member(cx, &world, &server.address(), &admin, "Writer");
    let (_reader_directory, mut reader) = member(cx, &world, &server.address(), &admin, "Reader");
    let (_third_directory, mut third) = member(cx, &world, &server.address(), &admin, "Third");
    let (writer_id, third_id) = (writer.own_account(), third.own_account());
    let group = world
        .run(
            reader
                .handle()
                .api()
                .create_group("Planning".to_owned(), vec![writer_id, third_id]),
        )
        .unwrap();
    for window in [&mut writer, &mut reader, &mut third] {
        window.until("the private group arrives", |window| {
            window.shows_named(format!("direct-row:{}", group.id))
        });
        window.click_named(format!("direct-row:{}", group.id));
        window.until("the group composer is loaded", |window| {
            window
                .feed()
                .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Direct(group.id))
        });
    }
    world
        .run(writer.handle().api().set_presence(&SetPresence {
            choice: PresenceChoice::DoNotDisturb,
        }))
        .unwrap();
    reader.until_typing_watched();
    writer.until_typing_watched();
    third.until_typing_watched();
    writer.type_into("composer-input", "writer draft");
    third.type_into("composer-input", "third draft");
    assert_eq!(
        writer.in_shell(|shell, _, cx| shell.composer_text(junto_client::shell::Pane::Main, cx)),
        "writer draft"
    );
    assert_eq!(
        third.in_shell(|shell, _, cx| shell.composer_text(junto_client::shell::Pane::Main, cx)),
        "third draft"
    );
    reader.until("two current group members are named", |window| {
        window.shows_named(format!("typing-person:{writer_id}"))
            && window.shows_named(format!("typing-person:{third_id}"))
    });
    reader.until("the named group typer has current presence", |window| {
        window.shows_named(format!("typing-presence:{writer_id}:DoNotDisturb"))
    });
    reader.click_named(format!("typing-person:{writer_id}"));
    reader.until("the group typer's profile is current", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.profile_view(window.shell.entity_id())
            })
            .is_some_and(|view| {
                view.account == writer_id && matches!(view.read, ProfileRead::Shown(_))
            })
    });
    reader.press("escape");
    writer.type_into("composer-input", " continued");
    third.type_into("composer-input", " continued");
    reader.right_click_named(format!("typing-person:{third_id}"));
    assert!(reader.shows_named(format!("account-copy-id:{third_id}")));
    world
        .run(third.handle().api().leave_group(group.id))
        .unwrap();
    reader.until(
        "the departed member's activity and menu are withdrawn",
        |window| {
            !window.shows_named(format!("typing-person:{third_id}"))
                && !window.shows("account-menu")
                && window.shows_named(format!("typing-person:{writer_id}"))
        },
    );
    assert_eq!(
        writer.in_shell(|shell, _, cx| shell.composer_text(junto_client::shell::Pane::Main, cx)),
        "writer draft continued"
    );
    reader.right_click_named(format!("typing-person:{writer_id}"));
    world
        .run(reader.handle().api().leave_group(group.id))
        .unwrap();
    reader.until(
        "the old group cannot retain activity after own departure",
        |window| !window.shows("typing-indicator") && !window.shows("account-menu"),
    );
    assert!(
        world
            .run(
                admin
                    .handle
                    .api()
                    .messages(PlaceId::Direct(group.id), &Default::default())
            )
            .is_err()
    );
}
