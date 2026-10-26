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
