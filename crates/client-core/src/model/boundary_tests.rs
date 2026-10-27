use junto_protocol::events::Event;
use junto_protocol::ids::AccountId;
use junto_protocol::message::Message;

use super::message_tests::{general, message};
use super::tests::{Scene, scene};
use super::*;

fn positions(model: &ServerModel, place: PlaceId) -> Vec<u64> {
    model
        .timeline(place)
        .unwrap()
        .messages
        .iter()
        .map(|held| held.position)
        .collect()
}

/// A timeline of the text channel, whose history the account of the scene may not read,
/// with a message of the first connection, one of the second, and one that a read
/// returned while no connection was subscribed. The second connection is the current one.
fn live(scene: &Scene) -> (ServerModel, [Message; 3]) {
    let mut model = scene.model();
    let place = general(scene);
    assert!(model.reads_live_only(place));
    model.open_timeline(place);
    model.open_live(place);
    let other = AccountId::generate();
    let held = [4, 5, 6].map(|position| message(scene.text_place(), position, other, "live"));
    for (read, connection) in held.iter().zip([Some(1), Some(2), None]) {
        model.note_received(read, connection);
        model.put_message(read.clone());
    }
    model.connection = Some(2);
    assert_eq!(positions(&model, place), [4, 5, 6]);
    (model, held)
}

#[test]
fn a_withdrawal_keeps_only_the_messages_that_reads_of_the_current_connection_returned() {
    let scene = scene();
    let (mut model, held) = live(&scene);
    let place = general(&scene);
    let follow = model.apply(&Event::ChannelHistoryWithdrawn {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert!(!follow.reload);
    assert_eq!(positions(&model, place), [5]);
    assert_eq!(
        model.received_in.keys().copied().collect::<Vec<_>>(),
        [held[1].id]
    );
}

#[test]
fn a_withdrawal_of_another_channel_changes_nothing() {
    let scene = scene();
    let (mut model, _) = live(&scene);
    model.apply(&Event::ChannelHistoryWithdrawn {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    });
    assert_eq!(positions(&model, general(&scene)), [4, 5, 6]);
}

#[test]
fn a_reload_after_a_permission_change_gives_back_only_the_messages_of_the_current_connection() {
    let scene = scene();
    let (mut model, _) = live(&scene);
    let place = general(&scene);
    model.apply(&Event::PermissionsChanged {
        workspace_id: Some(scene.workspace_id),
    });
    assert!(positions(&model, place).is_empty());

    let mut next = scene.model();
    next.carry_messages(&model, &[place]);
    next.connection = Some(2);
    assert!(next.open_live(place));
    assert_eq!(positions(&next, place), [5]);
    assert!(next.withheld.is_empty());
}

#[test]
fn a_reload_for_another_reason_keeps_the_messages_of_earlier_connections() {
    let scene = scene();
    let (model, _) = live(&scene);
    let place = general(&scene);
    let mut next = scene.model();
    next.carry_messages(&model, &[place]);
    next.connection = Some(3);
    next.open_live(place);
    assert_eq!(positions(&next, place), [4, 5]);
}
