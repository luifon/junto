//! The screens of the account layer (ADR 0781) and of device linking (ADR 0782): the
//! profile and the devices of an account with their forms, questions and refusals, the
//! approval of a new device in each state of its flow, a device that waits for its link,
//! and both roles of the link on all servers.

use std::sync::Arc;

use junto_client::shell::account::Mode;
use junto_client::shell::{Open, Shell};
use junto_client::state::account::{AccountPage, Ask, AskKind, AskState, LinkFlowView};
use junto_client::state::linking::{
    ApprovalLine, ApprovalShown, FirstDeviceView, FirstStep, LinkAllView, LinkLine, LinkRole,
    LinkShown, NewDeviceView, NewStep,
};
use junto_client::state::view::ServerSnapshot;
use junto_client::text;
use junto_client_core::api::{ApiError, Rejection};
use junto_client_core::flow::access::AccessState;
use junto_client_core::flow::approve_link::{
    ApproveLinkActionKind, ApproveLinkState, PendingApproval,
};
use junto_client_core::flow::link_all::LinkRequestText;
use junto_client_core::flow::{FlowError, FlowView};
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::account::{Avatar, Device, DeviceOrigin, ImageType};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::keys::PublicKey;
use junto_client_core::protocol::secret::LinkCode;
use time::Duration;

use super::{
    Screen, Studio, acme, at, harbor, harbor_as, id, person, reachable, select_harbor, two_servers,
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

fn link(state: ApproveLinkState) -> LinkFlowView {
    FlowView {
        state,
        in_progress: None,
        actions: Vec::new(),
        error: None,
    }
}

fn pending() -> PendingApproval {
    PendingApproval {
        code: LinkCode::new("K7QM-2XHD"),
        public_key: new_key(),
        device_name: "Junto on Linux".to_owned(),
        expires_at: at() + Duration::minutes(9),
        account_id: person("Luca"),
    }
}

/// Opens the devices page with these devices, this request and this state of the link.
fn open_devices(
    shell: &mut Shell,
    listed: Result<Vec<Device>, String>,
    ask: Option<Ask>,
    link: LinkFlowView,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Shell>,
) {
    shell.open_account(AccountPage::Devices, window, cx);
    shell.state().clone().update(cx, |state, cx| {
        state.show_account_fixture(Some(listed), ask, Some(link), cx);
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
    for (name, ask) in refusals {
        studio.draw(
            Screen::new(name, with_picture()),
            move |shell, window, cx| {
                shell.open_account(AccountPage::Profile, window, cx);
                if ask.what == AskKind::SaveName {
                    shell.type_account_name("Luca Moreira", window, cx);
                }
                shell.state().clone().update(cx, |state, cx| {
                    state.show_account_fixture(None, Some(ask), None, cx);
                });
            },
        )?;
    }
    Ok(())
}

fn draw_devices(studio: &mut Studio) -> Result<(), String> {
    let enter = || link(ApproveLinkState::EnterCode);
    studio.draw(
        Screen::new("account-devices", two_servers()),
        move |shell, window, cx| open_devices(shell, Ok(devices()), None, enter(), window, cx),
    )?;
    studio.draw(
        Screen::new("account-devices-150", two_servers()).scale(150),
        move |shell, window, cx| open_devices(shell, Ok(devices()), None, enter(), window, cx),
    )?;
    studio.draw(
        Screen::new("account-devices-reading", two_servers()),
        |shell, window, cx| shell.open_account(AccountPage::Devices, window, cx),
    )?;
    studio.draw(
        Screen::new("account-devices-not-read", two_servers()),
        move |shell, window, cx| {
            let failed = text::flow_error(&FlowError::Unavailable);
            open_devices(shell, Err(failed), None, enter(), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-devices-only-one", two_servers()),
        move |shell, window, cx| {
            let only = devices().into_iter().take(1).collect();
            open_devices(shell, Ok(only), None, enter(), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-rename", two_servers()),
        move |shell, window, cx| {
            open_devices(shell, Ok(devices()), None, enter(), window, cx);
            let mode = Mode::RenameDevice(id(PHONE));
            shell.set_account_mode(Some(mode), "Phone", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-remove-question", two_servers()),
        move |shell, window, cx| {
            open_devices(shell, Ok(devices()), None, enter(), window, cx);
            shell.set_account_mode(Some(Mode::RemoveDevice(id(PHONE))), "", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-remove-this-device", two_servers()),
        move |shell, window, cx| {
            open_devices(shell, Ok(devices()), None, enter(), window, cx);
            shell.set_account_mode(Some(Mode::RemoveDevice(id(LAPTOP))), "", window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-device-remove-refused", two_servers()),
        move |shell, window, cx| {
            let only = devices().into_iter().take(1).collect();
            let ask = refused(AskKind::RemoveDevice(id(LAPTOP)), ErrorCode::Conflict);
            open_devices(shell, Ok(only), Some(ask), enter(), window, cx);
        },
    )?;
    Ok(())
}

fn draw_link(studio: &mut Studio) -> Result<(), String> {
    studio.draw(
        Screen::new("account-link-lookup", two_servers()),
        |shell, window, cx| {
            let mut looking = link(ApproveLinkState::EnterCode);
            looking.in_progress = Some(ApproveLinkActionKind::LookUp);
            open_devices(shell, Ok(devices()), None, looking, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-link-code-refused", two_servers()),
        |shell, window, cx| {
            let mut refused = link(ApproveLinkState::EnterCode);
            refused.error = Some(FlowError::refused(ErrorCode::LinkCodeInvalid));
            open_devices(shell, Ok(devices()), None, refused, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-link-confirm", two_servers()),
        |shell, window, cx| {
            let confirm = link(ApproveLinkState::Confirm(pending()));
            open_devices(shell, Ok(devices()), None, confirm, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-link-approved", two_servers()),
        |shell, window, cx| {
            let mut listed = devices();
            let linked = device(303, "Junto on Linux", DeviceOrigin::Link, 0, false);
            listed.push(linked.clone());
            let approved = link(ApproveLinkState::Approved { device: linked });
            open_devices(shell, Ok(listed), None, approved, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("account-link-outcome-unknown", two_servers()),
        |shell, window, cx| {
            let mut unknown = link(ApproveLinkState::OutcomeUnknown(pending()));
            unknown.error = Some(FlowError::Unavailable);
            open_devices(shell, Ok(devices()), None, unknown, window, cx);
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

fn offer() -> String {
    "junto-link-offer-1.eyJzZXJ2ZXJzIjpbeyJzZXJ2ZXJfaWQiOiIwMTk4ZjFjMi03ZDNhLTdiMWUtOWM1NS0wMD\
     AwMDAwMDAwMDEiLCJhZGRyZXNzIjoiaHR0cHM6Ly9hY21lLmV4YW1wbGU6NzQ0MyJ9LHsic2VydmVyX2lkIjoiMDE5\
     OGYxYzItN2QzYS03YjFlLTljNTUtMDAwMDAwMDAwMDAyIiwiYWRkcmVzcyI6Imh0dHBzOi8vaGFyYm9yLmV4YW1wbG\
     U6NzQ0MyJ9XX0"
        .to_owned()
}

fn request() -> LinkRequestText {
    LinkRequestText::new(
        "junto-link-request-1.eyJkZXZpY2VfbmFtZSI6Ikp1bnRvIG9uIExpbnV4IiwibGlua3MiOlt7InNlcnZlcl9p\
         ZCI6IjAxOThmMWMyLTdkM2EtN2IxZS05YzU1LTAwMDAwMDAwMDAwMSIsImNvZGUiOiJLN1FNLTJYSEQiLCJwdWJsaW\
         Nfa2V5IjoiQ3pCVmVwX0U2UTR6V0gyaXgtd1JObHVBcGNydkZEbGVnNmpOOGhjOFlZWSJ9LHsic2VydmVyX2lkIjoi\
         MDE5OGYxYzItN2QzYS03YjFlLTljNTUtMDAwMDAwMDAwMDAyIiwiY29kZSI6IlA0VFctOE5LRSIsInB1YmxpY19rZX\
         kiOiJrTFhhXzhEcEM5VHdGVHBmaE0yNTdOb0xCajBBWWpXd3FybWFTRDNhNjdrIn1dfQ"
            .to_owned()
            .into(),
    )
}

fn first(step: FirstStep, busy: bool, error: Option<FlowError>) -> LinkAllView {
    LinkAllView::First(FirstDeviceView {
        offer: Some(offer()),
        offered: vec!["Acme Studio".to_owned(), "Harbor".to_owned()],
        step,
        busy,
        error: error.as_ref().map(text::account::link_error),
    })
}

fn approval(number: u32, server: &str, progress: ApprovalShown) -> ApprovalLine {
    ApprovalLine {
        server_id: id(number),
        server: server.to_owned(),
        account: (number != 4).then(|| "Luca".to_owned()),
        progress,
    }
}

fn confirm(lines: Vec<ApprovalLine>) -> FirstStep {
    FirstStep::Confirm {
        fingerprint: "7c41-0be2-95a6-d3f8".to_owned(),
        device_name: "Junto on Linux".to_owned(),
        can_approve: lines.iter().any(|line| {
            matches!(
                line.progress,
                ApprovalShown::Ready | ApprovalShown::Unknown | ApprovalShown::Failed(_)
            )
        }),
        all_approved: lines
            .iter()
            .all(|line| line.progress == ApprovalShown::Approved),
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

fn requested(lines: Vec<LinkLine>) -> NewStep {
    let waits = lines.iter().any(|line| line.progress == LinkShown::Waiting);
    NewStep::Requested {
        device_name: "Junto on Linux".to_owned(),
        request: waits.then(request),
        fingerprint: waits.then(|| "7c41-0be2-95a6-d3f8".to_owned()),
        all_linked: lines.iter().all(|line| line.progress == LinkShown::Linked),
        lines,
    }
}

fn draw_all_servers(studio: &mut Studio) -> Result<(), String> {
    let failed = |error: FlowError| text::account::link_error(&error);
    let first_device: Vec<(&'static str, LinkAllView)> = vec![
        (
            "link-all-first-offer",
            first(FirstStep::EnterRequest, false, None),
        ),
        (
            "link-all-first-request-refused",
            first(
                FirstStep::EnterRequest,
                false,
                Some(FlowError::InvalidBundle("link request")),
            ),
        ),
        (
            "link-all-first-looking-up",
            first(
                confirm(vec![
                    approval(1, "Acme Studio", ApprovalShown::Ready),
                    approval(2, "Harbor", ApprovalShown::Working),
                    approval(3, "Pier", ApprovalShown::Working),
                ]),
                true,
                None,
            ),
        ),
        (
            "link-all-first-confirm",
            first(
                confirm(vec![
                    approval(1, "Acme Studio", ApprovalShown::Ready),
                    approval(2, "Harbor", ApprovalShown::Ready),
                ]),
                false,
                None,
            ),
        ),
        (
            "link-all-first-mixed",
            first(
                confirm(vec![
                    approval(1, "Acme Studio", ApprovalShown::Approved),
                    approval(2, "Harbor", ApprovalShown::Unknown),
                    approval(
                        3,
                        "Pier",
                        ApprovalShown::AccountChanged("Luca M.".to_owned()),
                    ),
                    approval(
                        4,
                        "",
                        ApprovalShown::Failed(failed(FlowError::UnknownServer)),
                    ),
                    approval(
                        5,
                        "Quay",
                        ApprovalShown::Failed(failed(FlowError::refused(
                            ErrorCode::LinkCodeInvalid,
                        ))),
                    ),
                ]),
                false,
                Some(FlowError::Incomplete {
                    failed: 4,
                    total: 5,
                }),
            ),
        ),
        (
            "link-all-first-approved",
            first(
                confirm(vec![
                    approval(1, "Acme Studio", ApprovalShown::Approved),
                    approval(2, "Harbor", ApprovalShown::Approved),
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
                shell.open_link_all(LinkRole::FirstDevice, window, cx);
                shell.state().clone().update(cx, |state, cx| {
                    state.show_link_all_fixture(view, cx);
                });
            },
        )?;
    }
    let new: Vec<(&'static str, LinkAllView)> = vec![
        (
            "link-all-new-offer",
            new_device(NewStep::EnterOffer, false, None),
        ),
        (
            "link-all-new-offer-refused",
            new_device(
                NewStep::EnterOffer,
                false,
                Some(FlowError::InvalidBundle("link offer")),
            ),
        ),
        (
            "link-all-new-asking",
            new_device(
                requested(vec![
                    link_line(1, "Acme Studio", LinkShown::Working),
                    link_line(2, "harbor.example:7443", LinkShown::Working),
                ]),
                true,
                None,
            ),
        ),
        (
            "link-all-new-requested",
            new_device(
                requested(vec![
                    link_line(1, "Acme Studio", LinkShown::Waiting),
                    link_line(2, "Harbor", LinkShown::Waiting),
                ]),
                false,
                None,
            ),
        ),
        (
            "link-all-new-mixed",
            new_device(
                requested(vec![
                    link_line(1, "Acme Studio", LinkShown::Linked),
                    link_line(2, "Harbor", LinkShown::Waiting),
                    link_line(3, "pier.example:7443", LinkShown::Working),
                    link_line(
                        4,
                        "quay.example:7443",
                        LinkShown::Failed(failed(FlowError::OtherServer)),
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
            "link-all-new-linked",
            new_device(
                requested(vec![
                    link_line(1, "Acme Studio", LinkShown::Linked),
                    link_line(2, "Harbor", LinkShown::Linked),
                ]),
                false,
                None,
            ),
        ),
    ];
    for (name, view) in new {
        let offers = name == "link-all-new-offer-refused";
        studio.draw(
            Screen::new(name, two_servers()),
            move |shell, window, cx| {
                shell.open_link_all(LinkRole::NewDevice, window, cx);
                if offers {
                    shell.type_link_offer("junto-link-request-1.eyJkZXZpY2Vf", window, cx);
                }
                shell.state().clone().update(cx, |state, cx| {
                    state.show_link_all_fixture(view, cx);
                });
            },
        )?;
    }
    studio.draw(
        Screen::new("link-all-text-too-long", two_servers()),
        |shell, window, cx| {
            shell.open_link_all(LinkRole::NewDevice, window, cx);
            shell.type_link_offer(&"a".repeat(32 * 1024 + 1), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("settings-devices", two_servers()),
        |shell, window, cx| shell.open(Open::Settings, window, cx),
    )?;
    Ok(())
}

pub fn draw(studio: &mut Studio) -> Result<(), String> {
    draw_profile(studio)?;
    draw_devices(studio)?;
    draw_link(studio)?;
    draw_one_server(studio)?;
    draw_all_servers(studio)
}
