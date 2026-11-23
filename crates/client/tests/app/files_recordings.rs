//! The Files panel of a voice channel has the tabs Files and Recordings; a text channel has
//! only its files (ADR 0784).

use gpui::TestAppContext;

use crate::harness::{World, member};

#[gpui::test]
fn the_files_panel_of_a_voice_channel_has_its_recordings_tab(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");

    // A text channel: its files, without tabs.
    window.click_channel("general");
    window.click("place-files");
    window.until("the files of the text channel", |window| {
        window.shows("file-list-panel")
    });
    assert!(!window.shows("files-tab-recordings"));
    window.click("close-search");

    // A voice channel: the tab Recordings reads its recordings, of which it has none.
    window.click_channel("Daily");
    window.click("place-files");
    window.until("the tabs of the voice channel", |window| {
        window.shows("files-tab-recordings")
    });
    window.click("files-tab-recordings");
    window.until("the recordings are read", |window| {
        window.shows("channel-recordings-empty")
    });
    window.click("files-tab-files");
    assert!(!window.shows("channel-recordings-empty"));
}

/// The tab Recordings stays chosen when another voice channel is shown, and reads that
/// channel; the list of one account is forgotten when the device has another account
/// (ADR 0784).
#[gpui::test]
fn the_recordings_tab_follows_the_shown_channel_and_the_account(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    crate::calls::voice_channel(&world, &admin, &mut window, "Standup");
    let key = |window: &mut crate::harness::Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.channel_recordings().key)
    };

    window.click_channel("Daily");
    window.click("place-files");
    window.until("the tabs of the voice channel", |window| {
        window.shows("files-tab-recordings")
    });
    window.click("files-tab-recordings");
    window.until("the recordings of Daily are read", |window| {
        window.shows("channel-recordings-empty")
    });
    let daily = key(&mut window).unwrap().3;

    // Another voice channel: the tab reads its recordings by itself.
    window.click_channel("Standup");
    window.click("place-files");
    window.until("the recordings of Standup are read", |window| {
        window.shows("channel-recordings-empty") && key(window).is_some_and(|key| key.3 != daily)
    });
    let first = window.own_account();
    assert_eq!(key(&mut window).unwrap().1, first);

    // The account is removed: its list is forgotten; the next account reads its own.
    world.run(admin.handle.api().remove_account(first)).unwrap();
    window.until("the removal is shown", |window| window.shows("no-account"));
    assert_eq!(key(&mut window), None);
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.join_with_code(code.expose());
    window.until_ready();
    let next = window.own_account();
    assert_ne!(next, first);
    window.click_channel("Standup");
    window.click("place-files");
    window.until("the recordings of the next account are read", |window| {
        window.shows("channel-recordings-empty") && key(window).is_some_and(|key| key.1 == next)
    });
}
