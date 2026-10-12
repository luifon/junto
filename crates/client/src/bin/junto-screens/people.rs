//! The screens of the pictures of people (ADR 0783): a call and a member list with
//! avatars that accounts set, and the picture of the profile preset in the settings, also
//! with a server that refused it.

use std::collections::HashSet;
use std::sync::Arc;

use junto_client::shell::call::CallTab;
use junto_client::shell::{Open, Shell};
use junto_client::state::people::PictureHash;
use junto_client::state::view::ServerSnapshot;
use junto_client_core::protocol::account::{Avatar, ImageType};
use junto_client_core::protocol::ids::ServerId;
use junto_media::calls::Phase;

use super::{
    Screen, Studio, acme, acme_with_luca, harbor, id, in_call, person, show_daily, show_daily_tab,
    two_servers,
};

/// The side of a picture of a screen in pixels.
const SIDE: u32 = 160;

/// A picture that the program draws: a head and shoulders in one colour on a ground of
/// another. No file of a person is part of the tool.
fn picture(ground: [u8; 3], figure: [u8; 3]) -> Vec<u8> {
    let side = SIDE as f32;
    let inside = |x: f32, y: f32, centre: (f32, f32), radii: (f32, f32)| {
        let (dx, dy) = ((x - centre.0) / radii.0, (y - centre.1) / radii.1);
        dx * dx + dy * dy <= 1.0
    };
    let drawn = image::RgbImage::from_fn(SIDE, SIDE, |x, y| {
        let (x, y) = (x as f32, y as f32);
        let head = inside(x, y, (side * 0.5, side * 0.4), (side * 0.19, side * 0.21));
        let shoulders = inside(x, y, (side * 0.5, side * 1.02), (side * 0.38, side * 0.36));
        image::Rgb(if head || shoulders { figure } else { ground })
    });
    let mut bytes = Vec::new();
    // The picture is made of fixed values: an encoding that fails is a fault of the tool.
    if let Err(error) = drawn.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    ) {
        panic!("the picture of a screen was not encoded: {error}");
    }
    bytes
}

fn picture_of(name: &str) -> Option<Vec<u8>> {
    let colours = match name {
        "Luca" => ([0x2F, 0x4B, 0x3C], [0xEB, 0xD9, 0xBC]),
        "Ana" => ([0xC7, 0x9A, 0x45], [0x2A, 0x22, 0x1B]),
        "Bruno" => ([0x3E, 0x63, 0x73], [0xE8, 0xDC, 0xC3]),
        "Marta" => ([0xB4, 0x53, 0x2F], [0xF3, 0xE8, 0xD2]),
        "Elisa" => ([0x6E, 0x30, 0x44], [0xE3, 0xC9, 0xA8]),
        // Carla and Diego have no avatar: the generated one is drawn beside the images.
        _ => return None,
    };
    Some(picture(colours.0, colours.1))
}

fn acme_id() -> ServerId {
    id(1)
}

fn harbor_id() -> ServerId {
    id(2)
}

/// The server with the avatar of Luca in the state of the account, as a server tells the
/// own account.
fn with_own_picture(mut server: ServerSnapshot) -> ServerSnapshot {
    if let (Some(model), Some(bytes)) = (&mut server.model, picture_of("Luca")) {
        Arc::make_mut(model).account.avatar = Some(Avatar {
            hash: PictureHash::of(&bytes).to_string(),
            media_type: ImageType::Png,
            bytes: bytes.len() as u32,
            width: SIDE,
            height: SIDE,
        });
    }
    server
}

/// Shows the profiles of the people of "Acme Studio" with their pictures, and then what
/// `then` opens.
fn with_pictures(
    then: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static,
) -> impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static {
    move |shell, window, cx| {
        let people = super::PEOPLE
            .iter()
            .map(|name| (person(name), (*name).to_owned(), picture_of(name)))
            .collect();
        shell.state().clone().update(cx, |state, cx| {
            state.show_people_fixture(acme_id(), people, cx);
        });
        then(shell, window, cx);
    }
}

pub fn draw(studio: &mut Studio) -> Result<(), String> {
    let in_daily = || vec![with_own_picture(acme_with_luca(|_| {})), harbor()];
    studio.draw(
        Screen::call("people-call", in_call(Phase::Connected)).servers(in_daily()),
        with_pictures(show_daily),
    )?;
    studio.draw(
        Screen::call("people-call-members", in_call(Phase::Connected)).servers(in_daily()),
        with_pictures(show_daily_tab(CallTab::Members)),
    )?;
    studio.draw(
        Screen::new("people-members", vec![with_own_picture(acme()), harbor()]),
        with_pictures(|shell, window, cx| shell.open(Open::Members, window, cx)),
    )?;

    // The settings: the picture of the profile preset, and a server that refused it.
    let preset = picture([0x74, 0x7A, 0x3F], [0xEB, 0xD9, 0xBC]);
    let stored = preset.clone();
    studio.draw(
        Screen::new("people-settings-picture", two_servers()),
        move |shell, window, cx| {
            shell.state().clone().update(cx, |state, cx| {
                state.show_preset_picture_fixture(&stored, cx);
            });
            shell.open(Open::Settings, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("people-settings-refused", two_servers()),
        move |shell, window, cx| {
            shell.state().clone().update(cx, |state, cx| {
                state.show_preset_picture_fixture(&preset, cx);
                state.show_preset_refused_fixture(HashSet::from([harbor_id()]), cx);
            });
            shell.open(Open::Settings, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("people-settings-no-picture", two_servers()),
        |shell, window, cx| shell.open(Open::Settings, window, cx),
    )
}
