//! The screens of the logo of a server (ADR 0101): the server book with a server that has
//! a logo beside one without, and the page "Server" of the administration with its states
//! and the limits of the files of messages (ADR 0408). The screens of the logos of the
//! workspaces (ADR 0122): the workspace lists of the server book and of the menu of a
//! server, and the page of the workspaces of the administration.

use junto_client::shell::Shell;
use junto_client::shell::admin::Mode;
use junto_client::shell::admin::server::Limit;
use junto_client::state::admin::{AdminPage, Ask, AskKind, AskState};
use junto_client::state::view::ServerSnapshot;
use junto_client_core::model::{Collection, ServerModel};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};
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

/// The outer bands of the wide logo of a workspace: colours that no theme uses.
const SPILL: [[u8; 3]; 2] = [[0x00, 0xFF, 0x00], [0xFF, 0x00, 0xFF]];

/// A logo of a workspace that is three times as wide as high: a green band, a beige band
/// of 160 of its 360 pixels in the middle, and a magenta band. The square of the row shows
/// the middle third of a covering image, 120 pixels, which lies inside the beige band.
fn wide_mark() -> Vec<u8> {
    let drawn = image::RgbImage::from_fn(360, 120, |x, _| {
        image::Rgb(match x {
            0..100 => SPILL[0],
            100..260 => RING,
            _ => SPILL[1],
        })
    });
    let mut bytes = Vec::new();
    if let Err(error) = drawn.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    ) {
        panic!("the logo of a screen was not encoded: {error}");
    }
    bytes
}

/// The square of the row shows the middle of the wide logo and nothing else: no pixel of the
/// screen has the colour of one of its outer bands (ADR 0122). With the cut of the decoder
/// and the cover of the drawing both removed, the check finds them.
fn logo_inside_its_square(image: &image::RgbaImage) -> Option<String> {
    let near = |pixel: &image::Rgba<u8>, colour: [u8; 3]| {
        pixel.0[..3]
            .iter()
            .zip(colour)
            .all(|(got, wanted)| got.abs_diff(wanted) <= 40)
    };
    let spilled = image
        .pixels()
        .filter(|pixel| SPILL.iter().any(|colour| near(pixel, *colour)))
        .count();
    (spilled > 0).then(|| {
        format!(
            "{spilled} pixels of the outer bands of a workspace logo are drawn: the square of \
             the row shows more than the middle of the logo"
        )
    })
}

/// The workspaces "Studio" and "Client Alpha" of "Acme Studio" have logos; "Client Beta"
/// has none.
fn workspaces_with_logos() -> [(WorkspaceId, Vec<u8>); 2] {
    [(id(10), wide_mark()), (id(11), mark())]
}

/// The state of "Acme Studio" states the logos of "Studio" and "Client Alpha", and its
/// owner is Luca.
fn owned_with_workspace_logos() -> Vec<ServerSnapshot> {
    servers(|model: &mut ServerModel| {
        model.account.is_owner = true;
        for workspace in &mut model.workspaces {
            if workspaces_with_logos()
                .iter()
                .any(|(id, _)| *id == workspace.workspace.id)
            {
                std::sync::Arc::make_mut(workspace).workspace.logo =
                    Some(junto_client_core::protocol::account::Avatar {
                        hash: "a".repeat(64),
                        media_type: junto_client_core::protocol::account::ImageType::Png,
                        bytes: 2048,
                        width: 160,
                        height: 160,
                    });
            }
        }
    })
}

/// Shows the logos of the workspaces, and then what `then` opens.
fn with_workspace_logos(
    then: impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static,
) -> impl FnOnce(&mut Shell, &mut gpui::Window, &mut gpui::Context<Shell>) + 'static {
    move |shell, window, cx| {
        shell.state().clone().update(cx, |state, cx| {
            for (workspace_id, image) in workspaces_with_logos() {
                state.show_workspace_logo_fixture(acme_id(), workspace_id, &image, cx);
            }
        });
        then(shell, window, cx);
    }
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

    // The workspace lists with the logos of two workspaces in place of their generated
    // pictures: the logo of "Studio" is wider than its square.
    studio.draw(
        Screen::new("logo-book-workspaces", two_servers()).check(logo_inside_its_square),
        with_workspace_logos(|_, _, _| {}),
    )?;
    // At 200 % the server book is collapsed: the hover menu of the server lists the rows.
    studio.draw(
        Screen::new("logo-book-workspaces-menu", two_servers())
            .scale(200)
            .check(logo_inside_its_square),
        with_workspace_logos(|shell, window, cx| {
            shell.open(junto_client::shell::Open::ServerHoverMenu, window, cx);
        }),
    )?;
    studio.draw(
        Screen::new("logo-workspaces-page", owned_with_workspace_logos()),
        with_workspace_logos(|shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
        }),
    )?;
    studio.draw(
        Screen::new("logo-workspaces-page-remove", owned_with_workspace_logos()),
        with_workspace_logos(|shell, window, cx| {
            open_page(shell, AdminPage::Workspaces, window, cx);
            shell.set_admin_mode(Some(Mode::RemoveWorkspaceLogo(id(10))), "", window, cx);
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
    // The limit was saved: the row keeps its space with the result (ADR 0730).
    studio.draw_answered(
        Screen::new("limits-page-saved", owned()),
        with_logo(|shell, window, cx| {
            open_server_page(shell, window, cx);
            shell.set_admin_mode(Some(Mode::Limit(Limit::FileSize)), "", window, cx);
            shell.type_admin_value("25", window, cx);
        }),
        |shell, _, cx| shell.answer_open_admin_form(cx),
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
