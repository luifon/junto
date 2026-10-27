//! The pictures of people in the window against the real server (ADR 0783): the avatar of
//! an account at each place that shows the person, the generated avatar after its removal,
//! and the picture of the profile preset in the settings.

use std::path::Path;

use gpui::TestAppContext;
use junto_client::picture::Refusal;
use junto_client::state::people::{Face, PictureHash};
use junto_client::state::view::Members;
use junto_client_core::book::ProfileSync;
use junto_client_core::protocol::ids::{AccountId, ChannelId, ServerId};

use crate::harness::{Admin, Window, World, member, named};
use crate::messages::document;
use crate::support::{Fault, FaultProxy, logged_in};

/// A PNG image of one colour.
pub(crate) fn png(side: u32, shade: u8) -> Vec<u8> {
    let picture = image::RgbaImage::from_pixel(side, side, image::Rgba([shade, 90, 40, 255]));
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

impl Window {
    fn server_id(&mut self) -> ServerId {
        self.view().selected.unwrap().id
    }

    /// The picture of an account as the window drew it last.
    pub(crate) fn face(&mut self, account_id: AccountId) -> Option<Face> {
        let view = self.view();
        let server_id = view.selected.as_ref()?.id;
        view.faces.face(server_id, account_id).cloned()
    }

    /// True when the window drew the account with the image that has this hash.
    pub(crate) fn shows_picture(&mut self, account_id: AccountId, hash: PictureHash) -> bool {
        self.face(account_id)
            .is_some_and(|face| face.hash == hash && face.image.is_some())
    }

    /// Runs the work of the application that follows a change of its state, as a change
    /// of a server does.
    fn change(&mut self) {
        let server_id = self.server_id();
        let state = self.state.clone();
        self.cx.update(|_, cx| {
            state.update(cx, |state, cx| state.select_server(server_id, cx));
        });
        self.run_tasks();
    }

    /// True when the window draws the image of the avatar of the account somewhere.
    pub(crate) fn draws_picture(&mut self, account_id: AccountId) -> bool {
        self.shows_named(format!("avatar-image:{}", account_id.as_uuid().as_u128()))
    }

    /// The requests for profiles and images of the selected server that are on their way.
    fn people_on_way(&mut self) -> usize {
        let server_id = self.server_id();
        self.state
            .read_with(&self.cx, |state, _| state.people_on_way(server_id))
    }

    /// The number of requests in a row for the profile of an account that got no answer.
    fn unanswered(&mut self, account_id: AccountId) -> Option<u8> {
        let server_id = self.server_id();
        self.state.read_with(&self.cx, |state, _| {
            state.profile_unanswered(server_id, account_id)
        })
    }

    fn open_members(&mut self, count: usize) {
        self.click("members-button");
        self.until("the members are listed", |window| {
            matches!(
                &window.view().workspace().unwrap().members,
                Members::Loaded(members) if members.len() == count
            )
        });
    }

    fn use_picture(&mut self, file: &Path) {
        let file = file.to_owned();
        self.in_shell(|shell, _, cx| shell.use_preset_picture(file, cx));
    }

    /// Chooses a picture for the preset and saves its cut (ADR 0119).
    fn save_picture(&mut self, file: &Path) {
        self.use_picture(file);
        self.until_picture_crop();
        self.click("picture-crop-save");
    }
}

fn avatar_path(account_id: AccountId) -> String {
    format!("/api/v1/accounts/{account_id}/avatar")
}

fn profile_path(account_id: AccountId) -> String {
    format!("/api/v1/accounts/{account_id}/profile")
}

pub(crate) fn set_avatar(world: &World, admin: &Admin, image: &[u8]) -> PictureHash {
    let stored = world
        .run(admin.handle.api().set_avatar(image.to_vec()))
        .unwrap();
    let hash = PictureHash::parse(&stored.hash).unwrap();
    assert_eq!(hash, PictureHash::of(image));
    hash
}

#[gpui::test]
fn the_picture_of_an_account_is_shown_at_its_rows_and_the_generated_avatar_after_its_removal(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let ada = logged_in(&admin.handle);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel_id = window.selected_channel();
    admin.send(&world, channel_id, document("Good morning"));
    let message = window.until_message("Good morning");
    assert_eq!(message.message.author_id, Some(ada));
    window.open_members(2);
    assert!(window.shows_member("Ada"));

    // Ada has no avatar: the window read her profile and draws the generated avatar.
    window.until("the profile of Ada was read", |_| {
        proxy
            .seen(&profile_path(ada))
            .iter()
            .any(|seen| seen.answer.is_some())
    });
    assert_eq!(window.face(ada), None);
    assert!(proxy.seen(&avatar_path(ada)).is_empty());

    // The administrator sets an avatar: the event tells the window, which reads the
    // image once and draws it at the member row and at the message.
    let first = set_avatar(&world, &admin, &png(48, 200));
    window.until("the picture of Ada is drawn", |window| {
        window.shows_picture(ada, first)
    });
    assert!(window.shows_member("Ada"));
    assert!(window.shows_named(format!("message:{}", message.message.id)));
    let profiles = proxy.seen(&profile_path(ada)).len();

    // Another image has another hash: the window reads it and draws it in place of the
    // first one.
    let second = set_avatar(&world, &admin, &png(64, 20));
    assert_ne!(first, second);
    window.until("the new picture of Ada is drawn", |window| {
        window.shows_picture(ada, second)
    });
    // Each image was read once, and the events told the profile: it was not read again.
    for _ in 0..3 {
        window.change();
    }
    assert_eq!(proxy.seen(&avatar_path(ada)).len(), 2);
    assert_eq!(proxy.seen(&profile_path(ada)).len(), profiles);

    // The administrator removes the avatar: the generated avatar returns.
    world.run(admin.handle.api().remove_avatar()).unwrap();
    window.until("the generated avatar of Ada is drawn", |window| {
        window.face(ada).is_none()
    });
    assert!(window.shows_member("Ada"));
    assert_eq!(proxy.seen(&avatar_path(ada)).len(), 2);

    // The own account shows the avatar that the state of the server tells: its profile
    // is not read.
    let own = window.own_account();
    let api = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0)
        .api();
    let image = png(32, 120);
    world.run(api.set_avatar(image.clone())).unwrap();
    window.until("the own picture is drawn", |window| {
        window.shows_picture(own, PictureHash::of(&image))
    });
    assert!(proxy.seen(&profile_path(own)).is_empty());
    assert_eq!(proxy.seen(&avatar_path(own)).len(), 1);
}

/// A member window on a new server whose owner Ada has a picture and writes in a channel
/// that the window does not show, so that nothing else that the window shows names her.
struct PictureScene {
    _server: crate::support::TestServer,
    admin: Admin,
    _directory: tempfile::TempDir,
    window: Window,
    random: ChannelId,
    ada: AccountId,
    hash: PictureHash,
}

fn picture_scene(cx: &mut TestAppContext, world: &World) -> PictureScene {
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let ada = logged_in(&admin.handle);
    let (directory, mut window) = member(cx, world, &server.address(), &admin, "Luca");
    window.until("the messages of the text channel are read", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let random = admin.text_channel(world, &mut window, "random");
    let hash = set_avatar(world, &admin, &png(48, 200));
    PictureScene {
        _server: server,
        admin,
        _directory: directory,
        window,
        random,
        ada,
        hash,
    }
}

#[gpui::test]
fn the_author_of_a_notice_is_drawn_with_the_picture(cx: &mut TestAppContext) {
    let world = World::new();
    let PictureScene {
        _server,
        admin,
        _directory,
        mut window,
        random,
        ada,
        hash,
    } = picture_scene(cx, &world);
    let own = window.own_account();
    admin.mention(&world, random, "the quartz review, ", own);
    window.until("the notice is counted", |window| {
        window.unread_notices() == 1
    });
    assert!(!window.draws_picture(ada));
    window.open_notices();
    window.until("the picture of Ada at the notice", |window| {
        window.shows_picture(ada, hash) && window.draws_picture(ada)
    });
}

#[gpui::test]
fn the_author_of_a_search_result_is_drawn_with_the_picture(cx: &mut TestAppContext) {
    let world = World::new();
    let PictureScene {
        _server,
        admin,
        _directory,
        mut window,
        random,
        ada,
        hash,
    } = picture_scene(cx, &world);
    admin.send(&world, random, document("the quartz review"));
    assert!(!window.draws_picture(ada));
    window.search_for("quartz");
    window.until("the result of Ada", |window| {
        window.until_searched().results.len() == 1
    });
    window.until("the picture of Ada at the result", |window| {
        window.shows_picture(ada, hash) && window.draws_picture(ada)
    });
}

#[gpui::test]
fn a_profile_that_gets_no_answer_is_asked_for_three_times(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let ada = logged_in(&admin.handle);
    let proxy = world.run(FaultProxy::start(&server));
    // No request for the profile of Ada gets an answer.
    proxy.fail(&profile_path(ada), [Fault::Cut; 12]);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.open_members(2);
    let asked = |proxy: &FaultProxy| proxy.seen(&profile_path(ada)).len();
    window.until("the profile of Ada was asked for", |_| asked(&proxy) >= 1);
    // Each change of the state asks once more, until three requests in a row got no
    // answer. A change asks for nothing while a request is on its way, so the changes go
    // on until then. A request that was on its way when the state of the server was
    // loaded counts for nothing, so the proxy can have seen more than three.
    window.until("three requests in a row got no answer", |window| {
        window.change();
        window.unanswered(ada) == Some(3) && window.people_on_way() == 0
    });
    // Every request has ended, so the proxy has counted each one: the three, and at most
    // the one that was on its way when the first state of the server was loaded.
    let sent = asked(&proxy);
    assert!((3..=4).contains(&sent), "{sent}");
    // No later change asks again: a request that a change started would be on its way
    // until the proxy has counted it.
    for _ in 0..8 {
        window.until("no request is on its way", |window| {
            window.people_on_way() == 0
        });
        window.change();
    }
    window.until("no request is on its way", |window| {
        window.people_on_way() == 0
    });
    assert_eq!(window.unanswered(ada), Some(3));
    assert_eq!(asked(&proxy), sent);
    // The member list names the person, and the generated avatar is drawn.
    assert!(window.shows_member("Ada"));
    assert_eq!(window.face(ada), None);
}

#[gpui::test]
fn the_picture_of_the_preset_is_set_in_the_settings_and_goes_to_a_server_joined_afterwards(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = named(cx, "Luca");
    let files = tempfile::tempdir().unwrap();

    window.click("app-settings");
    window.click("settings-section-picture");
    assert!(window.shows("settings-dialog"));
    assert!(window.shows("preset-picture") && window.shows("choose-picture"));
    assert!(!window.shows("remove-picture"));
    assert_eq!(window.view().preset_picture, None);

    // A file that is no picture, and a file that is too large, are refused with the
    // reason, and nothing is stored.
    let notes = files.path().join("notes.txt");
    std::fs::write(&notes, b"not a picture").unwrap();
    window.use_picture(&notes);
    window.until("the refusal is shown", |window| {
        window.shows("picture-error")
    });
    let refusal = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.preset_picture().failed.clone())
    };
    assert_eq!(
        refusal(&mut window).as_deref(),
        Some(junto_client::text::picture_refusal(Refusal::Unsupported))
    );
    let mut large = png(16, 1);
    large.resize(10 * 1024 * 1024 + 1, 0);
    let heavy = files.path().join("heavy.png");
    std::fs::write(&heavy, &large).unwrap();
    window.use_picture(&heavy);
    window.until("the size is refused", |window| {
        refusal(window).as_deref() == Some(junto_client::text::picture_refusal(Refusal::TooLarge))
    });
    window.use_picture(&files.path().join("missing.png"));
    window.until("the missing file is told", |window| {
        refusal(window).as_deref()
            == Some(junto_client::text::picture_refusal(Refusal::Unreadable))
    });
    let held = |directory: &Path| {
        ["avatar.png", "avatar.jpg", "avatar.webp"]
            .into_iter()
            .map(|name| directory.join("junto").join(name))
            .find(|file| file.exists())
    };
    assert_eq!(held(directory.path()), None);
    assert!(!window.shows("remove-picture"));

    // A picture is cut, stored in the data directory and drawn in the settings.
    let chosen = files.path().join("me.png");
    std::fs::write(&chosen, png(96, 180)).unwrap();
    window.save_picture(&chosen);
    window.until("the picture of the preset is drawn", |window| {
        window.view().preset_picture.is_some() && window.shows("remove-picture")
    });
    assert!(!window.shows("picture-error"));
    let image = std::fs::read(held(directory.path()).unwrap()).unwrap();
    let read = image::load_from_memory(&image).unwrap();
    assert_eq!((read.width(), read.height()), (96, 96));
    window.click("dialog-close");

    // The device joins a server: the new account gets the picture, and the window draws
    // it for the own account.
    let code = admin.invite(&world, 1);
    window.find_server(&server.address());
    window.join_with_code(code.expose());
    window.until_ready();
    let own = window.own_account();
    let hash = PictureHash::of(&image);
    window.until("the own picture is drawn", |window| {
        window.shows_picture(own, hash)
    });
    let profile = world.run(admin.handle.api().profile(own)).unwrap();
    assert_eq!(profile.avatar.unwrap().hash, hash.to_string());
    let handle = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    assert_eq!(handle.profile_sync(), ProfileSync::Applied);
    assert!(window.view().preset_refused.is_empty());

    // The picture of the preset is removed: the file is gone, and the account on the
    // server keeps its avatar (ADR 0717).
    window.click("app-settings");
    window.click("settings-section-picture");
    assert!(!window.shows("preset-refused"));
    window.click("remove-picture");
    window.until("the preset has no picture", |window| {
        window.view().preset_picture.is_none() && !window.shows("remove-picture")
    });
    assert_eq!(held(directory.path()), None);
    assert!(window.shows_picture(own, hash));
}

#[gpui::test]
fn a_server_that_refused_the_picture_of_the_preset_is_named_in_the_settings(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    // A server that takes pictures of at most 1024 bytes (ADR 0119).
    let server = world.server_with_settings("Acme Studio", &[("JUNTO_PICTURE_BYTES", "1024")]);
    let admin = world.owner(&server);
    let (_directory, mut window) = named(cx, "Luca");
    let files = tempfile::tempdir().unwrap();

    // The preset takes a picture of 600 by 600 pixels; that server does not.
    let chosen = files.path().join("wide.png");
    std::fs::write(&chosen, crate::harness::picture_png(600, 600)).unwrap();
    window.save_picture(&chosen);
    window.until("the picture of the preset is stored", |window| {
        window.view().preset_picture.is_some()
    });

    let code = admin.invite(&world, 1);
    window.find_server(&server.address());
    window.join_with_code(code.expose());
    window.until_ready();
    let server_id = window.server_id();
    window.until("the server refused the picture", |window| {
        window.view().preset_refused == [(server_id, "Acme Studio".to_owned())]
    });
    let own = window.own_account();
    assert_eq!(window.face(own), None);

    window.click("app-settings");
    window.click("settings-section-picture");
    assert!(window.shows_named(format!("preset-refused:{server_id}")));
    let line = junto_client::text::people::preset_refused("Acme Studio");
    assert!(line.starts_with("Acme Studio did not accept"));
}
