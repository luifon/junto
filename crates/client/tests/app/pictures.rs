//! The pictures of messages in the window against the real server (ADR 0740): a picture
//! takes the width of its list with its proportions, in the main area, in the chat of a
//! call and in the call window.

use gpui::TestAppContext;
use junto_client::shell::Pane;
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

/// A JPEG photo as a phone stores a portrait photo: the pixels lie on their side, `width`
/// by `height`, with a red left half, and its EXIF segment states the orientation 6 (turn a
/// quarter to the right to show it).
pub(crate) fn turned_jpeg(width: u32, height: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(width, height, |x, _| {
        if x < width / 2 {
            image::Rgb([220, 30, 30])
        } else {
            image::Rgb([30, 30, 220])
        }
    });
    let mut encoded = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Jpeg,
        )
        .unwrap();
    // The TIFF structure in big-endian order with one entry: the orientation, 6.
    let mut block = b"Exif\0\0MM\0\x2a\0\0\0\x08\0\x01".to_vec();
    block.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0]);
    let length = u16::try_from(block.len() + 2).unwrap().to_be_bytes();
    let mut bytes = encoded[..2].to_vec();
    bytes.extend_from_slice(&[0xff, 0xe1, length[0], length[1]]);
    bytes.extend(block);
    bytes.extend_from_slice(&encoded[2..]);
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

/// A portrait photo that a phone stored on its side with an EXIF orientation is drawn
/// upright: the server states the size after the turn, the card has portrait proportions,
/// and the copy that the list holds is taller than wide.
#[gpui::test]
fn a_photo_stored_on_its_side_is_drawn_upright(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "From the phone",
        vec![("photo.jpg", turned_jpeg(1600, 1200))],
    );
    let attachment = &sent.attachments[0];
    assert_eq!(
        (attachment.width, attachment.height),
        (Some(1200), Some(1600)),
        "the server states the size after the turn"
    );
    window.until_message("From the phone");
    until_picture_read(&mut window, attachment.id);
    fits(&mut window, attachment.id, (1200.0, 1600.0), "the list");
    let ((width, height), _) = held(&mut window, attachment.id);
    assert!(
        height > width,
        "the copy of the list is {width} by {height}, on its side"
    );
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

/// A picture with a side above 1,024 pixels is drawn in the list from a reduced copy. The
/// viewer reads it at the size of its place in pixels of the device, never larger than its
/// own size: in a window narrower than the picture a reduced copy, and its own size in a
/// window wider than it (review M4). The bytes of the copy are printed for the record.
#[gpui::test]
fn a_picture_larger_than_the_copy_of_the_lists_is_read_at_the_size_of_the_viewer(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0))));
    let general = window.selected_channel();
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "A wide screen",
        vec![("screen.png", png(5120, 2880))],
    );
    let (id, bytes) = (sent.attachments[0].id, sent.attachments[0].bytes);
    window.until_message("A wide screen");
    until_picture_read(&mut window, id);
    fits(&mut window, id, (5120.0, 2880.0), "the channel");
    click_picture(&mut window, id);
    let device = window.cx.update(|window, _| {
        let viewport = window.viewport_size();
        (
            f32::from(viewport.width) * window.scale_factor(),
            f32::from(viewport.height) * window.scale_factor(),
        )
    });
    window.until("the viewer holds its copy", |window| {
        window
            .in_shell(|shell, _, _| shell.viewer_image_size())
            .is_some()
    });
    let (width, height) = window
        .in_shell(|shell, _, _| shell.viewer_image_size())
        .unwrap();
    eprintln!(
        "the viewer holds 5120 by 2880 as {width} by {height}: {} bytes, against {} at its own size",
        u64::from(width) * u64::from(height) * 4,
        5120u64 * 2880 * 4
    );
    assert!(
        (width as f32) <= device.0 && (height as f32) <= device.1,
        "the copy {width} by {height} is larger than the window {device:?}"
    );
    assert!(
        ((width as f32 / height as f32) - 5120.0 / 2880.0).abs() < 0.02,
        "the copy {width} by {height} keeps the proportions"
    );
    viewer_shows(&mut window, "screen.png", bytes, None, (5120.0, 2880.0));
    window.press("escape");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.viewer_image_size()),
        None
    );

    // A picture smaller than the place of the viewer is read at its own size.
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "A smaller screen",
        vec![("small.png", png(1600, 800))],
    );
    let small = sent.attachments[0].id;
    window.until_message("A smaller screen");
    until_picture_read(&mut window, small);
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(2600.0), gpui::px(1600.0))));
    click_picture(&mut window, small);
    window.until("the viewer holds the picture at its own size", |window| {
        window.in_shell(|shell, _, _| shell.viewer_image_size()) == Some((1600, 800))
    });
    window.press("escape");
}

/// A picture that the window holds is not read again when a smaller interface scale draws
/// it smaller: the held copy covers the smaller size (review M7). A larger scale reads it
/// again at its larger size.
#[gpui::test]
fn a_smaller_scale_draws_the_held_copy_of_a_picture_without_reading_it_again(
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
        vec![("plan.png", png(2400, 1200))],
    );
    let id = sent.attachments[0].id;
    window.until_message("The plan");
    until_picture_read(&mut window, id);
    let reads = window.in_shell(|shell, _, _| shell.picture_reads());
    window
        .state
        .update(&mut window.cx, |state, cx| state.set_scale(90, cx));
    window.settle();
    fits(&mut window, id, (2400.0, 1200.0), "at 90%");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.picture_reads()),
        reads,
        "a smaller scale read the picture again"
    );
    window
        .state
        .update(&mut window.cx, |state, cx| state.set_scale(150, cx));
    window.until("the larger copy is read", |window| {
        window.in_shell(|shell, _, _| shell.picture_reads()) > reads
    });
}

/// A read of a viewer that was closed never passes for the read of the next viewer: each
/// read has its own number in the window (review M3). Two viewers once both numbered their
/// first read 1, so the late answer of the first picture was shown under the second.
#[gpui::test]
fn the_read_of_a_closed_viewer_is_not_taken_for_the_read_of_the_next_one(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let first = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "The first plan",
        vec![("first.png", png(2400, 1200))],
    )
    .attachments[0]
        .id;
    let second = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "The second plan",
        vec![("second.png", png(1200, 2400))],
    )
    .attachments[0]
        .id;
    window.until_message("The second plan");
    until_picture_read(&mut window, first);
    until_picture_read(&mut window, second);
    click_picture(&mut window, first);
    let (shown, earlier) = window.in_shell(|shell, _, _| shell.viewer_read()).unwrap();
    assert_eq!(shown, first);
    window.press("escape");
    click_picture(&mut window, second);
    let (shown, later) = window.in_shell(|shell, _, _| shell.viewer_read()).unwrap();
    assert_eq!(shown, second);
    assert_ne!(
        later, earlier,
        "the read of the second viewer has the number of the read of the first"
    );
    window.press("escape");
}

/// A GIF whose frames have different colours, each shown for `delay` milliseconds.
pub(crate) fn gif(width: u32, height: u32, frames: u32, delay: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(&mut bytes, 30);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .unwrap();
        for index in 0..frames {
            let colour = image::Rgba([(index * 40 % 256) as u8, 90, 200, 255]);
            encoder
                .encode_frame(image::Frame::from_parts(
                    image::RgbaImage::from_pixel(width, height, colour),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(delay, 1),
                ))
                .unwrap();
        }
    }
    bytes
}

/// The size of the copy of a file that the lists hold, and its frames.
fn held(window: &mut Window, id: AttachmentId) -> ((u32, u32), usize) {
    window
        .in_shell(|shell, _, _| shell.held_picture(id))
        .unwrap_or_else(|| panic!("the window holds no picture of {id}"))
}

/// The largest size in pixels of the device at which the lists of the window draw a
/// picture: 292 by 220 pixels of the design.
fn list_fit(window: &mut Window) -> (f32, f32) {
    let scale = window.cx.update(|window, _| window.scale_factor());
    (292.0 * scale, 220.0 * scale)
}

/// A GIF and a screenshot of a screen of 2560 by 1440 pixels are drawn in the list within
/// its width with their proportions, from copies at the size that the list draws, and open
/// in the viewer at the size of its place, at most their own, the GIF with every frame. Its
/// download stays.
#[gpui::test]
fn a_gif_and_a_screenshot_are_drawn_in_the_list_and_open_in_the_viewer(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let party = gif(800, 450, 6, 100);
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "Two pictures",
        vec![
            ("party.gif", party.clone()),
            ("screen.png", png(2560, 1440)),
        ],
    );
    let (moving, screen) = (sent.attachments[0].clone(), sent.attachments[1].clone());
    assert_eq!(
        (
            moving.media_type.as_str(),
            moving.width.zip(moving.height),
            moving.frames
        ),
        ("image/gif", Some((800, 450)), Some(6))
    );
    assert_eq!(
        (
            screen.media_type.as_str(),
            screen.width.zip(screen.height),
            screen.frames
        ),
        ("image/png", Some((2560, 1440)), None)
    );
    window.until_message("Two pictures");
    until_picture_read(&mut window, moving.id);
    until_picture_read(&mut window, screen.id);
    fits(&mut window, moving.id, (800.0, 450.0), "the GIF");
    fits(&mut window, screen.id, (2560.0, 1440.0), "the screenshot");
    assert!(!window.shows_named("file:0".to_owned()) && !window.shows_named("file:1".to_owned()));

    // The lists hold both reduced to the size that they draw, the GIF with every frame.
    let (fit_width, fit_height) = list_fit(&mut window);
    for (id, own, frames) in [(moving.id, (800, 450), 6), (screen.id, (2560, 1440), 1)] {
        let ((width, height), count) = held(&mut window, id);
        assert_eq!(count, frames);
        assert!(
            width as f32 <= fit_width.ceil() && height as f32 <= fit_height.ceil(),
            "{width} by {height} is larger than the list draws, {fit_width} by {fit_height}"
        );
        assert!(width < own.0, "the copy of {own:?} is not reduced: {width}");
    }

    let downloads = tempfile::tempdir().unwrap();
    let target = downloads.path().to_path_buf();
    window.in_shell(move |shell, _, cx| {
        shell
            .state()
            .update(cx, |state, _| state.set_download_directory(target));
    });
    click_picture(&mut window, moving.id);
    viewer_shows(
        &mut window,
        "party.gif",
        moving.bytes,
        Some("1 of 2"),
        (800.0, 450.0),
    );
    window.until("the viewer holds the GIF at its own size", |window| {
        window.in_shell(|shell, _, _| shell.viewer_image_size()) == Some((800, 450))
    });
    window.click_named(format!("viewer-download:{}", moving.id));
    let saved = downloads.path().join("party.gif");
    window.until("the GIF is saved", |_| {
        std::fs::read(&saved).is_ok_and(|read| read == party)
    });
    window.press("right");
    viewer_shows(
        &mut window,
        "screen.png",
        screen.bytes,
        Some("2 of 2"),
        (2560.0, 1440.0),
    );
    // The screenshot is read at the size of the place of the viewer, at most its own.
    let device_width = window
        .cx
        .update(|window, _| f32::from(window.viewport_size().width) * window.scale_factor());
    window.until("the viewer holds the screenshot", |window| {
        window
            .in_shell(|shell, _, _| shell.viewer_image_size())
            .is_some_and(|(width, height)| {
                width <= 2560
                    && width as f32 <= device_width
                    && ((width as f32 / height as f32) - 16.0 / 9.0).abs() < 0.02
            })
    });
    window.press("escape");
    assert!(!window.shows("picture-viewer"));
}

/// Moves the clock of the application and gives the windows the frames that it asks for.
fn wait_for(window: &mut Window, milliseconds: u64) {
    window
        .cx
        .executor()
        .advance_clock(std::time::Duration::from_millis(milliseconds));
    window.settle();
}

fn frames(window: &mut Window, id: AttachmentId) -> Vec<usize> {
    let mut frames = window.in_shell(|shell, _, _| shell.shown_frames(id));
    frames.sort_unstable();
    frames
}

/// How often the window was drawn, after it has no work left for a frame of the display:
/// the scrollbars of the list ask for frames for a time after a scroll (ADR 0793).
fn draws(window: &mut Window) -> usize {
    window.until("the window has no work for a frame", |window| {
        window.frame() == 0
    });
    window.in_shell(|shell, _, _| shell.draws().0)
}

/// An animation with frames of 20 ms draws the window at most once in 50 ms, its least
/// interval, and passes over the frames between: each drawing renders the whole window
/// (review M5). It still moves.
#[gpui::test]
fn a_fast_animation_draws_the_window_at_most_once_in_its_least_interval(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let sent = send_files(
        &world,
        &admin,
        PlaceId::Channel(general),
        "Fast",
        vec![("fast.gif", gif(64, 64, 20, 20))],
    );
    let id = sent.attachments[0].id;
    window.until_message("Fast");
    until_picture_read(&mut window, id);
    window.cx.update(|window, _| window.activate_window());
    window.settle();
    let before = draws(&mut window);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..100 {
        wait_for(&mut window, 10);
        seen.extend(frames(&mut window, id));
    }
    let drawn = draws(&mut window) - before;
    assert!(
        drawn <= 22,
        "one second of an animation with frames of 20 ms drew the window {drawn} times"
    );
    assert!(seen.len() >= 5, "the animation showed the frames {seen:?}");
}

/// An animation plays while it is in view in the active window: each frame for its delay,
/// with one drawing of the window at the end of each frame. It stops while the window is
/// not active, with reduced motion unless the pointer is over it, and out of view, where it
/// asks for no drawing; back in view it continues at the frame where it stopped. The
/// viewer plays it too.
#[gpui::test]
fn an_animation_plays_only_in_view_of_the_active_window_without_reduced_motion(
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
        "Moving",
        vec![("moving.gif", gif(64, 64, 4, 100))],
    );
    let id = sent.attachments[0].id;
    window.until_message("Moving");
    until_picture_read(&mut window, id);
    window.settle();
    assert_eq!(frames(&mut window, id), [0]);

    // The window is not active: the animation stays at its first frame.
    let before = draws(&mut window);
    wait_for(&mut window, 300);
    assert_eq!(frames(&mut window, id), [0]);
    assert_eq!(
        draws(&mut window),
        before,
        "a window that is not active was drawn"
    );

    window.cx.update(|window, _| window.activate_window());
    window.settle();
    for frame in [0, 1, 2, 3, 0, 1] {
        assert_eq!(frames(&mut window, id), [frame]);
        let before = draws(&mut window);
        wait_for(&mut window, 99);
        assert_eq!(
            frames(&mut window, id),
            [frame],
            "a frame ended before its delay"
        );
        assert_eq!(
            draws(&mut window),
            before,
            "the window was drawn within a frame"
        );
        wait_for(&mut window, 1);
        assert!(
            draws(&mut window) > before,
            "the end of a frame drew nothing"
        );
    }
    assert_eq!(frames(&mut window, id), [2]);

    // With reduced motion the animation stops, and plays while the pointer is over it.
    window.cx.update(|_, cx| cx.set_reduce_motion(true));
    window.settle();
    let before = draws(&mut window);
    wait_for(&mut window, 500);
    assert_eq!(frames(&mut window, id), [2]);
    assert_eq!(draws(&mut window), before);
    window.hover_named(format!("picture:{id}"));
    wait_for(&mut window, 100);
    assert_eq!(frames(&mut window, id), [3]);
    window.move_pointer(gpui::point(gpui::px(2.0), gpui::px(2.0)));
    wait_for(&mut window, 500);
    assert_eq!(frames(&mut window, id), [3]);
    window.cx.update(|_, cx| cx.set_reduce_motion(false));
    window.settle();
    wait_for(&mut window, 100);
    assert_eq!(frames(&mut window, id), [0]);

    // Out of view the animation asks for no drawing.
    for number in 0..60 {
        send_files(
            &world,
            &admin,
            PlaceId::Channel(general),
            &format!("Text {number}"),
            Vec::new(),
        );
    }
    window.until_message("Text 59");
    window.until("the picture is out of view", |window| {
        window
            .in_shell(|shell, _, _| shell.shown_frames(id))
            .is_empty()
    });
    wait_for(&mut window, 200);
    assert!(!window.in_shell(|shell, _, _| shell.animation_waits()));
    let before = draws(&mut window);
    wait_for(&mut window, 1_000);
    assert_eq!(
        draws(&mut window),
        before,
        "an animation out of view drew the window"
    );
    let picture = selector(format!("picture:{id}"));
    scroll_until(
        &mut window,
        "the picture is in view again",
        300.0,
        |window| {
            let list = window.cx.debug_bounds("message-list").unwrap();
            window
                .cx
                .debug_bounds(picture)
                .is_some_and(|shown| shown.top() >= list.top() && shown.bottom() <= list.bottom())
        },
    );
    assert_eq!(frames(&mut window, id), [0]);
    wait_for(&mut window, 100);
    assert_eq!(frames(&mut window, id), [1]);

    // The viewer plays the animation.
    click_picture(&mut window, id);
    assert!(window.shows("picture-viewer"));
    window.until("the viewer draws the animation", |window| {
        window.in_shell(|shell, _, _| shell.shown_frames(id)).len() == 2
    });
    let shown = frames(&mut window, id);
    wait_for(&mut window, 100);
    let next = frames(&mut window, id);
    assert!(
        next.len() == 2 && next != shown,
        "the viewer does not play the animation: {shown:?} then {next:?}"
    );
}

/// Turns the wheel over the message list by `delta` pixels until `done` holds.
fn scroll_until(
    window: &mut Window,
    what: &str,
    delta: f32,
    mut done: impl FnMut(&mut Window) -> bool,
) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
    window.settle();
    let area = window.cx.debug_bounds("message-list").unwrap();
    window.until(what, |window| {
        let reached = done(window);
        if !reached {
            window.dispatch(ScrollWheelEvent {
                position: area.center(),
                delta: ScrollDelta::Pixels(point(px(0.0), px(delta))),
                ..Default::default()
            });
        }
        reached
    });
}

/// The bytes of the held copies of these files whose pictures lie within the message list
/// or the 800 pixels above and below it that the list lays out (the pictures that a
/// drawing asks for).
fn bytes_near_view(window: &mut Window, ids: &[(AttachmentId, bool)]) -> u64 {
    let list = window.cx.debug_bounds("message-list").unwrap();
    let near = gpui::Bounds {
        origin: gpui::point(list.left(), list.top() - gpui::px(800.0)),
        size: gpui::size(list.size.width, list.size.height + gpui::px(1600.0)),
    };
    let mut bytes = 0;
    for (id, _) in ids {
        let shown = window
            .cx
            .debug_bounds(selector(format!("picture:{id}")))
            .is_some_and(|picture| picture.intersects(&near));
        if let Some(((width, height), frames)) = window
            .in_shell(|shell, _, _| shell.held_picture(*id))
            .filter(|_| shown)
        {
            bytes += u64::from(width) * u64::from(height) * 4 * frames as u64;
        }
    }
    bytes
}

/// A list with 40 screenshots of 2560 by 1440 pixels and 10 GIFs of 498 by 280 pixels with
/// 30 frames is scrolled from its first picture to its last. Each picture is held at the
/// size that the list draws, and the window holds at most 96 MiB of pictures besides the
/// ones that the list lays out. The test prints the bytes that the window holds at the end
/// and at most, and the bytes of the pictures at their own size.
#[gpui::test]
fn many_large_pictures_are_held_at_the_size_that_the_list_draws(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let screen = png(2560, 1440);
    let party = gif(498, 280, 30, 50);
    let mut ids = Vec::new();
    for message in 0..5 {
        let mut files = Vec::new();
        for _ in 0..8 {
            files.push(("screen.png", screen.clone()));
        }
        for _ in 0..2 {
            files.push(("party.gif", party.clone()));
        }
        let sent = send_files(
            &world,
            &admin,
            PlaceId::Channel(general),
            &format!("Pictures {message}"),
            files,
        );
        ids.extend(
            sent.attachments
                .iter()
                .map(|file| (file.id, file.frames.is_some())),
        );
    }
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0))));
    window.until_message("Pictures 4");
    // The list goes to its first row at once: a scroll there with the wheel decodes every
    // picture that it passes in one wait, on the thread of the test, which took longer than
    // the wait under load. The loop below brings each picture into view with the wheel.
    let first = selector(format!("picture:{}", ids[0].0));
    window.in_shell(|shell, _, cx| shell.show_feed_row(Pane::Main, 0, cx));
    window.until("the first picture is in view", |window| {
        let list = window.cx.debug_bounds("message-list").unwrap();
        window
            .cx
            .debug_bounds(first)
            .is_some_and(|picture| picture.top() >= list.top())
    });
    const MOST_HELD_BYTES: u64 = 96 * 1024 * 1024;
    let (fit_width, fit_height) = list_fit(&mut window);
    let mut most = 0;
    for (id, moving) in &ids {
        window.wheel_to("message-list", selector(format!("picture:{id}")));
        until_picture_read(&mut window, *id);
        let ((width, height), count) = held(&mut window, *id);
        assert!(width as f32 <= fit_width.ceil() && height as f32 <= fit_height.ceil());
        assert_eq!(count, if *moving { 30 } else { 1 });
        let held_bytes = window.in_shell(|shell, _, _| shell.message_picture_bytes());
        let near = bytes_near_view(&mut window, &ids);
        assert!(
            held_bytes <= MOST_HELD_BYTES + near,
            "the window holds {held_bytes} bytes, {near} of them near the view"
        );
        most = most.max(held_bytes);
    }
    let held_bytes = window.in_shell(|shell, _, _| shell.message_picture_bytes());
    let own: u64 = 40 * 2560 * 1440 * 4 + 10 * 498 * 280 * 4 * 30;
    eprintln!(
        "held pictures at a display scale of {}: {held_bytes} bytes after the scroll, at most \
         {most} bytes; the pictures at their own size: {own} bytes",
        window.cx.update(|window, _| window.scale_factor())
    );
    assert!(most <= own / 4, "{most} of {own}");
}
