//! Device linking in the window against the real server (ADR 0782): a second device on
//! one server through the windows of both devices, and a second installation on two
//! servers with the two texts that the person carries (ADR 0716).

use gpui::TestAppContext;
use junto_client::shell::Open;
use junto_client::state::account::{LinkView, PageView};
use junto_client::state::linking::{
    ApprovalShown, FirstStep, LinkAllView, LinkShown, LinkWait, NewStep,
};
use junto_client::state::view::ServerStatus;
use junto_client::text;
use junto_client_core::book::Membership;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::AccessState;
use junto_client_core::flow::link_all::LONGEST_TEXT;
use junto_client_core::protocol::account::RecoverAccount;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::DeviceId;

use crate::harness::{Window, World, member, named};
use crate::support::{Fault, FaultProxy};

impl Window {
    /// What the selected server shows while this device waits for its link.
    fn link_wait(&mut self) -> Option<LinkWait> {
        let server_id = self.view().selected?.id;
        self.state
            .read_with(&self.cx, |state, _| state.link_wait(server_id))
    }

    /// The link code that the window drew for the selected server.
    fn drawn_code(&mut self) -> Option<String> {
        match self.status() {
            Some(ServerStatus::WaitingForLink { code }) => Some(code),
            _ => None,
        }
    }

    /// The link of a new device on the devices page, as the window drew it last.
    fn link(&mut self) -> Option<LinkView> {
        match self.account()?.page {
            PageView::Devices(page) => page.link,
            PageView::Profile(_) | PageView::Recoveries(_) | PageView::Departure(_) => None,
        }
    }

    /// The link on all servers, as the window drew it last.
    fn link_all(&mut self) -> Option<LinkAllView> {
        self.run_tasks();
        self.shell
            .read_with(&self.cx, |shell, _| shell.link_all_drawn().cloned())
    }

    /// The states of the lines of the new device, in the order of the layer.
    fn link_lines(&mut self) -> Vec<LinkShown> {
        match self.link_all() {
            Some(LinkAllView::New(view)) => match view.step {
                NewStep::Requested { lines, .. } => {
                    lines.into_iter().map(|line| line.progress).collect()
                }
                NewStep::EnterOffer => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// The states of the lines of the first device, in the order of the layer.
    fn approval_lines(&mut self) -> Vec<ApprovalShown> {
        match self.link_all() {
            Some(LinkAllView::First(view)) => match view.step {
                FirstStep::Confirm { lines, .. } => {
                    lines.into_iter().map(|line| line.progress).collect()
                }
                FirstStep::EnterRequest => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn clipboard(&mut self) -> Option<String> {
        self.cx.read_from_clipboard().and_then(|item| item.text())
    }
}

#[gpui::test]
fn a_second_device_is_linked_on_one_server_through_both_windows(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut first) = member(cx, &world, &server.address(), &admin, "Luca");
    let account_id = first.view().person.account_id.unwrap();
    let handle = first.handle();

    // The new device: a first start, the address, and the third way of the dialog.
    let (_new_directory, mut new) = named(new_cx, "Luca");
    new.find_server(&server.address());
    assert!(new.shows("request-link") && new.shows("code-field"));
    new.click("request-link");
    new.until("the dialog shows the link code", |new| {
        new.shows("link-wait") && new.drawn_code().is_some()
    });
    assert!(new.shows("join-dialog"));
    assert!(new.shows("link-wait-code") && new.shows("link-wait-fingerprint"));
    assert!(new.shows("link-wait-expiry"));
    assert!(!new.shows("request-link") && !new.shows("code-field"));
    let first_code = new.drawn_code().unwrap();
    assert_eq!(first_code.len(), 9, "two groups of four symbols");

    // "New code" asks the server again: the code of the first request is replaced.
    new.click("link-wait-new-code");
    new.until("the dialog shows another code", |new| {
        new.drawn_code().is_some_and(|code| code != first_code)
    });
    let code = new.drawn_code().unwrap();

    // The main area of the server shows the same after the dialog is closed.
    new.click("close-join");
    assert!(!new.shows("join-dialog"));
    assert!(new.shows("waiting-for-link") && new.shows("link-wait-code"));
    assert!(new.shows("link-wait-new-code") && new.shows("link-wait-cancel"));
    let wait = new.link_wait().unwrap();
    assert_eq!(
        wait.code.as_ref().map(|shown| shown.expose().to_owned()),
        Some(code.clone())
    );
    assert_eq!(wait.fingerprint.split(' ').count(), 11);

    // The first device: the account layer, the devices, and the code.
    first.open_devices();
    assert_eq!(
        first.link(),
        Some(LinkView::EnterCode {
            looking_up: false,
            error: None
        })
    );
    // An empty field is not sent.
    first.click("link-look-up");
    assert!(first.shows("link-code-invalid"));
    // The code of the first request is not known any more.
    first.in_shell(|shell, window, cx| shell.type_link_code(&first_code, window, cx));
    assert!(!first.shows("link-code-invalid"));
    first.click("link-look-up");
    first.until("the replaced code is refused", |first| {
        first.shows("link-error")
    });
    assert_eq!(
        first.link(),
        Some(LinkView::EnterCode {
            looking_up: false,
            error: Some(text::account::link_error(&FlowError::refused(
                ErrorCode::LinkCodeInvalid
            ))),
        })
    );
    // The field holds the refused code no longer.
    assert!(first.in_shell(|shell, _, cx| shell.link_code_is_empty(cx)));
    first.in_shell(|shell, window, cx| shell.type_link_code(&first_code, window, cx));

    // The layer that closes keeps no code in its field.
    first.press("escape");
    assert!(!first.shows("account"));
    assert!(first.in_shell(|shell, _, cx| shell.link_code_is_empty(cx)));
    first.open_devices();

    // The code as a person types it: without regard to case.
    first.in_shell(|shell, window, cx| shell.type_link_code(&code.to_lowercase(), window, cx));
    first.click("link-look-up");
    first.until("the key of the new device is shown", |first| {
        first.shows("link-confirm")
    });
    let Some(LinkView::Confirm {
        device_name,
        fingerprint,
        ..
    }) = first.link()
    else {
        panic!("the first device shows the key to compare");
    };
    assert_eq!(device_name, text::device_name());
    // The person compares the two texts: they are the same key.
    assert_eq!(fingerprint, wait.fingerprint);
    assert!(first.shows("link-fingerprint") && first.shows("link-device-name"));
    // The flow holds the code now, and the field holds nothing.
    assert!(first.in_shell(|shell, _, cx| shell.link_code_is_empty(cx)));
    // Nothing is approved before the person approves.
    assert_eq!(world.run(handle.api().devices()).unwrap().len(), 1);
    assert_eq!(new.drawn_code(), Some(code));

    first.click("link-approve");
    first.until(
        "the first device tells that the device is linked",
        |first| first.shows("link-approved"),
    );
    assert_eq!(
        first.link(),
        Some(LinkView::Approved {
            device_name: text::device_name()
        })
    );
    first.until("the list holds the new device", |first| {
        first.devices().len() == 2
    });
    assert_eq!(world.run(handle.api().devices()).unwrap().len(), 2);

    // The new device logs in by the check that the application makes by itself.
    new.until_ready();
    assert!(!new.shows("waiting-for-link"));
    let person = new.view().person;
    assert_eq!(person.account_id, Some(account_id));
    assert_eq!(person.name, "Luca");
    assert_eq!(new.channel_names().len(), 2);

    // "Done" returns to the entry of a code.
    first.click("link-done");
    assert!(first.shows("link-code-field") && !first.shows("link-approved"));
}

/// The page of the devices reads its list again at "Try again", and an approval whose
/// answer did not arrive is checked again at "Check again" (ADR 0782).
#[gpui::test]
fn a_device_list_that_was_not_read_and_an_approval_without_an_answer_are_read_again(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    const DEVICES: &str = "/api/v1/account/devices";
    const APPROVE: &str = "/api/v1/account/device-links/approve";
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    // The device of the window reaches the server through a proxy.
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut first) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let handle = first.handle();
    // A request that gets `busy` at each of its attempts fails.
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    let unread = || std::iter::repeat_n(Fault::Busy, attempts);

    // The first read of the list fails: the page states it with "Try again".
    proxy.fail(DEVICES, unread());
    first.open_account();
    first.click("account-nav-devices");
    first.until("the failure of the read is shown", |first| {
        first.shows("devices-read-failed")
    });
    assert!(first.devices().is_empty());
    assert!(first.shows("devices-read-again"));
    let reads = proxy.seen(DEVICES).len();
    first.click("devices-read-again");
    first.until("the list is read", |first| first.devices().len() == 1);
    assert!(proxy.seen(DEVICES).len() > reads);
    assert!(!first.shows("devices-read-failed"));

    // A new device of the account asks for a link, and the first device looks up its code.
    let (_new_directory, mut new) = named(new_cx, "Luca");
    new.find_server(&server.address());
    new.click("request-link");
    new.until("the dialog shows the link code", |new| {
        new.drawn_code().is_some()
    });
    let code = new.drawn_code().unwrap();
    let key = new.link_wait().unwrap().fingerprint;
    first.in_shell(|shell, window, cx| shell.type_link_code(&code, window, cx));
    first.click("link-look-up");
    first.until("the key of the new device is shown", |first| {
        first.shows("link-confirm")
    });

    // The server takes the approval, and its answer is lost. The list of the devices,
    // which would tell whether the key was added, is not read either. The events of the
    // server wait, so that only the approval reads the list.
    proxy.hold_events();
    proxy.fail(APPROVE, [Fault::LoseAnswer]);
    proxy.fail(DEVICES, unread());
    first.click("link-approve");
    first.until("the outcome is not known", |first| {
        first.shows("link-unknown")
    });
    let Some(LinkView::OutcomeUnknown {
        device_name,
        fingerprint,
        checking: false,
        ..
    }) = first.link()
    else {
        panic!("the page states that the outcome is not known");
    };
    assert_eq!(device_name, text::device_name());
    assert_eq!(fingerprint, key);
    assert!(first.shows("link-fingerprint") && first.shows("link-check"));
    assert!(!first.shows("link-approved"));
    assert_eq!(world.run(handle.api().devices()).unwrap().len(), 2);
    proxy.release_events();

    // "Check again" reads the list: the key is there, and the device is linked.
    first.click("link-check");
    first.until(
        "the first device tells that the device is linked",
        |first| first.shows("link-approved"),
    );
    assert_eq!(
        first.link(),
        Some(LinkView::Approved {
            device_name: text::device_name()
        })
    );
    first.until("the list holds the new device", |first| {
        first.devices().len() == 2
    });
    assert_eq!(proxy.seen(APPROVE).len(), 1, "the approval was sent once");
    new.until_ready();
    assert_eq!(new.view().person.account_id, first.view().person.account_id);
}

#[gpui::test]
fn the_wait_for_a_link_is_cancelled_in_the_dialog(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let _admin = world.owner(&server);
    let (_directory, mut window) = named(cx, "Luca");
    window.find_server(&server.address());
    window.click("request-link");
    window.until("the dialog shows the link code", |window| {
        window.shows("link-wait")
    });
    assert_eq!(window.view().servers.len(), 1);

    // "Cancel" forgets the key that asked: the dialog offers the three ways again, and
    // the server stays in the server book without an account.
    window.click("link-wait-cancel");
    window.until("the dialog offers the ways in again", |window| {
        window.shows("request-link") && !window.shows("link-wait")
    });
    assert_eq!(window.status(), Some(ServerStatus::NoAccount(None)));
    assert!(window.shows("code-field") && window.shows("request-access"));
    window.click("dialog-close");
    assert!(window.shows("no-account"));
    assert_eq!(window.view().servers.len(), 1);
}

#[gpui::test]
fn a_second_installation_is_linked_on_two_servers_with_two_texts(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let acme = world.server("Acme Studio");
    let harbor = world.server("Harbor");
    let usable =
        |status: &ServerStatus| matches!(status, ServerStatus::Ready | ServerStatus::Standby);

    // The first device has an account on both servers.
    let (_directory, mut first) = named(cx, "Luca");
    for server in [&acme, &harbor] {
        first.find_server(&server.address());
        first.join_with_code(&server.owner_code().unwrap());
        first.until_ready();
    }
    first.until("the first device is logged in on both servers", |first| {
        let view = first.view();
        view.servers.len() == 2 && view.servers.iter().all(|server| usable(&server.status))
    });
    let handles = first
        .state
        .read_with(&first.cx, |state, _| state.client().servers());
    let devices = |world: &World| -> Vec<usize> {
        handles
            .iter()
            .map(|handle| world.run(handle.api().devices()).unwrap().len())
            .collect()
    };
    assert_eq!(devices(&world), [1, 1]);

    // The settings lead to both roles. The first device shows its offer.
    first.in_shell(|shell, window, cx| shell.open(Open::Settings, window, cx));
    assert!(first.shows("settings-link-first") && first.shows("settings-link-new"));
    let scroll = first.in_shell(|shell, _, _| shell.settings_scroll_handle());
    first.reveal(&scroll, "settings-link-first");
    first.click("settings-link-first");
    assert!(first.shows("link-all") && first.shows("link-offer-text"));
    let Some(LinkAllView::First(shown)) = first.link_all() else {
        panic!("the first device shows its role");
    };
    assert_eq!(shown.offered, ["Acme Studio", "Harbor"]);
    assert_eq!(shown.step, FirstStep::EnterRequest);
    let offer = shown.offer.unwrap();
    assert!(offer.starts_with("junto-link-offer-1."));
    // The layer states the length of the text, also of a text that it draws whole.
    assert!(
        first.shows_named("link-offer-text-length".to_owned()),
        "the length of a text of {} characters is not stated",
        offer.chars().count()
    );
    // The clipboard is written by the click and at no other moment.
    assert_eq!(first.clipboard(), None);
    first.click("link-offer-copy");
    assert_eq!(first.clipboard(), Some(offer.clone()));

    // The new device: a first start, the settings, the other role.
    let (_new_directory, mut new) = named(new_cx, "Luca");
    new.in_shell(|shell, window, cx| shell.open(Open::Settings, window, cx));
    let scroll = new.in_shell(|shell, _, _| shell.settings_scroll_handle());
    new.reveal(&scroll, "settings-link-new");
    new.click("settings-link-new");
    assert!(new.shows("link-all") && new.shows("link-offer-field"));
    assert!(new.shows("link-device-name-field"));

    // A field takes at most 32 KiB.
    let too_long = "a".repeat(LONGEST_TEXT + 1);
    new.in_shell(|shell, window, cx| shell.type_link_offer(&too_long, window, cx));
    assert!(new.shows("link-text-invalid"));
    assert_eq!(
        new.in_shell(|shell, _, _| shell.link_text_refusal()),
        Some(text::account::TEXT_TOO_LONG)
    );
    assert!(new.in_shell(|shell, _, cx| shell.link_fields_are_empty(cx)));
    // A text that is no offer is refused, and no server is added.
    new.in_shell(|shell, window, cx| shell.type_link_offer("hello", window, cx));
    assert!(!new.shows("link-text-invalid"));
    new.click("link-start");
    new.until("the text is refused", |new| new.shows("link-all-error"));
    assert!(new.view().servers.is_empty());
    assert!(new.shows("link-offer-field"));

    // The offer, carried from the first device. Each server gets its line.
    new.in_shell(|shell, window, cx| shell.type_link_offer(&offer, window, cx));
    new.click("link-start");
    new.until("both servers wait for the approval", |new| {
        new.link_lines() == [LinkShown::Waiting, LinkShown::Waiting]
            && new.shows("link-request-text")
    });
    let Some(LinkAllView::New(shown)) = new.link_all() else {
        panic!("the new device shows its role");
    };
    let NewStep::Requested {
        device_name,
        request: Some(request),
        fingerprint: Some(fingerprint),
        lines,
        all_linked: false,
    } = shown.step
    else {
        panic!("the new device shows its request");
    };
    assert_eq!(device_name, text::device_name());
    assert_eq!(
        lines
            .iter()
            .map(|line| line.server.as_str())
            .collect::<Vec<_>>(),
        ["Acme Studio", "Harbor"]
    );
    assert_eq!(fingerprint.len(), 19);
    assert_eq!(new.view().servers.len(), 2);
    assert_eq!(new.clipboard(), None);
    new.click("link-request-copy");
    assert_eq!(new.clipboard().as_deref(), Some(request.expose()));

    // The request, carried to the first device. It looks each link up and approves
    // nothing.
    first.in_shell(|shell, window, cx| {
        shell.type_link_request(request.expose(), window, cx);
    });
    first.click("link-request-look-up");
    first.until("both links are ready", |first| {
        first.approval_lines() == [ApprovalShown::Ready, ApprovalShown::Ready]
    });
    let Some(LinkAllView::First(shown)) = first.link_all() else {
        panic!("the first device shows its role");
    };
    let FirstStep::Confirm {
        fingerprint: looked_up,
        device_name,
        lines,
        can_approve,
        all_approved,
    } = shown.step
    else {
        panic!("the first device shows the request");
    };
    // The person compares the two fingerprints: they are the same.
    assert_eq!(looked_up, fingerprint);
    assert!(first.shows("link-all-fingerprint") && new.shows("link-all-fingerprint"));
    assert_eq!(device_name, text::device_name());
    assert!(can_approve && !all_approved);
    for (line, server) in lines.iter().zip(["Acme Studio", "Harbor"]) {
        assert_eq!(line.server, server);
        assert_eq!(line.account.as_deref(), Some("Luca"));
    }
    // The field kept nothing of the request, and nothing is approved yet.
    assert!(first.in_shell(|shell, _, cx| shell.link_fields_are_empty(cx)));
    assert_eq!(devices(&world), [1, 1]);
    assert_eq!(new.link_lines(), [LinkShown::Waiting, LinkShown::Waiting]);

    let new_scroll = new.in_shell(|shell, _, _| shell.linking_scroll_handle());
    let check_bounds = new.reveal(&new_scroll, "link-check-all");
    let first_scroll = first.in_shell(|shell, _, _| shell.linking_scroll_handle());
    first.reveal(&first_scroll, "link-all-approve");
    first.click("link-all-approve");
    first.until("both links are approved", |first| {
        first.approval_lines() == [ApprovalShown::Approved, ApprovalShown::Approved]
            && first.shows("link-all-approved")
    });
    assert_eq!(devices(&world), [2, 2]);

    // The new device is linked on both servers by the check that the application makes
    // by itself.
    new.until("both servers are linked", |new| {
        new.link_lines() == [LinkShown::Linked, LinkShown::Linked]
    });
    assert_eq!(new.cx.debug_bounds("link-check-all").unwrap(), check_bounds);
    new.reveal(&new_scroll, "link-all-linked");
    assert!(!new.shows("link-request-text"));
    new.click("link-all-close");
    assert!(!new.shows("link-all"));
    new.until("both servers are usable on the new device", |new| {
        let view = new.view();
        view.servers.len() == 2 && view.servers.iter().all(|server| usable(&server.status))
    });
    new.until_ready();
    assert_eq!(new.view().person.name, "Luca");
    // On each server the new device belongs to the account of the first device.
    let linked = new
        .state
        .read_with(&new.cx, |state, _| state.client().servers());
    for handle in &handles {
        let other = linked
            .iter()
            .find(|other| other.id() == handle.id())
            .unwrap();
        assert!(handle.account().is_some());
        assert!(matches!(
            other.entry().membership,
            Membership::Member { account_id, .. } if Some(account_id) == handle.account()
        ));
    }

    // The layer that closes keeps no text in its fields.
    first.press("escape");
    assert!(!first.shows("link-all"));
    assert_eq!(first.link_all(), None);
}

/// The device of a window on its only server, as the server knows it.
fn device_of(window: &mut Window) -> DeviceId {
    match window.handle().access().state {
        AccessState::LoggedIn { device_id, .. } => device_id,
        other => panic!("the window is not logged in: {other:?}"),
    }
}

/// The first device offers the link on all servers, and the new device takes the offer.
/// Returns the text of the offer.
fn offer_to_the_new_device(first: &mut Window, new: &mut Window) -> String {
    first.in_shell(|shell, window, cx| shell.open(Open::Settings, window, cx));
    let scroll = first.in_shell(|shell, _, _| shell.settings_scroll_handle());
    first.reveal(&scroll, "settings-link-first");
    first.click("settings-link-first");
    let Some(LinkAllView::First(shown)) = first.link_all() else {
        panic!("the first device shows its role");
    };
    let offer = shown.offer.unwrap();
    new.in_shell(|shell, window, cx| shell.open(Open::Settings, window, cx));
    let scroll = new.in_shell(|shell, _, _| shell.settings_scroll_handle());
    new.reveal(&scroll, "settings-link-new");
    new.click("settings-link-new");
    new.in_shell(|shell, window, cx| shell.type_link_offer(&offer, window, cx));
    new.click("link-start");
    offer
}

/// The request of the new device, carried to the first device and looked up there.
fn request_to_the_first_device(new: &mut Window, first: &mut Window) {
    new.until("the request of the new device is shown", |new| {
        new.shows("link-request-text")
    });
    let Some(LinkAllView::New(shown)) = new.link_all() else {
        panic!("the new device shows its role");
    };
    let NewStep::Requested {
        request: Some(request),
        ..
    } = shown.step
    else {
        panic!("the new device shows its request");
    };
    first.in_shell(|shell, window, cx| {
        shell.type_link_request(request.expose(), window, cx);
    });
    first.click("link-request-look-up");
}

#[gpui::test]
fn a_server_whose_link_failed_is_asked_again_with_try_again(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut first) = named(cx, "Luca");
    first.find_server(&proxy.address());
    first.join_with_code(&server.owner_code().unwrap());
    first.until_ready();
    let (_new_directory, mut new) = named(new_cx, "Luca");

    // The server answers `busy` to every try of the request for a link code: the line of
    // the server states the failure, and "Try again" is offered.
    let links = "/api/v1/device-links";
    proxy.fail(links, [Fault::Busy; 5]);
    offer_to_the_new_device(&mut first, &mut new);
    new.until("the line of the server failed", |new| {
        matches!(new.link_lines().as_slice(), [LinkShown::Failed(_)])
    });
    assert_eq!(proxy.seen(links).len(), 5);
    assert!(!new.shows("link-request-text"));
    let scroll = new.in_shell(|shell, _, _| shell.linking_scroll_handle());
    new.reveal(&scroll, "link-retry");

    // "Try again" asks the server once more, and the link waits for the first device.
    new.click("link-retry");
    new.until("the server waits for the approval", |new| {
        new.link_lines() == [LinkShown::Waiting]
    });
    assert_eq!(proxy.seen(links).len(), 6);
    request_to_the_first_device(&mut new, &mut first);
    first.until("the link is ready", |first| {
        first.approval_lines() == [ApprovalShown::Ready]
    });
    let scroll = first.in_shell(|shell, _, _| shell.linking_scroll_handle());
    first.reveal(&scroll, "link-all-approve");
    first.click("link-all-approve");
    first.until("the link is approved", |first| {
        first.approval_lines() == [ApprovalShown::Approved]
    });
    new.until("the server is linked", |new| {
        new.link_lines() == [LinkShown::Linked]
    });
}

#[gpui::test]
fn an_approval_without_an_answer_is_checked_on_the_devices_page(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut first) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let handle = first.handle();
    let (_new_directory, mut new) = named(new_cx, "Luca");
    new.find_server(&server.address());
    new.click("request-link");
    new.until("the dialog shows the link code", |new| {
        new.drawn_code().is_some()
    });
    let code = new.drawn_code().unwrap();
    let key = new.link_wait().unwrap().fingerprint;
    first.open_devices();
    first.in_shell(|shell, window, cx| shell.type_link_code(&code, window, cx));
    first.click("link-look-up");
    first.until("the key of the new device is shown", |first| {
        first.shows("link-confirm")
    });

    // The server adds the device, and its answer is lost. The list of the devices, which
    // tells whether the key was added, cannot be read either. The events wait at the
    // proxy, so that no event of the new device makes the page read the list.
    proxy.hold_events();
    proxy.fail("/api/v1/account/device-links/approve", [Fault::LoseAnswer]);
    proxy.fail("/api/v1/account/devices", [Fault::Cut]);
    first.click("link-approve");
    first.until("the page states that the outcome is unknown", |first| {
        first.shows("link-unknown")
    });
    let Some(LinkView::OutcomeUnknown {
        device_name,
        fingerprint,
        checking: false,
        ..
    }) = first.link()
    else {
        panic!("the page shows the approval whose outcome is unknown");
    };
    assert_eq!(device_name, text::device_name());
    assert_eq!(fingerprint, key);
    assert!(first.shows("link-fingerprint") && first.shows("link-check"));
    assert!(!first.shows("link-approve"));
    // The server did add the device.
    assert_eq!(world.run(handle.api().devices()).unwrap().len(), 2);

    // "Check again" reads the list of the devices: the key is there. The events that
    // waited arrive first, and the page lists the new device.
    proxy.release();
    first.until("the page lists both devices", |first| {
        first.devices().len() == 2
    });
    let scroll = first.in_shell(|shell, _, _| shell.account_scroll_handle());
    first.reveal(&scroll, "link-check");
    first.click("link-check");
    first.until_read(
        "the page tells that the device is linked",
        |first| {
            let sent: Vec<(String, String, Option<u16>)> = proxy
                .all()
                .iter()
                .filter(|seen| seen.path.contains("/account/"))
                .map(|seen| {
                    (
                        seen.method.clone(),
                        seen.path.clone(),
                        seen.answer.as_ref().map(|(status, _)| *status),
                    )
                })
                .collect();
            (first.link(), sent, first.shows("link-approved"))
        },
        |(_, _, approved)| *approved,
    );
    assert_eq!(
        first.link(),
        Some(LinkView::Approved {
            device_name: text::device_name()
        })
    );
    new.until_ready();
    assert_eq!(new.view().person.account_id, handle.account());
}

#[gpui::test]
fn a_line_whose_device_has_another_account_by_now_is_approved_for_that_account_after_use_this_account(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut first) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let handle = first.handle();
    let luca = handle.account().unwrap();
    let first_device = device_of(&mut first);
    // A second device of the account of Luca, and another member, Bruno.
    let (_phone, phone) = crate::account::second_device(&world, &server, &handle);
    let code = admin.invite(&world, 1);
    let (_bruno_device, bruno) = crate::admin_pages::joined(&world, &server, &code, "Bruno");
    let bruno = crate::support::logged_in(&bruno);

    // The first device offers the link; the new device asks, and the first device looks
    // the link up for the account of Luca.
    let (_new_directory, mut new) = named(new_cx, "Luca");
    offer_to_the_new_device(&mut first, &mut new);
    request_to_the_first_device(&mut new, &mut first);
    first.until("the link is ready", |first| {
        first.approval_lines() == [ApprovalShown::Ready]
    });

    // Meanwhile the first device gets another account on the server: the phone removes it
    // from the account of Luca, and the owner recovers the account of Bruno onto its key
    // (ADR 0112). The events wait at the proxy, so the window learns of it only from the
    // next request of the device.
    let approve = "/api/v1/account/device-links/approve";
    // The device of the window approved the phone through this route already.
    let before = proxy.seen(approve).len();
    proxy.hold_events();
    let key = handle.device_public_key().unwrap();
    world.run(async {
        phone.api().remove_device(first_device).await.unwrap();
        admin
            .handle
            .api()
            .recover_account(
                bruno,
                &RecoverAccount {
                    public_key: key,
                    device_name: "Laptop".to_owned(),
                },
            )
            .await
            .unwrap();
    });

    // "Approve" sends nothing for the account of Bruno: the line states the account that
    // the device has now.
    let scroll = first.in_shell(|shell, _, _| shell.linking_scroll_handle());
    first.reveal(&scroll, "link-all-approve");
    first.click("link-all-approve");
    first.until("the line states the other account", |first| {
        matches!(
            first.approval_lines().as_slice(),
            [ApprovalShown::AccountChanged(_)]
        )
    });
    assert_eq!(handle.account(), Some(bruno));
    // The approval went out with the session of Luca, which the removal had ended: the
    // server refused it, and it was not sent again with the session of Bruno (ADR 0712).
    let accepted = |proxy: &FaultProxy| {
        proxy.seen(approve)[before..]
            .iter()
            .filter(|seen| matches!(seen.answer, Some((200..=299, _))))
            .count()
    };
    let sent = proxy.seen(approve).len();
    assert!(sent <= before + 1, "the approval was sent again");
    assert_eq!(accepted(&proxy), 0);
    let server_id = handle.id();
    let use_account = crate::harness::selector(format!("use-account-{server_id}"));
    assert!(first.shows(use_account));
    // "Approve" leaves such a line alone. What it would send is counted after the next
    // wait, which the requests of the client runtime precede.
    first.reveal(&scroll, "link-all-approve");
    first.click("link-all-approve");

    // "Use this account" accepts the account of Bruno: the link is looked up again for it,
    // and nothing is approved before the next "Approve".
    first.reveal(&scroll, use_account);
    first.click(use_account);
    first.until("the line is ready for the account of Bruno", |first| {
        first.approval_lines() == [ApprovalShown::Ready]
    });
    assert!(!first.shows(use_account));
    assert_eq!(proxy.seen(approve).len(), sent);
    first.reveal(&scroll, "link-all-approve");
    first.click("link-all-approve");
    first.until("the link is approved", |first| {
        first.approval_lines() == [ApprovalShown::Approved]
    });
    assert_eq!(accepted(&proxy), 1);
    proxy.release();

    // The new device belongs to the account of Bruno, and the account of Luca keeps only
    // the phone.
    new.until("the server is linked", |new| {
        new.link_lines() == [LinkShown::Linked]
    });
    let linked = new
        .state
        .read_with(&new.cx, |state, _| state.client().servers())
        .remove(0);
    assert!(matches!(
        linked.entry().membership,
        Membership::Member { account_id, .. } if account_id == bruno
    ));
    let devices = world.run(phone.api().devices()).unwrap();
    assert_eq!(devices.len(), 1);
    assert_ne!(luca, bruno);
}
