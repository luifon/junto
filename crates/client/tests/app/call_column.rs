//! The right column of a call: the pins and the search of its channel beside the tabs, and
//! the views that open over its chat (ADR 0741).

use gpui::TestAppContext;

use crate::harness::{World, member};

/// "Pins" beside the tabs opens the pin list of the channel of the call over its chat, and
/// the row "Chat" goes back; "Search" draws a search field under the tabs.
#[gpui::test]
fn the_pins_of_the_channel_of_the_call_open_over_its_chat_and_go_back(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = crate::calls::connected_call(&mut window, "Daily");
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(junto_media::calls::CallsView {
        call: Some(call),
        ..Default::default()
    });
    window.until("the panel of the call", |window| window.shows("call-chat"));
    assert!(window.shows("call-pins") && window.shows("call-search"));

    window.click("call-pins");
    window.until("the pins open over the chat", |window| {
        window.shows("pins-panel") && window.shows("call-back-to-chat")
    });
    assert!(!window.shows("call-chat"));
    assert!(window.view().call.is_some());
    window.click("call-back-to-chat");
    window.until("the chat returns", |window| {
        window.shows("call-chat") && !window.shows("pins-panel")
    });

    window.click("call-search");
    assert!(window.shows("search-field"));
    window.click("call-search");
    assert!(!window.shows("search-field"));
}

/// A step that opens a view over the chat of the call.
type Opening = Box<dyn Fn(&mut crate::harness::Window)>;

/// A thread, the pin list or a transcript over the chat belongs to the Chat tab: "In the call"
/// shows the people while one is open, and Chat shows it again. The transcript of a finished
/// recording of the channel opens during the call (ADR 0741).
#[gpui::test]
fn the_tab_in_the_call_shows_the_people_while_a_view_is_open_over_the_chat(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = crate::calls::connected_call(&mut window, "Daily");
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(junto_media::calls::CallsView {
        call: Some(call),
        ..Default::default()
    });
    window.until("the panel of the call", |window| window.shows("call-chat"));
    let view = window.view();
    let workspace = view.workspace().unwrap();
    let (server_id, workspace_id) = (view.selected.as_ref().unwrap().id, workspace.id);
    let channel_id = workspace.channel.as_ref().unwrap().id;

    let open: [(&str, Opening); 3] = [
        (
            "pins-panel",
            Box::new(|window: &mut crate::harness::Window| window.click("call-pins")),
        ),
        (
            "thread-panel",
            Box::new(move |window: &mut crate::harness::Window| {
                window.state.update(&mut window.cx, |state, cx| {
                    state.open_thread(
                        server_id,
                        channel_id,
                        junto_client_core::protocol::ids::MessageId::generate(),
                        cx,
                    )
                });
            }),
        ),
        (
            "transcript-panel",
            Box::new(move |window: &mut crate::harness::Window| {
                window.state.update(&mut window.cx, |state, cx| {
                    state.open_transcript(
                        server_id,
                        workspace_id,
                        channel_id,
                        junto_client_core::protocol::ids::RecordingId::generate(),
                        None,
                        cx,
                    )
                });
            }),
        ),
    ];
    for (panel, opening) in open {
        opening(&mut window);
        window.until("the view opens over the chat", |window| {
            window.shows(panel) && window.shows("call-back-to-chat")
        });
        assert!(window.view().call.is_some(), "{panel}");
        window.click("call-tab-members");
        assert!(
            window.shows("call-members") && !window.shows(panel),
            "{panel}"
        );
        window.click("call-tab-chat");
        assert!(window.shows(panel), "{panel}");
        window.click("call-back-to-chat");
        window.until("the chat returns", |window| {
            window.shows("call-chat") && !window.shows(panel)
        });
    }
}
