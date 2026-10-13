//! The derivation of the pictures of people (ADR 0783): which pictures the view carries,
//! and which accounts it names for the reads of the state of the application.

use std::sync::Arc;

use gpui::RenderImage;
use junto_client_core::model::{Loadable, Reading, Timeline};
use junto_client_core::protocol::account::{Avatar, ImageType};
use junto_client_core::protocol::call::{Call, CallParticipant, CallPlace, ChannelCall};
use junto_client_core::protocol::ids::{AccountId, CallId};
use junto_client_core::protocol::message::{Conversation, MessagePlace};
use junto_client_core::protocol::structure::WorkspaceMember;

use super::panel_tests::message;
use super::tests::{Scene, logged_in, scene, server, time};
use super::*;
use crate::state::people::{Face, PictureHash, Wanted};

fn image() -> Arc<RenderImage> {
    let pixels = image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
}

fn hash(number: u8) -> PictureHash {
    PictureHash::of(&[number])
}

fn avatar(hash: PictureHash) -> Avatar {
    Avatar {
        hash: hash.to_string(),
        media_type: ImageType::Png,
        bytes: 100,
        width: 2,
        height: 2,
    }
}

fn member(account_id: AccountId, name: &str) -> WorkspaceMember {
    WorkspaceMember {
        avatar: None,
        account_id,
        name: name.to_owned(),
        role_ids: Vec::new(),
    }
}

/// A call in "Daily" with these people inside.
fn call(scene: &mut Scene, people: &[AccountId]) {
    let (workspace_id, channel_id) = (scene.studio, scene.daily);
    scene.studio_mut().calls.push(ChannelCall {
        workspace_id,
        channel_id,
        in_use: true,
        call: Some(Call {
            id: CallId::generate(),
            place: CallPlace::Channel {
                workspace_id,
                channel_id,
            },
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: false,
            participants: people
                .iter()
                .map(|account_id| CallParticipant {
                    avatar: None,
                    account_id: *account_id,
                    identity: format!("seat-{account_id}"),
                    joined_at: time(),
                    muted: false,
                    deafened: false,
                    microphone_block: None,
                    sound_block: None,
                    hand_raised: false,
                    screen_share: None,
                })
                .collect(),
            recording: None,
        }),
    });
}

#[test]
fn the_view_carries_the_picture_of_each_account_whose_profile_names_one() {
    let scene = scene();
    let server = server(
        "Acme Studio",
        Some(scene.model()),
        logged_in(scene.own, Vec::new()),
    );
    let (ana, ben, caio) = (
        AccountId::generate(),
        AccountId::generate(),
        AccountId::generate(),
    );
    let held = image();
    let mut known = Known::default();
    // The window holds the image of Ana, reads the image of Ben, and Caio has no avatar.
    known
        .people
        .hold(server.id, ana, "Ana", Some((hash(1), Some(held.clone()))));
    known
        .people
        .hold(server.id, ben, "Ben", Some((hash(2), None)));
    known.people.hold(server.id, caio, "Caio", None);
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    assert_eq!(
        view.faces.face(server.id, ana),
        Some(&Face {
            hash: hash(1),
            image: Some(held.clone())
        })
    );
    assert_eq!(
        view.faces.face(server.id, ben),
        Some(&Face {
            hash: hash(2),
            image: None
        })
    );
    assert_eq!(view.faces.face(server.id, caio), None);
    // A drawing takes the pictures of its server once.
    let faces = view.faces.of(server.id);
    assert_eq!(faces.image(ana), Some(held));
    assert_eq!(faces.image(ben), None);
    assert_eq!(faces.image(AccountId::generate()), None);
    // A picture belongs to its server.
    assert_eq!(view.faces.face(ServerId::generate(), ana), None);
}

#[test]
fn the_own_account_shows_the_avatar_that_the_state_of_the_server_tells() {
    let mut scene = scene();
    scene.snapshot.account.avatar = Some(avatar(hash(1)));
    let own = scene.own;
    let server = server(
        "Acme Studio",
        Some(scene.model()),
        logged_in(own, Vec::new()),
    );
    let mut known = Known::default();
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    // The image is not read yet: the view names it for the read.
    assert_eq!(
        view.faces.face(server.id, own),
        Some(&Face {
            hash: hash(1),
            image: None
        })
    );
    assert_eq!(
        view.wanted_people[&server.id].own,
        Some((own, Some(hash(1))))
    );

    // A profile of the own account that was read is not what the window shows of it, and
    // the image with the hash of the state of the server is.
    let held = image();
    let other = AccountId::generate();
    known
        .people
        .hold(server.id, own, "Old name", Some((hash(2), Some(image()))));
    known
        .people
        .hold(server.id, other, "Ana", Some((hash(1), Some(held.clone()))));
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    assert_eq!(
        view.faces.face(server.id, own),
        Some(&Face {
            hash: hash(1),
            image: Some(held)
        })
    );
    assert_eq!(view.person.name, "Luca");

    // An avatar whose hash is not a hash is no avatar.
    scene.snapshot.account.avatar = Some(Avatar {
        hash: "not-a-hash".to_owned(),
        ..avatar(hash(1))
    });
    let server = ServerSnapshot {
        id: server.id,
        ..super::tests::server(
            "Acme Studio",
            Some(scene.model()),
            logged_in(own, Vec::new()),
        )
    };
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    assert_eq!(view.faces.face(server.id, own), None);
    assert_eq!(view.wanted_people[&server.id].own, Some((own, None)));
}

#[test]
fn the_view_names_the_people_that_it_shows_the_most_visible_first() {
    let mut scene = scene();
    let (ana, ben, caio, dora, zoe) = (
        AccountId::generate(),
        AccountId::generate(),
        AccountId::generate(),
        AccountId::generate(),
        AccountId::generate(),
    );
    let own = scene.own;
    // Ana is in the voice channel, Ben wrote the message of the shown channel, Zoe is the
    // other person of a direct conversation, and Caio and Dora are members.
    call(&mut scene, &[ana, own]);
    let mut model = scene.model();
    let general = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.general,
    };
    model.timelines.push(Arc::new(Timeline {
        place: PlaceId::Channel(scene.general),
        messages: vec![Arc::new(message(general, 1, ben, "hello"))],
        more_before: false,
        more_after: false,
        reading: Reading::default(),
        loaded: true,
        failed: None,
        stale: false,
        unread_after: None,
        root: None,
        forks: Vec::new(),
    }));
    model.conversations = vec![Conversation {
        id: junto_client_core::protocol::ids::ConversationId::generate(),
        low_account_id: own.min(zoe),
        high_account_id: own.max(zoe),
    }];
    Arc::make_mut(&mut model.workspaces[1]).members = Loadable::Loaded(vec![
        member(dora, "Dora"),
        member(caio, "Caio"),
        member(ana, "Ana"),
        member(own, "Luca"),
    ]);
    let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
    let view = derive(
        std::slice::from_ref(&server),
        &Selection::default(),
        &Known::default(),
    );
    assert_eq!(
        view.wanted_people,
        HashMap::from([(
            server.id,
            Wanted {
                own: Some((own, None)),
                // The people of the channel column, the authors of the messages, the
                // direct conversations, and then the members in the order of the list.
                accounts: vec![ana, own, ben, zoe, ana, caio, dora, own],
            }
        )])
    );
}

#[test]
fn a_server_that_shows_no_content_names_nobody_and_carries_no_picture() {
    let scene = scene();
    let ana = AccountId::generate();
    // The device has no session: the state that the event stream still holds is not
    // shown, and neither is a picture that was read for the account before.
    let server = server(
        "Acme Studio",
        Some(scene.model()),
        AccessState::NoAccount { notice: None },
    );
    let mut known = Known::default();
    known
        .people
        .hold(server.id, ana, "Ana", Some((hash(1), Some(image()))));
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    assert_eq!(view.faces.face(server.id, ana), None);
    assert!(view.wanted_people.is_empty());
}

#[test]
fn the_preset_picture_and_the_servers_that_refused_it_are_in_the_view() {
    let scene = scene();
    let acme = server(
        "Acme Studio",
        Some(scene.model()),
        logged_in(scene.own, Vec::new()),
    );
    let harbor = server("Harbor", None, AccessState::NoAccount { notice: None });
    let lab = server("Lab", None, AccessState::NoAccount { notice: None });
    let servers = [acme.clone(), harbor.clone(), lab.clone()];
    let view = derive(&servers, &Selection::default(), &Known::default());
    assert_eq!(view.preset_picture, None);
    assert!(view.preset_refused.is_empty());

    let picture = image();
    let known = Known {
        preset_picture: Some(picture.clone()),
        preset_refused: HashSet::from([lab.id, acme.id, ServerId::generate()]),
        ..Known::default()
    };
    let view = derive(&servers, &Selection::default(), &known);
    assert_eq!(view.preset_picture, Some(picture));
    // In the order of the server book.
    assert_eq!(
        view.preset_refused,
        [
            (acme.id, "Acme Studio".to_owned()),
            (lab.id, "Lab".to_owned())
        ]
    );
}
