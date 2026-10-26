//! The pictures of messages in the window against the real server (ADR 0740): a picture
//! takes the width of its list with its proportions, in the main area, in the chat of a
//! call and in the call window.

use gpui::TestAppContext;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::AttachmentId;
use junto_client_core::protocol::message::{Message, SendMessage};
use junto_client_core::transfers::UploadSource;
use junto_media::calls::CallsView;

use crate::calls::{connected_call, voice_channel};
use crate::harness::{Admin, Window, World, member, selector};
use crate::messages::document;

/// A PNG picture with a left half and a right half of two colours.
pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
    let picture = image::RgbaImage::from_fn(width, height, |x, _| {
        if x < width / 2 {
            image::Rgba([200, 90, 40, 255])
        } else {
            image::Rgba([40, 90, 200, 255])
        }
    });
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// The administrator sends a message with a text and these files.
pub(crate) fn send_files(
    world: &World,
    admin: &Admin,
    place: PlaceId,
    text: &str,
    files: Vec<(&str, Vec<u8>)>,
) -> Message {
    world.run(async {
        let mut attachments = Vec::new();
        for (name, bytes) in files {
            let source = UploadSource::Bytes {
                name: name.to_owned(),
                bytes: std::sync::Arc::new(bytes),
            };
            attachments.push(
                admin
                    .handle
                    .transfers()
                    .upload(place, source)
                    .finished()
                    .await
                    .unwrap(),
            );
        }
        let message = SendMessage {
            document: document(text),
            reply_to: None,
            attachments,
        };
        admin
            .handle
            .api()
            .send_message(place, &message)
            .await
            .unwrap()
    })
}

/// Waits until the window holds the decoded picture of this file.
pub(crate) fn until_picture_read(window: &mut Window, attachment_id: AttachmentId) {
    window.until("the picture is read", |window| {
        !window
            .in_shell(|shell, _, _| shell.owners_of_picture(attachment_id))
            .is_empty()
    });
}

/// Opens the panel of the call on its chat tab.
fn show_chat(window: &mut Window) {
    window.in_shell(|shell, window, cx| {
        shell.open(
            junto_client::shell::Open::CallTab(junto_client::shell::call::CallTab::Chat),
            window,
            cx,
        );
    });
}

fn give_calls(window: &mut Window, call: junto_media::calls::ActiveCall) {
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(CallsView {
        call: Some(call),
        ..CallsView::default()
    });
    window.settle();
}

/// Checks that the card of the picture lies inside the message list and that the picture
/// has the proportions of the file. Returns the bounds of the picture.
#[track_caller]
fn fits(
    window: &mut Window,
    attachment_id: AttachmentId,
    (width, height): (f32, f32),
    place: &str,
) -> gpui::Bounds<gpui::Pixels> {
    window.settle();
    let list = window
        .cx
        .debug_bounds("message-list")
        .unwrap_or_else(|| panic!("{place}: the message list is not drawn"));
    let card = window
        .cx
        .debug_bounds(selector(format!("picture-card:{attachment_id}")))
        .unwrap_or_else(|| panic!("{place}: the card of the picture is not drawn"));
    let picture = window
        .cx
        .debug_bounds(selector(format!("picture:{attachment_id}")))
        .unwrap_or_else(|| panic!("{place}: the picture is not drawn"));
    assert!(
        card.left() >= list.left() && card.right() <= list.right(),
        "{place}: the card {card:?} passes the edge of the list {list:?}"
    );
    assert!(
        picture.left() >= card.left() && picture.right() <= card.right(),
        "{place}: the picture {picture:?} passes the edge of its card {card:?}"
    );
    let drawn = f32::from(picture.size.width) / f32::from(picture.size.height);
    assert!(
        (drawn - width / height).abs() < 0.02,
        "{place}: the picture {picture:?} has the proportions {drawn}, the file {}",
        width / height
    );
    picture
}

/// A picture wider than the chat panel of a call is reduced to the width of the panel with
/// its proportions, in the main window and in the call window.
#[gpui::test]
fn a_picture_in_the_chat_of_a_call_fits_the_panel_and_keeps_its_proportions(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let daily = (window.channels().into_iter())
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    let call = connected_call(&mut window, "Daily");
    give_calls(&mut window, call);
    window.until("the chat of the call", |window| {
        window.shows("call-tab-chat") && window.shows("call-chat")
    });
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(daily),
        "The plan",
        vec![("plan.png", png(1000, 500))],
    );
    let attachment_id = sent.attachments[0].id;
    window.until_message("The plan");
    until_picture_read(&mut window, attachment_id);
    let in_panel = fits(
        &mut window,
        attachment_id,
        (1000.0, 500.0),
        "the chat of the call",
    );
    let panel = window.cx.debug_bounds("call-panel").unwrap();
    assert!(
        in_panel.size.width < panel.size.width,
        "{in_panel:?} {panel:?}"
    );

    window.click("detach-call");
    let mut detached = window.call_window();
    show_chat(&mut detached);
    detached.until("the chat of the call window shows the picture", |window| {
        window.shows("call-chat")
            && window.shows_named(format!("picture:{attachment_id}"))
            && !window
                .in_shell(|shell, _, _| shell.owners_of_picture(attachment_id))
                .is_empty()
    });
    for width in [480.0, 800.0, 1200.0] {
        detached.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(600.0))));
        fits(
            &mut detached,
            attachment_id,
            (1000.0, 500.0),
            &format!("the call window at a width of {width}"),
        );
    }
}

/// A picture lies inside the message list of the main area at every width of the window
/// and every interface scale, with its proportions. Where the list is wide enough, it has
/// its size in the list (at most 292 by 220 pixels of the design).
#[gpui::test]
fn a_picture_fits_the_message_list_at_every_window_width_and_interface_scale(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "The plan",
        vec![("plan.png", png(1000, 500))],
    );
    let attachment_id = sent.attachments[0].id;
    window.until_message("The plan");
    until_picture_read(&mut window, attachment_id);
    for scale in [
        junto_client::theme::SCALE_MIN,
        100,
        junto_client::theme::SCALE_MAX,
    ] {
        window
            .state
            .update(&mut window.cx, |state, cx| state.set_scale(scale, cx));
        for width in [720.0, 1000.0, 1600.0] {
            window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(width), gpui::px(700.0))));
            let drawn = fits(
                &mut window,
                attachment_id,
                (1000.0, 500.0),
                &format!("a width of {width} at {scale}%"),
            );
            if width == 1600.0 {
                // The card has the size of the picture, and the picture fills it inside its
                // border.
                let card = window
                    .cx
                    .debug_bounds(selector(format!("picture-card:{attachment_id}")))
                    .unwrap();
                let full = 292.0 * scale as f32 / 100.0;
                assert!(
                    (f32::from(card.size.width) - full).abs() < 1.0
                        && card.size.width - drawn.size.width < gpui::px(3.0),
                    "at {scale}% the card {card:?} or the picture {drawn:?} has not the size \
                     of the picture, {full} pixels"
                );
            }
        }
    }
}

/// Clicks the part of the picture of this file that the message list shows.
#[track_caller]
fn click_picture(window: &mut Window, attachment_id: AttachmentId) {
    window.settle();
    let list = window.cx.debug_bounds("message-list").unwrap();
    let picture = window
        .cx
        .debug_bounds(selector(format!("picture:{attachment_id}")))
        .unwrap();
    let shown = picture.intersect(&list);
    assert!(
        shown.size.width > gpui::px(4.0) && shown.size.height > gpui::px(4.0),
        "the picture {picture:?} is not shown in the list {list:?}"
    );
    window.press_at(shown.center(), gpui::MouseButton::Left);
    window.settle();
}

/// The texts that the window draws in its next frame.
fn painted(window: &mut Window) -> Vec<String> {
    let _record = junto_client::ui::record_painted_text();
    window.cx.update(|window, _| window.refresh());
    window.settle();
    junto_client::ui::take_painted_text()
        .into_iter()
        .map(|painted| painted.text.to_string())
        .collect()
}

/// The viewer shows the picture of this name with its size and its place among the
/// pictures of the message, fitted inside the window with the proportions of the file.
#[track_caller]
fn viewer_shows(
    window: &mut Window,
    name: &str,
    bytes: u64,
    place: Option<&str>,
    (width, height): (f32, f32),
) {
    window.until(&format!("the viewer shows {name}"), |window| {
        let texts = painted(window);
        window.shows("picture-viewer")
            && texts.iter().any(|text| text == name)
            && texts
                .iter()
                .any(|text| *text == junto_client::text::file_size(bytes))
            && place.is_none_or(|place| texts.iter().any(|text| text == place))
    });
    let picture = window.cx.debug_bounds("viewer-picture").unwrap();
    let size = window.cx.update(|window, _| window.viewport_size());
    assert!(
        picture.left() >= gpui::px(0.0)
            && picture.top() >= gpui::px(0.0)
            && picture.right() <= size.width
            && picture.bottom() <= size.height,
        "{name}: the picture {picture:?} passes the edge of the window {size:?}"
    );
    let drawn = f32::from(picture.size.width) / f32::from(picture.size.height);
    assert!(
        (drawn - width / height).abs() < 0.02,
        "{name}: the picture {picture:?} has the proportions {drawn}, the file {}",
        width / height
    );
    assert!(
        f32::from(picture.size.width) <= width + 0.5,
        "{name}: the picture {picture:?} is larger than its {width} pixels"
    );
}

/// A press on a picture of a message opens it large over the window, with its name, its
/// size, its download, and arrows and the Left and Right keys to the other pictures of the
/// message. Escape, the close button and a press outside the picture close it, and the
/// focus returns.
#[gpui::test]
fn a_press_on_a_picture_opens_the_viewer_with_its_name_its_size_its_download_and_arrows(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let files = [
        ("first.png", png(600, 300)),
        ("second.png", png(300, 600)),
        ("third.png", png(800, 800)),
    ];
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "Three pictures",
        files
            .iter()
            .map(|(name, bytes)| (*name, bytes.clone()))
            .collect(),
    );
    let ids: Vec<AttachmentId> = sent.attachments.iter().map(|file| file.id).collect();
    let sizes: Vec<u64> = sent.attachments.iter().map(|file| file.bytes).collect();
    window.until_message("Three pictures");
    for id in &ids {
        until_picture_read(&mut window, *id);
    }
    let downloads = tempfile::tempdir().unwrap();
    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell
            .state()
            .update(cx, |state, _| state.set_download_directory(target));
    });

    click_picture(&mut window, ids[0]);
    viewer_shows(
        &mut window,
        "first.png",
        sizes[0],
        Some("1 of 3"),
        (600.0, 300.0),
    );
    assert!(window.in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)));

    // The download of the viewer saves the file of the message.
    window.click_named(format!("viewer-download:{}", ids[0]));
    let saved = downloads.path().join("first.png");
    window.until("the file is saved", |_| {
        std::fs::read(&saved).is_ok_and(|read| read == files[0].1)
    });
    assert!(window.shows("picture-viewer"));

    // The arrows and the keys go from one picture to the next, and from the last to the
    // first.
    window.click("viewer-next");
    viewer_shows(
        &mut window,
        "second.png",
        sizes[1],
        Some("2 of 3"),
        (300.0, 600.0),
    );
    window.press("right");
    viewer_shows(
        &mut window,
        "third.png",
        sizes[2],
        Some("3 of 3"),
        (800.0, 800.0),
    );
    window.press("right");
    viewer_shows(
        &mut window,
        "first.png",
        sizes[0],
        Some("1 of 3"),
        (600.0, 300.0),
    );
    window.press("left");
    viewer_shows(
        &mut window,
        "third.png",
        sizes[2],
        Some("3 of 3"),
        (800.0, 800.0),
    );
    window.click("viewer-previous");
    viewer_shows(
        &mut window,
        "second.png",
        sizes[1],
        Some("2 of 3"),
        (300.0, 600.0),
    );

    // A press on the picture leaves the viewer open; Escape closes it.
    window.click("viewer-picture");
    assert!(window.shows("picture-viewer"));
    window.press("escape");
    assert!(!window.shows("picture-viewer"));
    assert!(!window.in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)));

    // A press outside the picture closes the viewer.
    click_picture(&mut window, ids[1]);
    viewer_shows(
        &mut window,
        "second.png",
        sizes[1],
        Some("2 of 3"),
        (300.0, 600.0),
    );
    let picture = window.cx.debug_bounds("viewer-picture").unwrap();
    window.press_at(
        gpui::point(picture.left() - gpui::px(40.0), picture.center().y),
        gpui::MouseButton::Left,
    );
    assert!(!window.shows("picture-viewer"));

    // The close button closes it.
    click_picture(&mut window, ids[2]);
    viewer_shows(
        &mut window,
        "third.png",
        sizes[2],
        Some("3 of 3"),
        (800.0, 800.0),
    );
    window.click("close-picture");
    assert!(!window.shows("picture-viewer"));
}

/// The picture of a message in the chat of a call opens in the viewer, in the main window
/// and in the call window. The viewer is fitted to the smaller call window.
#[gpui::test]
fn a_picture_in_the_chat_of_a_call_opens_in_the_viewer_of_both_windows(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let daily = (window.channels().into_iter())
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    let call = connected_call(&mut window, "Daily");
    give_calls(&mut window, call);
    window.until("the chat of the call", |window| {
        window.shows("call-tab-chat") && window.shows("call-chat")
    });
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(daily),
        "The plan",
        vec![("plan.png", png(1000, 500))],
    );
    let (id, bytes) = (sent.attachments[0].id, sent.attachments[0].bytes);
    window.until_message("The plan");
    until_picture_read(&mut window, id);
    click_picture(&mut window, id);
    viewer_shows(&mut window, "plan.png", bytes, None, (1000.0, 500.0));
    assert!(!window.shows("viewer-next") && !window.shows("viewer-position"));
    window.press("escape");
    assert!(!window.shows("picture-viewer") && window.shows("call-chat"));

    window.click("detach-call");
    let mut detached = window.call_window();
    show_chat(&mut detached);
    detached.until("the chat of the call window shows the picture", |window| {
        window.shows("call-chat") && window.shows_named(format!("picture:{id}"))
    });
    detached.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(700.0))));
    click_picture(&mut detached, id);
    assert!(detached.shows("picture-viewer"));
    viewer_shows(&mut detached, "plan.png", bytes, None, (1000.0, 500.0));
    // The open viewer follows a smaller window.
    detached.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(640.0), gpui::px(420.0))));
    viewer_shows(&mut detached, "plan.png", bytes, None, (1000.0, 500.0));
    let picture = detached.cx.debug_bounds("viewer-picture").unwrap();
    assert!(
        f32::from(picture.size.width) < 640.0,
        "the picture {picture:?} is not reduced to the call window"
    );
    assert!(detached.in_shell(|shell, window, cx| shell.layer_holds_focus(window, cx)));
    detached.press("escape");
    assert!(!detached.shows("picture-viewer"));
    detached.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(700.0))));
    click_picture(&mut detached, id);
    viewer_shows(&mut detached, "plan.png", bytes, None, (1000.0, 500.0));
    let picture = detached.cx.debug_bounds("viewer-picture").unwrap();
    detached.press_at(
        gpui::point(picture.center().x, picture.bottom() + gpui::px(10.0)),
        gpui::MouseButton::Left,
    );
    assert!(!detached.shows("picture-viewer"));
}

/// A picture with a side above 1,024 pixels is drawn in the list from a reduced copy, and
/// the viewer reads it at its own size while it is open.
#[gpui::test]
fn a_picture_larger_than_the_copy_of_the_lists_is_read_at_its_own_size_for_the_viewer(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "A wide screen",
        vec![("screen.png", png(2400, 1200))],
    );
    let (id, bytes) = (sent.attachments[0].id, sent.attachments[0].bytes);
    assert_eq!(
        (sent.attachments[0].width, sent.attachments[0].height),
        (Some(2400), Some(1200))
    );
    window.until_message("A wide screen");
    until_picture_read(&mut window, id);
    fits(&mut window, id, (2400.0, 1200.0), "the channel");
    click_picture(&mut window, id);
    window.until("the viewer holds the picture at its own size", |window| {
        window.in_shell(|shell, _, _| shell.viewer_image_size()) == Some((2400, 1200))
    });
    viewer_shows(&mut window, "screen.png", bytes, None, (2400.0, 1200.0));
    window.press("escape");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.viewer_image_size()),
        None
    );
}
