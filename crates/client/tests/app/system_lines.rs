//! The lines of a channel that the chat draws (ADR 0408, ADR 0740): a pin and a rename, with
//! the names of the channel. The lines of a call are in `live::lines`.

use gpui::TestAppContext;
use junto_client::state::feed::FeedRow;
use junto_client_core::protocol::structure::UpdateChannel;

use crate::harness::{Window, World, member};
use crate::messages::document;

fn line_texts(window: &mut Window) -> Vec<String> {
    window
        .feed()
        .map(|feed| {
            feed.rows
                .iter()
                .filter_map(|row| match row {
                    FeedRow::System(row) => {
                        Some(row.parts.iter().map(|part| part.text.as_str()).collect())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[gpui::test]
fn a_pin_and_a_rename_are_lines_of_the_chat_and_renames_in_a_row_are_one_line(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let channel = window.selected_channel();
    let before = window.channel_names()[0].clone();
    let kept = admin.send(&world, channel, document("keep this"));
    let api = admin.handle.api();
    world.run(api.pin_message(kept.id)).unwrap();
    for name in ["plans", "roadmap"] {
        world
            .run(api.update_channel(
                channel,
                &UpdateChannel {
                    name: Some(name.to_owned()),
                    ..UpdateChannel::default()
                },
            ))
            .unwrap();
    }
    let expected = [
        "Ada pinned a message".to_owned(),
        format!("Ada renamed this channel from {before} to roadmap"),
    ];
    window.until("the chat draws the pin and one rename", |window| {
        let lines = line_texts(window);
        lines.ends_with(&expected)
    });
}
