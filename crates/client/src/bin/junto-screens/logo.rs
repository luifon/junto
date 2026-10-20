//! The screens of the logo of a server (ADR 0101): the server book with a server that has
//! a logo beside one without, and the page "Server" of the administration with its states
//! and the limits of the files of messages (ADR 0408).

use junto_client::shell::Shell;
use junto_client::shell::admin::Mode;
use junto_client::shell::admin::server::Limit;
use junto_client::state::admin::{AdminPage, Ask, AskKind, AskState};
use junto_client::state::view::ServerSnapshot;
use junto_client_core::model::{Collection, ServerModel};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::protocol::message::AttachmentLimits;

use super::admin::{open_page, refused, servers, tell};
use super::{Screen, Studio, id, two_servers};

/// The side of the logo of a screen in pixels.
const SIDE: u32 = 160;
const GROUND: [u8; 3] = [0x2F, 0x4B, 0x3C];
const RING: [u8; 3] = [0xEB, 0xD9, 0xBC];

/// The logo of "Acme Studio" on a screen: a ring on a ground of one colour, drawn by the
/// program. No file of a server is part of the tool.
fn mark() -> Vec<u8> {
    let side = SIDE as f32;
    let drawn = image::RgbImage::from_fn(SIDE, SIDE, |x, y| {
        let (dx, dy) = (x as f32 - side / 2.0, y as f32 - side / 2.0);
        let distance = (dx * dx + dy * dy).sqrt() / side;
        image::Rgb(if (0.22..=0.36).contains(&distance) {
            RING
        } else {
            GROUND
        })
    });
    let mut bytes = Vec::new();
    // The logo is made of fixed values: an encoding that fails is a fault of the tool.
    if let Err(error) = drawn.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    ) {
        panic!("the logo of a screen was not encoded: {error}");
    }
    bytes
}

fn acme_id() -> ServerId {
    id(1)
}

/// Luca owns "Acme Studio", so the administration has the page "Server". The server
/// takes files of up to 100 MB, 10 in a message.
fn owned() -> Vec<ServerSnapshot> {
    servers(|model: &mut ServerModel| {
        model.account.is_owner = true;
        std::sync::Arc::make_mut(&mut model.admin).attachment_limits =
            Collection::Loaded(AttachmentLimits {
                bytes_per_file: 100_000_000,
                files_per_message: 10,
            });
    })
}

/// Shows the logo of "Acme Studio", and then what `then` opens. "Harbor" has no logo.
fn with_logo(
    then: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static,
) -> impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static {
    move |shell, window, cx| {
        let image = mark();
        shell.state().clone().update(cx, |state, cx| {
            state.show_logo_fixture(acme_id(), &image, cx);
        });
        then(shell, window, cx);
    }
}

fn open_server_page(shell: &mut Shell, window: &mut gpui::Window, cx: &mut gpui::Context<Shell>) {
    open_page(shell, AdminPage::Server, window, cx);
}

pub fn draw(studio: &mut Studio) -> Result<(), String> {
    // The server book: the logo in place of the generated picture of the server.
    studio.draw(
        Screen::new("logo-book", two_servers()),
        with_logo(|_, _, _| {}),
    )?;
    studio.draw(
        Screen::new("logo-book-collapsed", two_servers()),
        with_logo(|shell, _, cx| {
            shell
                .state()
                .clone()
                .update(cx, |state, cx| state.set_server_book_collapsed(true, cx));
        }),
    )?;

    // The page "Server" of the administration.
    studio.draw(Screen::new("logo-page-none", owned()), open_server_page)?;
    studio.draw(
        Screen::new("logo-page", owned()),
        with_logo(open_server_page),
    )?;
    studio.draw(
        Screen::new("logo-page-remove", owned()),
        with_logo(|shell, window, cx| {
            open_server_page(shell, window, cx);
            shell.set_admin_mode(Some(Mode::RemoveLogo), "", window, cx);
        }),
    )?;
    studio.draw(
        Screen::new("logo-page-sending", owned()),
        |shell, window, cx| {
            open_server_page(shell, window, cx);
            let sending = Ask {
                what: AskKind::SetLogo,
                state: AskState::Sending,
            };
            tell(shell, Some(sending), None, cx);
        },
    )?;
    // The server refused an image that is wider than 1024 pixels.
    studio.draw(
        Screen::new("logo-page-refused", owned()),
        with_logo(|shell, window, cx| {
            open_server_page(shell, window, cx);
            let what = AskKind::SetLogo;
            tell(shell, refused(what, ErrorCode::ImageUnsupported), None, cx);
        }),
    )?;

    // The form of the largest file of a message, with a number outside the range.
    studio.draw(
        Screen::new("limits-page-form", owned()),
        with_logo(|shell, window, cx| {
            open_server_page(shell, window, cx);
            shell.set_admin_mode(Some(Mode::Limit(Limit::FileSize)), "", window, cx);
            shell.type_admin_value("2000", window, cx);
            shell.submit_admin_form(window, cx);
        }),
    )?;
    // The server refused the limits.
    studio.draw(
        Screen::new("limits-page-refused", owned()),
        with_logo(|shell, window, cx| {
            open_server_page(shell, window, cx);
            shell.set_admin_mode(Some(Mode::Limit(Limit::FilesPerMessage)), "", window, cx);
            let what = AskKind::AttachmentLimits;
            tell(shell, refused(what, ErrorCode::InvalidRequest), None, cx);
        }),
    )
}
