//! The screens of the account layer (ADR 0781) and of device linking (ADR 0782): the
//! profile and the devices of an account with their forms, questions and refusals, the
//! button that links a new device, a device that waits for its link, and both roles of
//! the link with one code.

use std::sync::Arc;

use junto_client::shell::account::Mode;
use junto_client::shell::{Open, Shell};
use junto_client::state::account::{AccountPage, Ask, AskKind, AskState};
use junto_client::state::linking::{
    ApprovalLine, ApprovalShown, ChoiceLine, FirstDeviceView, FirstStep, LinkAllView, LinkLine,
    LinkRole, LinkShown, LinkWait, NewDeviceView, NewStep,
};
use junto_client::state::view::ServerSnapshot;
use junto_client::text;
use junto_client_core::api::{ApiError, Rejection};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::AccessState;
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::account::{Avatar, Device, DeviceOrigin, ImageType};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::keys::PublicKey;
use junto_client_core::protocol::secret::LinkCode;
use time::Duration;

use super::{
    Screen, Studio, acme, at, harbor, harbor_as, id, reachable, select_harbor, two_servers,
};

const LAPTOP: u32 = 300;
const PHONE: u32 = 301;
const TABLET: u32 = 302;

/// The key of the new device of the screens.
fn new_key() -> PublicKey {
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
    }
    PublicKey::from_bytes(bytes)
}

fn device(number: u32, name: &str, origin: DeviceOrigin, days: i64, current: bool) -> Device {
    Device {
        id: id(number),
        name: name.to_owned(),
        public_key: PublicKey::from_bytes([number as u8; 32]),
        origin,
        created_at: at() - Duration::days(days),
        last_login_at: (number != TABLET).then(|| at() - Duration::hours(days)),
        current,
    }
}

fn devices() -> Vec<Device> {
    vec![
        device(LAPTOP, "Junto on macOS", DeviceOrigin::Invite, 90, true),
        device(PHONE, "Phone", DeviceOrigin::Link, 12, false),
        device(TABLET, "", DeviceOrigin::Recovery, 2, false),
    ]
}

/// "Acme Studio" with a picture of the account of Luca.
fn with_picture() -> Vec<ServerSnapshot> {
    let mut server = acme();
    if let Some(model) = &server.model {
        let mut model = ServerModel::clone(model);
        model.account.avatar = Some(Avatar {
            hash: "5e".repeat(32),
            media_type: ImageType::Png,
            bytes: 84 * 1024,
            width: 512,
            height: 512,
        });
        server.model = Some(Arc::new(model));
    }
    vec![server, harbor()]
}

fn refused(what: AskKind, code: ErrorCode) -> Ask {
    let error = ApiError::Rejected(Rejection {
        code,
        status: code.http_status(),
        retry_after: Some(std::time::Duration::from_secs(40)),
        message: String::new(),
        left: None,
    });
    Ask {
        what,
        state: AskState::Refused(text::account::refusal(what, &error)),
    }
}

/// Opens the devices page with these devices and this request.
fn open_devices(
    shell: &mut Shell,
    listed: Result<Vec<Device>, String>,
    ask: Option<Ask>,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    shell.open_account(AccountPage::Devices, window, cx);
    shell.state().clone().update(cx, |state, cx| {
        state.show_account_fixture(Some(listed), ask, cx);
    });
}

fn draw_profile(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new("account-menu", two_servers()),
        |shell, window, cx| shell.open(Open::ServerMenu, window, cx),
    )?;
    studio.draw(
        Screen::new("account-profile", two_servers()),
        |shell, window, cx| shell.open_account(AccountPage::Profile, window, cx),
    )?;
    studio.draw(
        Screen::new("account-profile-picture", with_picture()),
        |shell, window, cx| shell.open_account(AccountPage::Profile, window, cx),
    )?;
    studio.draw(
        Screen::new("account-profile-150", with_picture()).scale(150),
        |shell, window, cx| shell.open_account(AccountPage::Profile, window, cx),
    )?;
    let refusals: [(&'static str, Ask); 4] = [
        (
            "account-profile-picture-too-large",
            Ask {
                what: AskKind::SetPicture,
                state: AskState::Refused(
                    text::account::invalid_picture(
                        junto_client_core::profile::InvalidAvatar::TooLarge,
                    )
                    .to_owned(),
                ),
            },
        ),
        (
            "account-profile-picture-refused",
            refused(AskKind::SetPicture, ErrorCode::ImageUnsupported),
        ),
        (
            "account-profile-name-limited",
            refused(AskKind::SaveName, ErrorCode::RateLimited),
        ),
        (
            "account-profile-picture-sending",
            Ask {
                what: AskKind::SetPicture,
                state: AskState::Sending,
            },
        ),
    ];
    // The cut of a chosen picture over the account page (ADR 0119): at its largest square,
    // and zoomed in.
    for (name, zoom) in [
        ("account-picture-crop", 1.0),
        ("account-picture-crop-zoomed", 2.5),
    ] {
        studio.draw(
            Screen::new(name, with_picture()),
            move |shell, window, cx| {
                shell.open_account(AccountPage::Profile, window, cx);
                let photo = crop_photo();
                shell.state().clone().update(cx, |state, cx| {
                    state.show_picture_crop_fixture(
                        junto_client::state::picture_crop::PicturePurpose::Account,
                        &photo,
                        zoom,
                        cx,
                    );
                });
            },
        )?;
    }
    for (name, ask) in refusals {
        studio.draw(
            Screen::new(name, with_picture()),
            move |shell, window, cx| {
                shell.open_account(AccountPage::Profile, window, cx);
                if ask.what == AskKind::SaveName {
                    shell.type_account_name("Luca Moreira", window, cx);
                }
                shell.state().clone().update(cx, |state, cx| {
                    state.show_account_fixture(None, Some(ask), cx);
                });
            },
        )?;
    }
    Ok(())
}

fn draw_devices(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new("account-devices", two_servers()),
        move |shell, window, cx| open_devices(shell, Ok(devices()), None, window, cx),
    )?;
    studio.draw(
        Screen::new("account-devices-150", two_servers()).scale(150),
        move |shell, window, cx| open_devices(shell, Ok(devices()), None, window, cx),
    )?;
    studio.draw(
        Screen::new("account-devices-reading", two_servers()),
        |shell, window, cx| shell.open_account(AccountPage::Devices, window, cx),
    )?;
    studio.draw(
        Screen::new("account-devices-not-read", two_servers()),
        move |shell, window, cx| {
            let failed = text::flow_error(&FlowError::Unavailable);
            open_devices(shell, Err(failed), None, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-devices-only-one", two_servers()),
        move |shell, window, cx| {
            let only = devices().into_iter().take(1).collect();
            open_devices(shell, Ok(only), None, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-rename", two_servers()),
        move |shell, window, cx| {
            open_devices(shell, Ok(devices()), None, window, cx);
            let mode = Mode::RenameDevice(id(PHONE));
            shell.set_account_mode(Some(mode), "Phone", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-remove-question", two_servers()),
        move |shell, window, cx| {
            open_devices(shell, Ok(devices()), None, window, cx);
            shell.set_account_mode(Some(Mode::RemoveDevice(id(PHONE))), "", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-remove-this-device", two_servers()),
        move |shell, window, cx| {
            open_devices(shell, Ok(devices()), None, window, cx);
            shell.set_account_mode(Some(Mode::RemoveDevice(id(LAPTOP))), "", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-remove-refused", two_servers()),
        move |shell, window, cx| {
            let only = devices().into_iter().take(1).collect();
            let ask = refused(AskKind::RemoveDevice(id(LAPTOP)), ErrorCode::Conflict);
            open_devices(shell, Ok(only), Some(ask), window, cx);
        },
    )?;
    Ok(())
}

/// "Harbor" while this device waits for its link there.
fn waiting_for_link(error: Option<FlowError>) -> ServerSnapshot {
    harbor_as(
        AccessState::WaitingForLink {
            code: LinkCode::new("K7QM-2XHD"),
            public_key: new_key(),
            expires_at: at() + Duration::minutes(9),
        },
        reachable(),
        error,
    )
}

fn draw_one_server(studio: &mut Studio) -> Result<(), String> {
    studio.draw_after(
        Screen::new("join-three-ways", vec![acme()]),
        Box::new(|shell, window, cx| {
            shell.type_address("harbor.example", window, cx);
            shell.submit_address(window, cx);
        }),
        Shell::join_is_open,
    )?;
    studio.draw(
        Screen::new(
            "join-waiting-for-link",
            vec![acme(), waiting_for_link(None)],
        ),
        |shell, window, cx| {
            select_harbor(shell, window, cx);
            shell.open_join_for(id(2), window, cx);
        },
    )?;
    studio.draw(
        Screen::new(
            "server-waiting-for-link",
            vec![acme(), waiting_for_link(None)],
        ),
        select_harbor,
    )?;
    studio.draw(
        Screen::new(
            "server-waiting-for-link-150",
            vec![acme(), waiting_for_link(None)],
        )
        .scale(150),
        select_harbor,
    )?;
    studio.draw(
        Screen::new(
            "server-waiting-for-link-new-code-refused",
            vec![
                acme(),
                waiting_for_link(Some(FlowError::refused(ErrorCode::DeviceLinksFull))),
            ],
        ),
        select_harbor,
    )?;
    Ok(())
}

fn first(step: FirstStep, busy: bool, error: Option<FlowError>) -> LinkAllView {
    LinkAllView::First(FirstDeviceView {
        step,
        busy,
        error: error.as_ref().map(text::account::link_error),
    })
}

fn choice(number: u32, server: &str, checked: bool) -> ChoiceLine {
    ChoiceLine {
        server_id: id(number),
        server: server.to_owned(),
        checked,
        locked: false,
        unavailable: None,
    }
}

fn choices() -> Vec<ChoiceLine> {
    let mut pier = choice(3, "Pier", false);
    pier.unavailable = Some(text::account::link_error(&FlowError::NotLoggedIn));
    vec![
        choice(1, "Acme Studio", true),
        choice(2, "Harbor", true),
        pier,
    ]
}

fn approval(number: u32, server: &str, progress: ApprovalShown) -> ApprovalLine {
    ApprovalLine {
        server_id: id(number),
        server: server.to_owned(),
        account: "Luca".to_owned(),
        progress,
    }
}

fn linking(lines: Vec<ApprovalLine>) -> FirstStep {
    FirstStep::Linking {
        device_name: "Junto on Linux".to_owned(),
        done: !lines.iter().any(|line| {
            matches!(
                line.progress,
                ApprovalShown::Waiting | ApprovalShown::Working
            )
        }),
        lines,
    }
}

fn new_device(step: NewStep, busy: bool, error: Option<FlowError>) -> LinkAllView {
    LinkAllView::New(NewDeviceView {
        step,
        busy,
        error: error.as_ref().map(text::account::link_error),
    })
}

fn link_line(number: u32, server: &str, progress: LinkShown) -> LinkLine {
    LinkLine {
        server_id: id(number),
        server: server.to_owned(),
        progress,
    }
}

fn linked(lines: Vec<LinkLine>, offer_read: bool) -> NewStep {
    NewStep::Linked {
        server: "Acme Studio".to_owned(),
        done: offer_read && lines.iter().all(|line| line.progress == LinkShown::Linked),
        offer_read,
        lines,
    }
}

fn draw_link_device(studio: &mut Studio) -> Result<(), String> {
    let failed = |error: FlowError| text::account::link_error(&error);
    let mut locked = choices();
    locked[1].locked = true;
    let confirm = FirstStep::Confirm {
        device_name: "Junto on Linux".to_owned(),
        device_code: text::account::key_fingerprint(&new_key()),
        choices: locked,
    };
    let first_device: Vec<(&'static str, LinkAllView)> = vec![
        (
            "link-first-code",
            first(FirstStep::EnterCode { choices: choices() }, false, None),
        ),
        (
            "link-first-code-refused",
            first(
                FirstStep::EnterCode { choices: choices() },
                false,
                Some(FlowError::refused(ErrorCode::LinkCodeInvalid)),
            ),
        ),
        ("link-first-confirm", first(confirm, false, None)),
        (
            "link-first-waiting",
            first(
                linking(vec![
                    approval(2, "Harbor", ApprovalShown::Approved),
                    approval(1, "Acme Studio", ApprovalShown::Waiting),
                ]),
                false,
                None,
            ),
        ),
        (
            "link-first-mixed",
            first(
                linking(vec![
                    approval(2, "Harbor", ApprovalShown::Approved),
                    approval(1, "Acme Studio", ApprovalShown::Approved),
                    approval(3, "Pier", ApprovalShown::AlreadyLinked),
                    approval(
                        4,
                        "Quay",
                        ApprovalShown::Failed(failed(FlowError::OnNewDevice(
                            "the server cannot be reached".to_owned(),
                        ))),
                    ),
                ]),
                false,
                Some(FlowError::Incomplete {
                    failed: 1,
                    total: 4,
                }),
            ),
        ),
        (
            "link-first-done",
            first(
                linking(vec![
                    approval(2, "Harbor", ApprovalShown::Approved),
                    approval(1, "Acme Studio", ApprovalShown::Approved),
                ]),
                false,
                None,
            ),
        ),
    ];
    for (name, view) in first_device {
        studio.draw(
            Screen::new(name, two_servers()),
            move |shell, window, cx| {
                shell.open_link_all(LinkRole::FirstDevice, None, window, cx);
                shell.state().clone().update(cx, |state, cx| {
                    state.show_link_all_fixture(view, cx);
                });
            },
        )?;
    }
    let wait = LinkWait {
        code: Some(LinkCode::new("K7QM-2XHD")),
        fingerprint: text::account::key_fingerprint(&new_key()),
        expires_at: at() + Duration::minutes(9),
    };
    let new: Vec<(&'static str, LinkAllView)> = vec![
        (
            "link-new-address",
            new_device(NewStep::EnterAddress, false, None),
        ),
        (
            "link-new-address-refused",
            new_device(NewStep::EnterAddress, false, Some(FlowError::AlreadyLinked)),
        ),
        (
            "link-new-code",
            new_device(
                NewStep::Waiting {
                    server: "Acme Studio".to_owned(),
                    wait: Some(wait),
                },
                false,
                None,
            ),
        ),
        (
            "link-new-reading",
            new_device(linked(Vec::new(), false), true, None),
        ),
        (
            "link-new-mixed",
            new_device(
                linked(
                    vec![
                        link_line(2, "Harbor", LinkShown::Waiting),
                        link_line(3, "pier.example:7443", LinkShown::Working),
                        link_line(
                            4,
                            "quay.example:7443",
                            LinkShown::Failed(failed(FlowError::OtherServer)),
                        ),
                    ],
                    true,
                ),
                false,
                Some(FlowError::Incomplete {
                    failed: 1,
                    total: 3,
                }),
            ),
        ),
        (
            "link-new-done",
            new_device(
                linked(vec![link_line(2, "Harbor", LinkShown::Linked)], true),
                false,
                None,
            ),
        ),
    ];
    for (name, view) in new {
        let typed = name == "link-new-address-refused";
        studio.draw(
            Screen::new(name, two_servers()),
            move |shell, window, cx| {
                shell.open_link_all(LinkRole::NewDevice, None, window, cx);
                if typed {
                    shell.type_link_address("acme.example:7443", window, cx);
                }
                shell.state().clone().update(cx, |state, cx| {
                    state.show_link_all_fixture(view, cx);
                });
            },
        )?;
    }
    studio.draw(
        Screen::new("link-code-too-long", two_servers()),
        |shell, window, cx| {
            shell.open_link_all(LinkRole::FirstDevice, None, window, cx);
            shell.type_link_code(&"A".repeat(17), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("settings-devices", two_servers()),
        |shell, window, cx| shell.open(Open::Settings, window, cx),
    )?;
    Ok(())
}

/// The page that leaves a workspace or deletes the account (ADR 0121), with each question.
fn draw_departure(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new("account-departure", two_servers()),
        |shell, window, cx| shell.open_account(AccountPage::Departure, window, cx),
    )?;
    studio.draw(
        Screen::new("account-departure-leave-question", two_servers()),
        |shell, window, cx| {
            let shown = shell
                .state()
                .read(cx)
                .view()
                .workspace()
                .map(|workspace| workspace.id);
            if let Some(workspace_id) = shown {
                shell.open_departure(workspace_id, window, cx);
            }
        },
    )?;
    studio.draw(
        Screen::new("account-departure-delete-question", two_servers()),
        |shell, window, cx| {
            shell.open_account(AccountPage::Departure, window, cx);
            shell.set_account_mode(Some(Mode::DeleteAccount), "", window, cx);
        },
    )?;
    Ok(())
}

pub fn draw(studio: &mut Studio) -> Result<(), String> {
    draw_profile(studio)?;
    draw_devices(studio)?;
    draw_departure(studio)?;
    draw_one_server(studio)?;
    draw_link_device(studio)
}

/// A photo of 1600 by 1000 pixels with a gradient and a round shape, for the cut.
fn crop_photo() -> Vec<u8> {
    let picture = image::RgbaImage::from_fn(1600, 1000, |x, y| {
        let (dx, dy) = (x as f32 - 800.0, y as f32 - 480.0);
        if dx * dx + dy * dy < 260.0 * 260.0 {
            image::Rgba([196, 120, 72, 255])
        } else {
            image::Rgba([(40 + x / 16) as u8, (70 + y / 12) as u8, 110, 255])
        }
    });
    let mut bytes = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(picture).write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    );
    bytes
}
