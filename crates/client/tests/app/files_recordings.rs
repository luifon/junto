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
