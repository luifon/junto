use junto_client_core::api::{Rejection, TransportError, TransportErrorKind};
use junto_client_core::protocol::account::{Avatar, ImageType};
use junto_client_core::protocol::error::ErrorCode;

use super::*;

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

fn image() -> Arc<RenderImage> {
    decode(&png(4, 1), AVATAR_SIDE).unwrap()
}

/// A hash that is the same for the same number.
fn hash(number: u32) -> PictureHash {
    PictureHash::of(&number.to_be_bytes())
}

fn profile(id: AccountId, name: &str, avatar: Option<PictureHash>) -> AccountProfile {
    AccountProfile {
        id,
        name: name.to_owned(),
        avatar: avatar.map(|hash| Avatar {
            hash: hash.to_string(),
            media_type: ImageType::Png,
            bytes: 100,
            width: 4,
            height: 4,
        }),
    }
}

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

/// A server of the cache with the account of its session, and people who are shown.
struct Scene {
    people: People,
    server: ServerId,
    own: AccountId,
}

fn scene() -> Scene {
    let (server, own) = (ServerId::generate(), AccountId::generate());
    let mut people = People::default();
    people.belongs(server, Some(own));
    Scene {
        people,
        server,
        own,
    }
}

fn accounts(count: usize) -> Vec<AccountId> {
    (0..count).map(|_| AccountId::generate()).collect()
}

impl Scene {
    fn show(&mut self, accounts: &[AccountId]) {
        self.people.want(
            self.server,
            &Wanted {
                own: Some((self.own, None)),
                accounts: accounts.to_vec(),
            },
        );
    }

    fn next(&mut self, again: bool) -> Vec<Ask> {
        self.people.next(self.server, again)
    }

    /// Answers every request that starts, until none starts: each account gets a profile
    /// with the avatar that `avatar` names, and a request for an image gets no answer.
    fn read_profiles(&mut self, avatar: impl Fn(AccountId) -> Option<PictureHash>) -> usize {
        let mut read = 0;
        loop {
            let asks = self.next(false);
            if asks.is_empty() {
                return read;
            }
            for ask in asks {
                match ask.what {
                    Asked::Profile(account_id) => {
                        self.people.profile_answered(
                            &ask,
                            Answer::Read(profile(account_id, "Ana", avatar(account_id))),
                        );
                        read += 1;
                    }
                    Asked::Picture { .. } => self.people.picture_answered(&ask, Answer::None),
                }
            }
        }
    }
}

fn profile_asks(asks: &[Ask]) -> Vec<AccountId> {
    asks.iter()
        .filter_map(|ask| match ask.what {
            Asked::Profile(account_id) => Some(account_id),
            Asked::Picture { .. } => None,
        })
        .collect()
}

#[test]
fn a_hash_is_64_hexadecimal_digits_in_lower_case() {
    let written = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let hash = PictureHash::parse(written).unwrap();
    assert_eq!(hash.to_string(), written);
    assert_eq!(PictureHash::of(b"abc"), hash);
    assert_eq!(PictureHash::parse(&written.to_uppercase()), None);
    assert_eq!(PictureHash::parse(&written[..63]), None);
    assert_eq!(PictureHash::parse(&format!("{written}0")), None);
    assert_eq!(PictureHash::parse(&written.replace('b', "g")), None);
    assert_eq!(PictureHash::parse(&"é".repeat(32)), None);
}

#[test]
fn at_most_four_requests_of_one_server_are_on_their_way() {
    let mut scene = scene();
    let shown = accounts(10);
    scene.show(&shown);
    let first = scene.next(true);
    // The most visible people are asked for first.
    assert_eq!(profile_asks(&first), shown[..MOST_REQUESTS]);
    assert!(scene.next(true).is_empty());
    // Each answer frees one place.
    scene
        .people
        .profile_answered(&first[0], Answer::Read(profile(shown[0], "Ana", None)));
    assert_eq!(profile_asks(&scene.next(false)), [shown[4]]);
    assert!(scene.next(false).is_empty());
    assert_eq!(
        scene.people.name(scene.server, shown[0]).as_deref(),
        Some("Ana")
    );
    // Another server has its own four.
    let (other, account) = (ServerId::generate(), AccountId::generate());
    scene.people.belongs(other, Some(account));
    scene.people.want(
        other,
        &Wanted {
            own: None,
            accounts: accounts(6),
        },
    );
    assert_eq!(scene.people.next(other, true).len(), MOST_REQUESTS);
}

#[test]
fn a_refusal_is_final_and_a_request_without_an_answer_is_sent_three_times() {
    let mut scene = scene();
    let shown = accounts(2);
    let (silent, hidden) = (shown[0], shown[1]);
    scene.show(&shown);
    let asks = scene.next(true);
    assert_eq!(profile_asks(&asks), [silent, hidden]);
    scene
        .people
        .profile_answered(&asks[0], Answer::of(Err(unreachable())));
    scene
        .people
        .profile_answered(&asks[1], Answer::of(Err(refused(ErrorCode::NotFound, 404))));
    // The answer of a request starts the next requests, and none for an account that got
    // no answer: that one waits for a change of the state.
    assert!(scene.next(false).is_empty());
    for _ in 1..TRIES {
        let asks = scene.next(true);
        assert_eq!(profile_asks(&asks), [silent]);
        scene
            .people
            .profile_answered(&asks[0], Answer::of(Err(unreachable())));
    }
    assert!(scene.next(true).is_empty());
    assert_eq!(scene.people.name(scene.server, silent), None);
    assert_eq!(scene.people.name(scene.server, hidden), None);
}

#[test]
fn a_limit_of_the_server_counts_as_no_answer() {
    assert_eq!(
        Answer::<()>::of(Err(refused(ErrorCode::RateLimited, 429))),
        Answer::None
    );
    assert_eq!(
        Answer::<()>::of(Err(refused(ErrorCode::Busy, 503))),
        Answer::None
    );
    assert_eq!(
        Answer::<()>::of(Err(refused(ErrorCode::Forbidden, 403))),
        Answer::Final
    );
    assert_eq!(Answer::of(Ok(7)), Answer::Read(7));
}

#[test]
fn a_server_holds_at_most_512_profiles_and_the_one_shown_longest_ago_gives_way() {
    let mut scene = scene();
    let many = accounts(MOST_PROFILES + 40);
    scene.show(&many);
    // Only the first 512 of the people who are shown at one time are read.
    assert_eq!(scene.read_profiles(|_| None), MOST_PROFILES);
    assert!(
        scene
            .people
            .name(scene.server, many[MOST_PROFILES - 1])
            .is_some()
    );
    assert!(
        scene
            .people
            .name(scene.server, many[MOST_PROFILES])
            .is_none()
    );
    assert!(scene.next(true).is_empty());

    // Other people are shown: the profiles that are shown no longer give way, the oldest
    // first, and a profile that is still shown stays.
    let (kept, new) = (many[0], accounts(3));
    scene.show(&[kept]);
    scene.show(&[kept, new[0], new[1], new[2]]);
    assert_eq!(scene.read_profiles(|_| None), 3);
    assert!(scene.people.name(scene.server, kept).is_some());
    let held = many[..MOST_PROFILES]
        .iter()
        .filter(|account_id| scene.people.name(scene.server, **account_id).is_some())
        .count();
    assert_eq!(held, MOST_PROFILES - 3);
}

#[test]
fn the_image_of_an_avatar_is_read_once_for_its_hash() {
    let mut scene = scene();
    let shown = accounts(2);
    scene.show(&shown);
    let picture = hash(1);
    let asks = scene.next(true);
    for ask in &asks {
        let Asked::Profile(account_id) = ask.what else {
            panic!("{ask:?}");
        };
        scene
            .people
            .profile_answered(ask, Answer::Read(profile(account_id, "Ana", Some(picture))));
    }
    // Two accounts have the same image: it is asked for once, from one of them.
    let asks = scene.next(false);
    assert_eq!(
        asks.iter().map(|ask| ask.what).collect::<Vec<_>>(),
        [Asked::Picture {
            account_id: shown[0],
            hash: picture
        }]
    );
    let faces = scene.people.faces(scene.server);
    assert_eq!(faces[&shown[0]].hash, picture);
    assert_eq!(faces[&shown[0]].image, None);

    let image = image();
    scene
        .people
        .picture_answered(&asks[0], Answer::Read(image.clone()));
    let faces = scene.people.faces(scene.server);
    assert_eq!(faces[&shown[0]].image, Some(image.clone()));
    assert_eq!(faces[&shown[1]].image, Some(image));
    assert!(scene.next(true).is_empty());
}

#[test]
fn an_image_with_another_hash_or_that_is_no_image_is_not_asked_for_again() {
    let bytes = png(8, 3);
    let read = |bytes: &[u8]| {
        Ok(AvatarImage {
            media_type: ImageType::Png,
            hash: profile::avatar_hash(bytes),
            bytes: bytes.to_vec(),
        })
    };
    // The bytes of the avatar route are the image that the profile names.
    assert!(matches!(
        picture_of(PictureHash::of(&bytes), read(&bytes)),
        Answer::Read(_)
    ));
    // Another image than the profile names.
    assert_eq!(picture_of(hash(9), read(&bytes)), Answer::Final);
    // Bytes that start as a PNG file and are none.
    let broken = b"\x89PNG\r\n\x1a\n-not-an-image-";
    assert_eq!(
        picture_of(PictureHash::of(broken), read(broken)),
        Answer::Final
    );
    // An image that is larger than a server stores, whatever the server states.
    let large = png(AVATAR_SIDE + 1, 3);
    assert_eq!(
        picture_of(PictureHash::of(&large), read(&large)),
        Answer::Final
    );
    assert_eq!(picture_of(hash(9), Err(unreachable())), Answer::None);

    let mut scene = scene();
    let shown = accounts(1);
    scene.show(&shown);
    let asks = scene.next(true);
    scene.people.profile_answered(
        &asks[0],
        Answer::Read(profile(shown[0], "Ana", Some(hash(9)))),
    );
    let asks = scene.next(false);
    assert!(matches!(asks[0].what, Asked::Picture { .. }));
    scene.people.picture_answered(&asks[0], Answer::Final);
    // Every later change of the state asks for nothing: the hash names the same bytes.
    for _ in 0..5 {
        scene.show(&shown);
        assert!(scene.next(true).is_empty());
    }
    scene.people.stale(scene.server);
    scene.show(&shown);
    assert_eq!(profile_asks(&scene.next(true)), shown);
    assert_eq!(scene.people.faces(scene.server)[&shown[0]].image, None);
}

#[test]
fn a_decoded_image_is_a_square_of_at_most_192_pixels() {
    let side = |image: &Arc<RenderImage>| {
        let size = image.size(0);
        (size.width.0, size.height.0)
    };
    let small = decode(&png(40, 1), AVATAR_SIDE).unwrap();
    assert_eq!(side(&small), (40, 40));
    let large = decode(&png(600, 1), AVATAR_SIDE).unwrap();
    assert_eq!(side(&large), (PICTURE_SIDE as i32, PICTURE_SIDE as i32));
    // A wide image gives its middle.
    let wide = image::RgbaImage::from_fn(30, 10, |x, _| {
        image::Rgba([if (10..20).contains(&x) { 255 } else { 0 }, 0, 0, 255])
    });
    let mut bytes = Vec::new();
    wide.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    let middle = decode(&bytes, AVATAR_SIDE).unwrap();
    assert_eq!(side(&middle), (10, 10));
    // Red is the third byte of a pixel of the window.
    assert!(
        middle
            .as_bytes(0)
            .unwrap()
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 255, 255])
    );
    assert!(decode(b"", AVATAR_SIDE).is_none());
    assert!(decode(b"<svg></svg>", AVATAR_SIDE).is_none());
}

/// While the administration is open, the cache reads the people of the call first, then the
/// laid-out rows of its page that the page still lists, and then the people of the view;
/// with the layer closed, the people of the view take the places of the rows again
/// (ADR 0783).
#[test]
fn the_rows_of_the_open_administration_come_after_the_call_and_before_the_view() {
    let mut scene = scene();
    let (call, rows, members) = (accounts(2), accounts(40), accounts(601));
    let gone = AccountId::generate();
    let followed = Followed {
        call: HashMap::from([(
            scene.server,
            Wanted {
                own: None,
                accounts: call.clone(),
            },
        )]),
        view: HashMap::from([(
            scene.server,
            Wanted {
                own: Some((scene.own, None)),
                accounts: members.clone(),
            },
        )]),
        admin_rows: rows.iter().copied().chain([gone]).collect(),
    };
    let listed: HashSet<AccountId> = rows.iter().copied().collect();

    scene
        .people
        .want_all(&followed_people(&followed, Some((scene.server, &listed))));
    let first: Vec<AccountId> = call.iter().chain(&rows).copied().collect();
    let expected: Vec<AccountId> = first
        .iter()
        .chain(&members)
        .copied()
        .take(MOST_PROFILES)
        .collect();
    assert_eq!(scene.people.wanted(scene.server), expected);
    let asks = scene.next(false);
    assert_eq!(profile_asks(&asks), first[..MOST_REQUESTS]);
    for ask in &asks {
        if let Asked::Profile(account_id) = ask.what {
            let answer = Answer::Read(profile(account_id, "Ana", None));
            scene.people.profile_answered(ask, answer);
        }
    }
    assert_eq!(scene.read_profiles(|_| None), MOST_PROFILES - MOST_REQUESTS);
    assert!(scene.people.name(scene.server, gone).is_none());

    scene.people.want_all(&followed_people(&followed, None));
    let expected: Vec<AccountId> = call
        .iter()
        .chain(&members)
        .copied()
        .take(MOST_PROFILES)
        .collect();
    assert_eq!(scene.people.wanted(scene.server), expected);
    assert_eq!(
        scene.read_profiles(|_| None),
        rows.len(),
        "the members after the rows are read in the places of the rows"
    );
}

/// People who come before shown people in the list take only the places of the profiles
/// that are no longer shown: a profile that is shown later in the same list keeps its value
/// and is not read again (ADR 0783).
#[test]
fn new_people_before_shown_people_take_only_the_places_of_profiles_no_longer_shown() {
    let mut scene = scene();
    let held = accounts(MOST_PROFILES);
    scene.show(&held);
    assert_eq!(scene.read_profiles(|_| None), MOST_PROFILES);
    let new = accounts(40);
    let still = &held[..MOST_PROFILES - new.len()];
    let shown: Vec<AccountId> = new.iter().chain(still).copied().collect();
    scene.show(&shown);
    let kept = still
        .iter()
        .filter(|account_id| scene.people.name(scene.server, **account_id).is_some())
        .count();
    assert_eq!(kept, still.len());
    assert_eq!(scene.read_profiles(|_| None), new.len());
}

/// As for the profiles: the images of shown people that come later in the list keep their
/// places when the images of people before them need places.
#[test]
fn new_images_before_shown_images_take_only_the_places_of_images_no_longer_shown() {
    let mut scene = scene();
    let (old, new) = (accounts(MOST_IMAGES), accounts(10));
    let all: Vec<AccountId> = old.iter().chain(&new).copied().collect();
    let number = |account_id: AccountId| {
        all.iter()
            .position(|held| *held == account_id)
            .map(|index| hash(index as u32))
    };
    // Every account has a profile; the images of the first 256 accounts are read, and the
    // others have no place.
    scene.show(&all);
    scene.read_profiles(number);
    loop {
        let asks = scene.next(true);
        if asks.is_empty() {
            break;
        }
        for ask in asks {
            scene.people.picture_answered(&ask, Answer::Read(image()));
        }
    }
    let still = &old[..MOST_IMAGES - new.len()];
    let shown: Vec<AccountId> = new.iter().chain(still).copied().collect();
    scene.show(&shown);
    let faces = scene.people.faces(scene.server);
    let kept = still
        .iter()
        .filter(|account_id| {
            faces
                .get(account_id)
                .is_some_and(|face| face.image.is_some())
        })
        .count();
    assert_eq!(kept, still.len());
    assert_eq!(scene.people.take_dropped().len(), new.len());
}

#[test]
fn a_server_holds_at_most_256_images_and_the_one_shown_longest_ago_gives_way() {
    let mut scene = scene();
    let shown = accounts(MOST_IMAGES + 10);
    let number = |account_id: AccountId| {
        shown
            .iter()
            .position(|held| *held == account_id)
            .map(|index| hash(index as u32))
    };
    scene.show(&shown);
    scene.read_profiles(number);
    // Every account has another image. The first 256 of them are read.
    let mut read = 0;
    loop {
        let asks = scene.next(true);
        if asks.is_empty() {
            break;
        }
        for ask in asks {
            scene.people.picture_answered(&ask, Answer::Read(image()));
            read += 1;
        }
    }
    assert_eq!(read, MOST_IMAGES);
    let with_image = |scene: &Scene| {
        scene
            .people
            .faces(scene.server)
            .values()
            .filter(|face| face.image.is_some())
            .count()
    };
    assert_eq!(with_image(&scene), MOST_IMAGES);
    assert!(scene.people.take_dropped().is_empty());

    // The people after the first 256 are shown alone: their images take the places of
    // images that are not shown, and those are given back to be dropped from the window.
    let late = &shown[MOST_IMAGES..];
    scene.show(late);
    let asks = scene.next(true);
    assert_eq!(asks.len(), MOST_REQUESTS);
    assert_eq!(scene.people.take_dropped().len(), late.len());
    assert_eq!(with_image(&scene), MOST_IMAGES - late.len());
}

#[test]
fn a_profile_event_replaces_the_profile_and_its_image_is_asked_for() {
    let mut scene = scene();
    let shown = accounts(1);
    let ana = shown[0];
    scene.show(&shown);
    let asks = scene.next(true);
    scene
        .people
        .profile_answered(&asks[0], Answer::Read(profile(ana, "Ana", None)));
    assert!(scene.people.faces(scene.server).is_empty());

    // The account sets an avatar and another name.
    assert!(
        scene
            .people
            .profile_updated(scene.server, &profile(ana, "Ana Lima", Some(hash(1))))
    );
    assert_eq!(
        scene.people.name(scene.server, ana).as_deref(),
        Some("Ana Lima")
    );
    let asks = scene.next(false);
    assert_eq!(
        asks[0].what,
        Asked::Picture {
            account_id: ana,
            hash: hash(1)
        }
    );
    scene
        .people
        .picture_answered(&asks[0], Answer::Read(image()));
    assert!(scene.people.faces(scene.server)[&ana].image.is_some());

    // The account removes the avatar: the generated avatar returns.
    assert!(
        scene
            .people
            .profile_updated(scene.server, &profile(ana, "Ana Lima", None))
    );
    assert!(scene.people.faces(scene.server).is_empty());
    assert!(scene.next(true).is_empty());

    // An event about an account that the cache does not hold changes nothing.
    let other = AccountId::generate();
    assert!(
        !scene
            .people
            .profile_updated(scene.server, &profile(other, "Zoe", None))
    );
    assert_eq!(scene.people.name(scene.server, other), None);
}

#[test]
fn an_event_is_later_than_the_answer_of_a_request_that_was_on_its_way() {
    let mut scene = scene();
    let shown = accounts(1);
    scene.show(&shown);
    let asks = scene.next(true);
    scene
        .people
        .profile_updated(scene.server, &profile(shown[0], "New name", None));
    scene
        .people
        .profile_answered(&asks[0], Answer::Read(profile(shown[0], "Old name", None)));
    assert_eq!(
        scene.people.name(scene.server, shown[0]).as_deref(),
        Some("New name")
    );
    // The request is no longer on its way: four more can start.
    scene.show(&accounts(6));
    assert_eq!(scene.next(true).len(), MOST_REQUESTS);
}

#[test]
fn another_account_of_the_session_and_a_removed_server_drop_what_was_held() {
    let mut scene = scene();
    let shown = accounts(1);
    scene.show(&shown);
    scene.read_profiles(|_| Some(hash(1)));
    let asks = scene.next(true);
    scene
        .people
        .picture_answered(&asks[0], Answer::Read(image()));
    let late = {
        scene.show(&accounts(1));
        scene.next(true)
    };
    assert_eq!(late.len(), 1);
    assert!(scene.people.take_dropped().is_empty());

    // The same account again, and no account for a moment: nothing is dropped.
    scene.people.belongs(scene.server, Some(scene.own));
    scene.people.belongs(scene.server, None);
    assert!(scene.people.name(scene.server, shown[0]).is_some());

    // Another account: the names and the images of the account before are gone.
    let other = AccountId::generate();
    scene.people.belongs(scene.server, Some(other));
    assert_eq!(scene.people.name(scene.server, shown[0]), None);
    assert!(scene.people.faces(scene.server).is_empty());
    assert_eq!(scene.people.take_dropped().len(), 1);

    // An answer to a request of the account before is not used and frees no place.
    let again = accounts(MOST_REQUESTS);
    scene.people.want(
        scene.server,
        &Wanted {
            own: Some((other, None)),
            accounts: again.clone(),
        },
    );
    assert_eq!(scene.next(true).len(), MOST_REQUESTS);
    let Asked::Profile(asked) = late[0].what else {
        panic!("{late:?}");
    };
    scene
        .people
        .profile_answered(&late[0], Answer::Read(profile(asked, "Ana", None)));
    assert_eq!(scene.people.name(scene.server, asked), None);
    assert!(scene.next(true).is_empty());

    // The server is removed from the book.
    let asks = scene.next(false);
    assert!(asks.is_empty());
    scene.people.retain(&HashSet::new());
    assert!(scene.people.names(scene.server).next().is_none());
    scene.people.want(scene.server, &Wanted::default());
    assert!(scene.next(true).is_empty());
}

#[test]
fn the_own_account_is_not_read_and_its_image_is_the_one_of_the_state_of_the_server() {
    let mut scene = scene();
    let (server, own) = (scene.server, scene.own);
    scene.people.want(
        server,
        &Wanted {
            own: Some((own, Some(hash(1)))),
            // The own account among the shown people asks for no profile.
            accounts: vec![own],
        },
    );
    let asks = scene.next(true);
    assert_eq!(
        asks.iter().map(|ask| ask.what).collect::<Vec<_>>(),
        [Asked::Picture {
            account_id: own,
            hash: hash(1)
        }]
    );
    let image = image();
    scene
        .people
        .picture_answered(&asks[0], Answer::Read(image.clone()));
    assert_eq!(scene.people.image(server, hash(1)), Some(image));
    assert!(scene.next(true).is_empty());
}

#[test]
fn a_replaced_state_reads_each_shown_profile_again_and_shows_the_old_one_meanwhile() {
    let mut scene = scene();
    let shown = accounts(2);
    scene.show(&shown);
    scene.read_profiles(|_| None);
    assert!(scene.next(true).is_empty());

    scene.people.stale(scene.server);
    assert_eq!(
        scene.people.name(scene.server, shown[0]).as_deref(),
        Some("Ana")
    );
    let asks = scene.next(false);
    assert_eq!(profile_asks(&asks), shown);
    scene
        .people
        .profile_answered(&asks[0], Answer::Read(profile(shown[0], "Ana Lima", None)));
    // The account can no longer see the other profile.
    scene.people.profile_answered(&asks[1], Answer::Final);
    assert_eq!(
        scene.people.name(scene.server, shown[0]).as_deref(),
        Some("Ana Lima")
    );
    assert_eq!(scene.people.name(scene.server, shown[1]), None);
}

#[test]
fn the_name_and_the_hash_of_a_profile_are_text_of_another_party() {
    let mut scene = scene();
    let shown = accounts(3);
    scene.show(&shown);
    let asks = scene.next(true);
    let mut long = profile(shown[0], &format!("A\nna\u{7}{}", "é".repeat(200)), None);
    long.avatar = Some(Avatar {
        hash: "../../etc/passwd".repeat(1000),
        media_type: ImageType::Png,
        bytes: 1,
        width: 1,
        height: 1,
    });
    scene.people.profile_answered(&asks[0], Answer::Read(long));
    scene
        .people
        .profile_answered(&asks[1], Answer::Read(profile(shown[1], "\n \t", None)));
    // An answer about another account than the one that was asked for is no answer.
    scene.people.profile_answered(
        &asks[2],
        Answer::Read(profile(AccountId::generate(), "Mallory", None)),
    );
    let name = scene.people.name(scene.server, shown[0]).unwrap();
    assert_eq!(name.chars().count(), 64);
    assert!(name.starts_with("Anaé"));
    assert!(scene.people.faces(scene.server).is_empty());
    assert_eq!(scene.people.name(scene.server, shown[1]), None);
    assert_eq!(scene.people.name(scene.server, shown[2]), None);
    assert!(scene.next(true).is_empty());
}

#[test]
fn an_answer_for_a_server_that_was_removed_and_added_again_is_not_used() {
    let mut scene = scene();
    let shown = accounts(1);
    scene.show(&shown);
    let late = scene.next(true);
    assert_eq!(profile_asks(&late), shown);

    // The server leaves the server book and is added again, and another account joins on
    // this device. Its cache is a new one.
    scene.people.retain(&HashSet::new());
    let other = AccountId::generate();
    scene.people.belongs(scene.server, Some(other));
    let again: Vec<AccountId> = shown
        .iter()
        .copied()
        .chain(accounts(MOST_REQUESTS))
        .collect();
    scene.people.want(
        scene.server,
        &Wanted {
            own: Some((other, None)),
            accounts: again,
        },
    );
    assert_eq!(scene.next(true).len(), MOST_REQUESTS);

    // The answer to the request of the account before arrives now. It is no profile of
    // this cache, and it frees no place of its requests.
    scene
        .people
        .profile_answered(&late[0], Answer::Read(profile(shown[0], "Ana", None)));
    assert_eq!(scene.people.name(scene.server, shown[0]), None);
    assert!(scene.next(true).is_empty());
}

#[test]
fn an_answer_that_was_asked_for_before_the_state_was_replaced_is_not_used() {
    let mut scene = scene();
    let shown = accounts(1);
    scene.show(&shown);
    let before = scene.next(true);
    assert_eq!(profile_asks(&before), shown);

    // The state is replaced while the request is on its way: the profile is read again.
    scene.people.stale(scene.server);
    let after = scene.next(false);
    assert_eq!(profile_asks(&after), shown);
    // The answer to the first request can be older than the new state: it is not used.
    scene.people.profile_answered(
        &before[0],
        Answer::Read(profile(shown[0], "Old name", None)),
    );
    assert_eq!(scene.people.name(scene.server, shown[0]), None);
    scene
        .people
        .profile_answered(&after[0], Answer::Read(profile(shown[0], "New name", None)));
    assert_eq!(
        scene.people.name(scene.server, shown[0]).as_deref(),
        Some("New name")
    );
    assert!(scene.next(true).is_empty());

    // A request that is on its way keeps its place: a state that is replaced again and
    // again starts no fifth request.
    let many = accounts(MOST_REQUESTS);
    scene.show(&many);
    let first = scene.next(true);
    assert_eq!(first.len(), MOST_REQUESTS);
    scene.people.stale(scene.server);
    assert!(scene.next(true).is_empty());
    // Each late answer frees one place, and its account is asked for again.
    scene
        .people
        .profile_answered(&first[0], Answer::Read(profile(many[0], "Old name", None)));
    assert_eq!(scene.people.name(scene.server, many[0]), None);
    assert_eq!(profile_asks(&scene.next(false)), [many[0]]);
}

#[cfg(unix)]
#[test]
fn the_picture_of_the_preset_is_read_from_a_regular_file_only() {
    use junto_client_core::paths::DataDir;

    let directory = tempfile::Builder::new()
        .prefix("junto-client12-fix-")
        .tempdir()
        .unwrap();
    let store = ProfileStore::new(DataDir::open(directory.path().join("junto")).unwrap());
    let picture = directory.path().join("picture.png");
    std::fs::write(&picture, png(4, 1)).unwrap();
    // A link to a picture is not followed.
    let link = directory.path().join("link.png");
    std::os::unix::fs::symlink(&picture, &link).unwrap();
    assert!(matches!(
        store_picture(&store, &link),
        Err(PictureRefusal::Unreadable)
    ));
    assert!(matches!(
        store_picture(&store, directory.path()),
        Err(PictureRefusal::Unreadable)
    ));
    assert!(store.avatar().unwrap().is_none());
    assert!(matches!(store_picture(&store, &picture), Ok(Some(_))));
    assert!(store.avatar().unwrap().is_some());
}
