use junto_client_core::Reach;
use junto_client_core::api::{Rejection, TransportError, TransportErrorKind};
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::StreamStatus;
use junto_client_core::profile;
use junto_client_core::protocol::account::{Avatar, ImageType};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::AccountId;

use super::*;
use crate::state::people::PICTURE_SIDE;
use crate::state::view::{Known, Selection, ServerSnapshot, derive};

fn png(side: u32, shade: u8) -> Vec<u8> {
    let picture = image::RgbaImage::from_pixel(side, side, image::Rgba([shade, 40, 200, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// A logo that is another one for another shade.
fn logo(shade: u8) -> Logo {
    let bytes = png(4, shade);
    Logo {
        hash: PictureHash::of(&bytes),
        image: decode(&bytes, AVATAR_SIDE).unwrap(),
    }
}

fn stated(hash: PictureHash) -> Told {
    Told::Present(Some(hash))
}

const NAMED: Told = Told::NotStated { named: true };
const NOT_NAMED: Told = Told::NotStated { named: false };

fn unreachable() -> ApiError {
    ApiError::Unreachable(TransportError::new(
        TransportErrorKind::Connect,
        "no server",
    ))
}

fn refused(code: ErrorCode, status: u16) -> ApiError {
    ApiError::Rejected(Rejection {
        code,
        status,
        retry_after: None,
        message: String::new(),
        left: None,
    })
}

/// One server of the cache.
struct Scene {
    logos: Logos,
    server: ServerId,
}

fn scene() -> Scene {
    Scene {
        logos: Logos::default(),
        server: ServerId::generate(),
    }
}

impl Scene {
    fn want(&mut self, told: Told) {
        self.logos.want(self.server, told);
    }

    fn next(&mut self, again: bool) -> Option<Ask> {
        self.logos.next(self.server, again)
    }

    fn image(&self) -> Option<Arc<RenderImage>> {
        self.logos.image(self.server)
    }

    /// Starts the request that the cache names, and answers it with this logo.
    fn read(&mut self, logo: Option<Logo>) {
        let ask = self.next(false).unwrap();
        self.logos.answered(&ask, Answer::Read(logo));
    }

    /// The cache holds this logo after a read of the logo that the server book names.
    fn holding(logo: &Logo) -> Self {
        let mut scene = scene();
        scene.want(NAMED);
        scene.read(Some(logo.clone()));
        assert_eq!(scene.image(), Some(logo.image.clone()));
        scene
    }
}

fn server(id: ServerId) -> ServerSnapshot {
    ServerSnapshot {
        id,
        name: "Acme Studio".to_owned(),
        address: "acme.example:7443".to_owned(),
        keep_connected: true,
        reach: Reach::Unknown,
        access: AccessState::LoggedOut {
            account_id: AccountId::generate(),
        },
        in_progress: None,
        error: None,
        stream: StreamStatus::Off,
        model: None,
        server_version: None,
    }
}

#[test]
fn what_is_told_follows_the_state_of_the_server_and_the_server_book() {
    let hash = PictureHash::of(b"logo");
    let description = |hash: String| Avatar {
        hash,
        media_type: ImageType::Png,
        bytes: 100,
        width: 64,
        height: 64,
    };
    assert_eq!(Told::of(&LogoState::NotStated, true), NAMED);
    assert_eq!(Told::of(&LogoState::NotStated, false), NOT_NAMED);
    // An event is newer than the path of the server book.
    assert_eq!(Told::of(&LogoState::Stated(None), true), Told::Absent);
    assert_eq!(
        Told::of(
            &LogoState::Stated(Some(description(hash.to_string()))),
            false
        ),
        stated(hash)
    );
    // Other text of a server is no hash: the logo exists, and its image is read.
    assert_eq!(
        Told::of(
            &LogoState::Stated(Some(description("ABC".to_owned()))),
            false
        ),
        Told::Present(None)
    );
}

#[test]
fn a_logo_that_the_server_book_names_is_read_once() {
    let mut scene = scene();
    scene.want(NAMED);
    assert!(scene.logos.exists(scene.server));
    assert_eq!(scene.image(), None);
    let ask = scene.next(false).unwrap();
    assert_eq!(ask.server_id, scene.server);
    let read = logo(1);
    scene.logos.answered(&ask, Answer::Read(Some(read.clone())));
    assert_eq!(scene.image(), Some(read.image));
    // The same statement again, and a later change of the state, read nothing.
    scene.want(NAMED);
    assert_eq!(scene.next(true), None);
    assert!(scene.logos.take_dropped().is_empty());
}

#[test]
fn a_server_without_a_named_logo_has_no_logo_and_nothing_is_read() {
    let mut scene = scene();
    scene.want(NOT_NAMED);
    assert_eq!(scene.next(true), None);
    assert_eq!(scene.image(), None);
    assert!(!scene.logos.exists(scene.server));
    assert_eq!(scene.logos.shown(), ShownLogos::default());

    // The server book names no path any more: the server had no logo at the last
    // contact, and the image that was read before is dropped.
    let held = logo(1);
    let mut scene = Scene::holding(&held);
    scene.want(NOT_NAMED);
    assert_eq!(scene.image(), None);
    assert_eq!(scene.next(true), None);
    assert_eq!(scene.logos.take_dropped(), [held.image]);
}

#[test]
fn an_event_that_states_no_logo_drops_the_held_image_and_reads_nothing() {
    let held = logo(1);
    let mut scene = Scene::holding(&held);
    scene.want(Told::Absent);
    assert_eq!(scene.image(), None);
    assert!(!scene.logos.exists(scene.server));
    assert_eq!(scene.next(true), None);
    assert_eq!(scene.logos.take_dropped(), [held.image]);
    // Each image that left the cache is given once.
    assert!(scene.logos.take_dropped().is_empty());
}

#[test]
fn an_event_that_states_the_hash_of_the_held_image_reads_nothing() {
    let held = logo(1);
    let mut scene = Scene::holding(&held);
    scene.want(stated(held.hash));
    assert_eq!(scene.next(true), None);
    assert_eq!(scene.image(), Some(held.image));
    assert!(scene.logos.take_dropped().is_empty());
}

#[test]
fn an_event_that_states_another_hash_reads_the_image_and_the_held_one_stays_until_then() {
    let (held, new) = (logo(1), logo(2));
    assert_ne!(held.hash, new.hash);
    let mut scene = Scene::holding(&held);
    scene.want(stated(new.hash));
    let ask = scene.next(false).unwrap();
    assert_eq!(scene.image(), Some(held.image.clone()));
    assert!(scene.logos.take_dropped().is_empty());
    scene.logos.answered(&ask, Answer::Read(Some(new.clone())));
    assert_eq!(scene.image(), Some(new.image));
    assert_eq!(scene.logos.take_dropped(), [held.image]);
    assert_eq!(scene.next(true), None);
}

#[test]
fn an_event_that_states_a_logo_reads_the_image_when_none_is_held() {
    let mut scene = scene();
    scene.want(NOT_NAMED);
    let new = logo(1);
    scene.want(stated(new.hash));
    assert!(scene.logos.exists(scene.server));
    scene.read(Some(new.clone()));
    assert_eq!(scene.image(), Some(new.image));
    assert_eq!(scene.next(true), None);

    // A hash in another form than the one of a server: the image is read once.
    let mut scene = scene_with(Told::Present(None));
    let unknown = logo(3);
    scene.read(Some(unknown.clone()));
    assert_eq!(scene.image(), Some(unknown.image));
    scene.want(Told::Present(None));
    assert_eq!(scene.next(true), None);
}

fn scene_with(told: Told) -> Scene {
    let mut scene = scene();
    scene.want(told);
    scene
}

#[test]
fn after_a_reload_a_logo_that_is_not_stated_is_read_again_once() {
    let held = logo(1);
    let mut scene = Scene::holding(&held);
    scene.logos.stale(scene.server);
    // The held image is shown until the answer arrives.
    assert_eq!(scene.image(), Some(held.image.clone()));
    let ask = scene.next(true).unwrap();
    assert_eq!(scene.next(true), None);
    // The route gave the same bytes: the held image stays the one of the window.
    let same = Logo {
        hash: held.hash,
        image: logo(1).image,
    };
    scene.logos.answered(&ask, Answer::Read(Some(same)));
    assert_eq!(scene.image(), Some(held.image.clone()));
    assert!(scene.logos.take_dropped().is_empty());
    assert_eq!(scene.next(true), None);

    // A server whose server book names no logo is asked too: an administrator can have
    // set one meanwhile.
    let mut scene = scene_with(NOT_NAMED);
    scene.logos.stale(scene.server);
    let new = logo(2);
    scene.read(Some(new.clone()));
    assert_eq!(scene.image(), Some(new.image));
    assert!(scene.logos.exists(scene.server));

    // A logo that an event stated is the truth of after the reload.
    let mut scene = Scene::holding(&held);
    scene.want(stated(held.hash));
    scene.logos.stale(scene.server);
    assert_eq!(scene.next(true), None);
}

#[test]
fn a_state_that_is_loaded_again_after_an_event_reads_the_logo_without_a_named_path() {
    // The server book names no logo; an administrator set one, and the event stated it.
    let mut scene = scene_with(NOT_NAMED);
    let set = logo(1);
    scene.want(stated(set.hash));
    scene.read(Some(set.clone()));
    // The state is replaced: no event of the new state stated the logo, and the server
    // book still names what the server had at the first contact.
    scene.want(NOT_NAMED);
    assert_eq!(scene.image(), Some(set.image.clone()));
    assert!(scene.logos.exists(scene.server));
    let ask = scene.next(false).unwrap();
    // The logo was removed while the events were not delivered.
    scene.logos.answered(&ask, Answer::Read(None));
    assert_eq!(scene.image(), None);
    assert!(!scene.logos.exists(scene.server));
    assert_eq!(scene.logos.take_dropped(), [set.image]);
}

#[test]
fn a_read_that_finds_no_logo_drops_the_held_image() {
    let held = logo(1);
    let mut scene = Scene::holding(&held);
    scene.logos.stale(scene.server);
    scene.read(None);
    assert_eq!(scene.image(), None);
    // The server book still names the path of before; the read is newer.
    assert!(!scene.logos.exists(scene.server));
    assert_eq!(scene.logos.take_dropped(), [held.image]);
    assert_eq!(scene.next(true), None);
}

#[test]
fn one_request_of_a_server_is_on_its_way_at_a_time() {
    let mut scene = scene_with(NAMED);
    let first = scene.next(true).unwrap();
    assert!(scene.logos.on_way(scene.server));
    assert_eq!(scene.next(true), None);
    // An event states a logo while the request is on its way: the new read waits for
    // the end of that request.
    let new = logo(1);
    scene.want(stated(new.hash));
    assert_eq!(scene.next(true), None);
    scene.logos.answered(&first, Answer::None);
    assert!(!scene.logos.on_way(scene.server));
    let second = scene.next(false).unwrap();
    assert_ne!(first, second);
    assert_eq!(scene.next(true), None);

    // Another server has its own request.
    let other = ServerId::generate();
    scene.logos.want(other, NAMED);
    assert!(scene.logos.next(other, false).is_some());
}

#[test]
fn a_read_without_an_answer_is_sent_three_times_and_the_held_image_stays() {
    let errors: [fn() -> ApiError; 2] = [unreachable, || refused(ErrorCode::RateLimited, 429)];
    for error in errors {
        let held = logo(1);
        let mut scene = Scene::holding(&held);
        scene.logos.stale(scene.server);
        let ask = scene.next(false).unwrap();
        scene.logos.answered(&ask, logo_of(Err(error())));
        // The answer of a request starts no request for a logo that got no answer: that
        // one waits for a change of the state.
        assert_eq!(scene.next(false), None);
        for _ in 1..TRIES {
            let ask = scene.next(true).unwrap();
            scene.logos.answered(&ask, logo_of(Err(error())));
        }
        assert_eq!(scene.next(true), None);
        assert_eq!(scene.image(), Some(held.image));
        assert!(scene.logos.take_dropped().is_empty());
    }
}

#[test]
fn a_final_failure_keeps_the_held_image_and_is_not_asked_again() {
    let held = logo(1);
    let no_image = AvatarImage {
        media_type: ImageType::Png,
        hash: profile::avatar_hash(b"\x89PNG\r\n\x1a\nnot an image"),
        bytes: b"\x89PNG\r\n\x1a\nnot an image".to_vec(),
    };
    for answer in [
        logo_of(Err(refused(ErrorCode::Forbidden, 403))),
        logo_of(Ok(Some(no_image))),
    ] {
        assert_eq!(answer, Answer::Final);
        let mut scene = Scene::holding(&held);
        scene.want(stated(PictureHash::of(b"another logo")));
        let ask = scene.next(false).unwrap();
        scene.logos.answered(&ask, answer);
        assert_eq!(scene.next(true), None);
        assert_eq!(scene.image(), Some(held.image.clone()));
        assert!(scene.logos.take_dropped().is_empty());
    }
}

#[test]
fn an_answer_is_taken_only_for_the_newest_request_of_its_server() {
    let (early, late) = (logo(1), logo(2));
    let mut scene = scene_with(NAMED);
    let first = scene.next(false).unwrap();
    // The state is loaded again while the request is on its way: its answer can be older
    // than the reload, and is not used.
    scene.logos.stale(scene.server);
    scene
        .logos
        .answered(&first, Answer::Read(Some(early.clone())));
    assert_eq!(scene.image(), None);
    let second = scene.next(false).unwrap();
    // An answer that arrives twice, and the answer to a request of before, change nothing.
    scene.logos.answered(&first, Answer::Read(Some(early)));
    assert_eq!(scene.image(), None);
    assert!(scene.logos.on_way(scene.server));
    scene
        .logos
        .answered(&second, Answer::Read(Some(late.clone())));
    assert_eq!(scene.image(), Some(late.image.clone()));

    // The cache of a server that was removed and added again did not ask.
    let mut scene = scene_with(NAMED);
    let before = scene.next(false).unwrap();
    scene.logos.retain(&HashSet::new());
    scene.want(NAMED);
    scene.logos.answered(&before, Answer::Read(Some(late)));
    assert_eq!(scene.image(), None);
    assert!(scene.next(false).is_some());
}

#[test]
fn a_server_that_left_the_client_is_dropped_with_its_image() {
    let (held, kept) = (logo(1), logo(2));
    let mut scene = Scene::holding(&held);
    let other = ServerId::generate();
    scene.logos.hold(other, kept.clone());
    scene.logos.retain(&HashSet::from([other]));
    assert_eq!(scene.image(), None);
    assert!(!scene.logos.exists(scene.server));
    assert_eq!(scene.logos.image(other), Some(kept.image));
    assert_eq!(scene.logos.take_dropped(), [held.image]);
    assert_eq!(scene.next(true), None);
}

#[test]
fn the_logo_that_was_read_has_the_hash_that_this_device_computed() {
    let bytes = png(8, 7);
    let read = AvatarImage {
        media_type: ImageType::Png,
        hash: profile::avatar_hash(&bytes),
        bytes: bytes.clone(),
    };
    let Answer::Read(Some(logo)) = logo_of(Ok(Some(read))) else {
        panic!("the image of the route is a logo");
    };
    assert_eq!(logo.hash, PictureHash::of(&bytes));
    // A server without a logo.
    assert_eq!(logo_of(Ok(None)), Answer::Read(None));

    // A large image is reduced as an avatar is, and an image that is larger than a
    // server stores is not decoded.
    let wide = png(400, 7);
    let read = AvatarImage {
        media_type: ImageType::Png,
        hash: profile::avatar_hash(&wide),
        bytes: wide,
    };
    let Answer::Read(Some(reduced)) = logo_of(Ok(Some(read))) else {
        panic!("an image of 400 pixels is a logo");
    };
    let size = reduced.image.size(0);
    assert_eq!(
        (size.width.0, size.height.0),
        (PICTURE_SIDE as i32, PICTURE_SIDE as i32)
    );
    let huge = png(AVATAR_SIDE + 1, 7);
    let read = AvatarImage {
        media_type: ImageType::Png,
        hash: profile::avatar_hash(&huge),
        bytes: huge,
    };
    assert_eq!(logo_of(Ok(Some(read))), Answer::Final);
}

#[test]
fn the_row_of_a_server_carries_the_image_of_its_logo() {
    let (with, without, reading) = (
        ServerId::generate(),
        ServerId::generate(),
        ServerId::generate(),
    );
    let held = logo(1);
    let mut logos = Logos::default();
    logos.hold(with, held.clone());
    logos.want(without, NOT_NAMED);
    logos.want(reading, NAMED);
    let shown = logos.shown();
    assert!(shown.of(with).exists && shown.of(reading).exists);
    assert!(!shown.of(without).exists);
    let known = Known {
        logos: shown,
        ..Known::default()
    };
    let view = derive(
        &[server(with), server(without), server(reading)],
        &Selection::default(),
        &known,
    );
    // Without the image the window draws the generated picture of the server.
    assert_eq!(view.servers[0].logo, Some(held.image));
    assert_eq!(view.servers[1].logo, None);
    assert_eq!(view.servers[2].logo, None);
}
