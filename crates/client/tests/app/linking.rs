//! Device linking in the window against the real server (ADR 0782): a new device asks one
//! server for a code, a device of the account types it once and chooses the servers, and
//! the relay of the first server carries the other servers and their codes (ADR 0104).

use gpui::TestAppContext;
use junto_client::shell::Open;
use junto_client::state::linking::{
    ApprovalShown, ChoiceLine, FirstStep, LinkAllView, LinkShown, LinkWait, NewStep,
};
use junto_client::state::view::ServerStatus;
use junto_client::text;
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::error::ErrorCode;

use crate::harness::{Window, World, member, named, open};
use crate::support::{Fault, FaultProxy};

impl Window {
    /// What the selected server shows while this device waits for its link.
    fn link_wait(&mut self) -> Option<LinkWait> {
        let server_id = self.view().selected?.id;
        self.state
            .read_with(&self.cx, |state, _| state.link_wait(server_id))
    }

    /// The layer of a link, as the window drew it last.
    fn link_all(&mut self) -> Option<LinkAllView> {
        self.run_tasks();
        self.shell
            .read_with(&self.cx, |shell, _| shell.link_all_drawn().cloned())
    }

    /// The code that the new device shows in the layer.
    fn layer_code(&mut self) -> Option<String> {
        match self.link_all() {
            Some(LinkAllView::New(view)) => match view.step {
                NewStep::Waiting {
                    wait: Some(wait), ..
                } => wait.code.map(|code| code.expose().to_owned()),
                _ => None,
            },
            _ => None,
        }
    }

    /// The device code that the new device shows in the layer.
    fn layer_device_code(&mut self) -> Option<String> {
        match self.link_all() {
            Some(LinkAllView::New(view)) => match view.step {
                NewStep::Waiting {
                    wait: Some(wait), ..
                } => Some(wait.fingerprint),
                _ => None,
            },
            _ => None,
        }
    }

    /// The servers of the device that approves, while it chooses them.
    fn choices(&mut self) -> Vec<ChoiceLine> {
        match self.link_all() {
            Some(LinkAllView::First(view)) => match view.step {
                FirstStep::EnterCode { choices } | FirstStep::Confirm { choices, .. } => choices,
                FirstStep::Linking { .. } => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// The states of the lines of the device that approves, in the order of the layer.
    fn approval_lines(&mut self) -> Vec<(String, ApprovalShown)> {
        match self.link_all() {
            Some(LinkAllView::First(view)) => match view.step {
                FirstStep::Linking { lines, .. } => lines
                    .into_iter()
                    .map(|line| (line.server, line.progress))
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    /// The states of the lines of the new device after its first server, in the order of
    /// the layer, and whether the link is done.
    fn link_lines(&mut self) -> Option<(Vec<LinkShown>, bool)> {
        match self.link_all() {
            Some(LinkAllView::New(view)) => match view.step {
                NewStep::Linked { lines, done, .. } => {
                    Some((lines.into_iter().map(|line| line.progress).collect(), done))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn open_link_settings(&mut self, selector: &'static str) {
        self.in_shell(|shell, window, cx| shell.open(Open::Settings, window, cx));
        self.click("settings-section-devices");
        let scroll = self.in_shell(|shell, _, _| shell.settings_scroll_handle());
        self.reveal(&scroll, selector);
        self.click(selector);
        assert!(self.shows("link-all"));
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

    // The new device: a first start, the address, and the third way of the dialog, which
    // starts the link of the layer with this server.
    let (_new_directory, mut new) = named(new_cx, "Luca");
    new.find_server(&server.address());
    assert!(new.shows("request-link") && new.shows("code-field"));
    new.click("request-link");
    new.until("the layer shows the link code", |new| {
        new.shows("link-wait-code") && new.layer_code().is_some()
    });
    assert!(!new.shows("join-dialog") && new.shows("link-all"));
    assert!(new.shows("link-wait-fingerprint") && new.shows("link-wait-expiry"));
    let first_code = new.layer_code().unwrap();
    assert_eq!(first_code.len(), 9, "two groups of four symbols");
    assert_eq!(new.view().servers.len(), 1);

    // "New code" asks the server again: the code of the first request is replaced.
    new.click("link-all-new-code");
    new.until("the layer shows another code", |new| {
        new.layer_code().is_some_and(|code| code != first_code)
    });
    let code = new.layer_code().unwrap();
    let device_code = new.layer_device_code().unwrap();
    assert_eq!(device_code.split(' ').count(), 11);
    let wait = new.link_wait().unwrap();
    assert_eq!(wait.fingerprint, device_code);

    // The device of the account: "Link a new device" on the devices page opens the same
    // flow with only this server checked.
    first.open_devices();
    first.click("account-link-new-device");
    assert!(first.shows("link-all") && !first.shows("account"));
    let choices = first.choices();
    assert_eq!(choices.len(), 1);
    assert!(choices[0].checked);

    // An empty field is not sent.
    first.click("link-code-look-up");
    assert!(first.shows("link-text-invalid"));
    // The code of the first request is not known any more.
    first.in_shell(|shell, window, cx| shell.type_link_code(&first_code, window, cx));
    assert!(!first.shows("link-text-invalid"));
    first.click("link-code-look-up");
    first.until("the replaced code is refused", |first| {
        first.shows("link-all-error")
    });
    let Some(LinkAllView::First(view)) = first.link_all() else {
        panic!("the device of the account shows its role");
    };
    assert_eq!(
        view.error,
        Some(text::account::link_error(&FlowError::refused(
            ErrorCode::LinkCodeInvalid
        )))
    );

    // The code as a person types it: without regard to case.
    first.in_shell(|shell, window, cx| shell.type_link_code(&code.to_lowercase(), window, cx));
    first.click("link-code-look-up");
    first.until("the device code of the new device is shown", |first| {
        first.shows("link-all-device-code")
    });
    let Some(LinkAllView::First(view)) = first.link_all() else {
        panic!("the device of the account shows its role");
    };
    let FirstStep::Confirm {
        device_name,
        device_code: shown,
        choices,
    } = view.step
    else {
        panic!("the device of the account shows the device code to compare");
    };
    assert_eq!(device_name, text::device_name());
    // The person compares the two texts: they are the same key.
    assert_eq!(shown, device_code);
    assert!(choices[0].locked);
    // Nothing is approved before the person approves.
    assert_eq!(world.run(handle.api().devices()).unwrap().len(), 1);

    first.click("link-all-approve");
    first.until(
        "the device of the account tells that the device is linked",
        |first| first.shows("link-all-done"),
    );
    assert_eq!(
        first.approval_lines(),
        [("Acme Studio".to_owned(), ApprovalShown::Approved)]
    );
    assert_eq!(world.run(handle.api().devices()).unwrap().len(), 2);

    // The new device logs in by the check that the application makes by itself, and its
    // layer tells that the link is done.
    new.until("the new device is linked", |new| {
        new.link_lines() == Some((Vec::new(), true))
    });
    new.click("link-all-done");
    new.until_ready();
    let person = new.view().person;
    assert_eq!(person.account_id, Some(account_id));
    assert_eq!(person.name, "Luca");
    first.click("link-all-done");
    assert!(!first.shows("link-all"));
}

#[gpui::test]
fn a_second_installation_is_linked_on_two_servers_with_one_code(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let acme = world.server("Acme Studio");
    let harbor = world.server("Harbor");
    let usable =
        |status: &ServerStatus| matches!(status, ServerStatus::Ready | ServerStatus::Standby);

    // The device of the account has an account on both servers.
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

    // The new device: the first start offers the link in place of the name.
    let new_directory = tempfile::tempdir().unwrap();
    let mut new = open(new_cx, new_directory.path());
    assert!(new.shows("welcome-dialog") && new.shows("welcome-link"));
    new.click("welcome-link");
    assert!(new.shows("link-all") && new.shows("link-address-field"));
    // An empty address is not sent.
    new.click("link-start");
    assert!(new.shows("link-text-invalid"));
    new.in_shell(|shell, window, cx| shell.type_link_address(&harbor.address(), window, cx));
    new.click("link-start");
    new.until("the new device shows the code of Harbor", |new| {
        new.layer_code().is_some()
    });
    let code = new.layer_code().unwrap();
    let device_code = new.layer_device_code().unwrap();
    assert_eq!(new.view().servers.len(), 1);

    // The device of the account: the settings, both servers checked, the code once.
    first.open_link_settings("settings-link-first");
    assert!(first.choices().iter().all(|choice| choice.checked));
    first.in_shell(|shell, window, cx| shell.type_link_code(&code, window, cx));
    first.click("link-code-look-up");
    first.until("the device code is shown", |first| {
        first.shows("link-all-device-code")
    });
    let choices = first.choices();
    let locked: Vec<&str> = choices
        .iter()
        .filter(|choice| choice.locked)
        .map(|choice| choice.server.as_str())
        .collect();
    assert_eq!(locked, ["Harbor"]);
    let Some(LinkAllView::First(view)) = first.link_all() else {
        panic!("the device of the account shows its role");
    };
    let FirstStep::Confirm {
        device_code: shown, ..
    } = view.step
    else {
        panic!("the device code is shown");
    };
    assert_eq!(shown, device_code);
    let scroll = first.in_shell(|shell, _, _| shell.linking_scroll_handle());
    first.reveal(&scroll, "link-all-approve");
    first.click("link-all-approve");

    // Harbor links the new device; the relay carries Acme to the new device, which asks
    // Acme for a code and answers; the device of the account approves Acme.
    first.until("both servers are approved", |first| {
        let lines = first.approval_lines();
        lines.len() == 2
            && lines
                .iter()
                .all(|(_, progress)| *progress == ApprovalShown::Approved)
    });
    assert_eq!(first.approval_lines()[0].0, "Harbor");
    assert_eq!(devices(&world), [2, 2]);
    new.until("the new device is linked on both servers", |new| {
        new.link_lines() == Some((vec![LinkShown::Linked], true))
    });
    assert_eq!(new.view().servers.len(), 2);
    new.click("link-all-done");
    new.until("the new device is logged in on both servers", |new| {
        let view = new.view();
        view.servers.len() == 2 && view.servers.iter().all(|server| usable(&server.status))
    });
}

/// The page of the devices reads its list again at "Try again" (ADR 0781).
#[gpui::test]
fn a_device_list_that_was_not_read_is_read_again(cx: &mut TestAppContext) {
    const DEVICES: &str = "/api/v1/account/devices";
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut first) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    proxy.fail(DEVICES, std::iter::repeat_n(Fault::Busy, attempts));
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
}

/// A server that refuses the request for a link code is stated on the line of the new
/// device and on the device that approves, and "Try again" asks it again.
#[gpui::test]
fn a_server_that_refused_the_new_device_is_asked_again_with_try_again(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let acme = world.server("Acme Studio");
    let harbor = world.server("Harbor");
    let proxy = world.run(FaultProxy::start(&harbor));
    let (_directory, mut first) = named(cx, "Luca");
    first.find_server(&acme.address());
    first.join_with_code(&acme.owner_code().unwrap());
    first.until_ready();
    first.find_server(&proxy.address());
    first.join_with_code(&harbor.owner_code().unwrap());
    first.until_ready();

    let (_new_directory, mut new) = named(new_cx, "Luca");
    new.open_link_settings("settings-link-new");
    new.in_shell(|shell, window, cx| shell.type_link_address(&acme.address(), window, cx));
    new.click("link-start");
    new.until("the code is shown", |new| new.layer_code().is_some());
    let code = new.layer_code().unwrap();

    // Harbor answers `busy` to every try of the request for a link code.
    let links = "/api/v1/device-links";
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    proxy.fail(links, std::iter::repeat_n(Fault::Busy, attempts));
    first.open_link_settings("settings-link-first");
    first.in_shell(|shell, window, cx| shell.type_link_code(&code, window, cx));
    first.click("link-code-look-up");
    first.until("the device code is shown", |first| {
        first.shows("link-all-device-code")
    });
    let scroll = first.in_shell(|shell, _, _| shell.linking_scroll_handle());
    first.reveal(&scroll, "link-all-approve");
    first.click("link-all-approve");
    new.until("the line of Harbor failed", |new| {
        matches!(
            new.link_lines(),
            Some((lines, false)) if matches!(lines.as_slice(), [LinkShown::Failed(_)])
        )
    });
    first.until("the device of the account states the failure", |first| {
        matches!(
            first.approval_lines().get(1),
            Some((_, ApprovalShown::Failed(_)))
        )
    });

    // "Try again" on the new device asks Harbor once more, and the device of the account
    // approves the new code.
    let scroll = new.in_shell(|shell, _, _| shell.linking_scroll_handle());
    new.reveal(&scroll, "link-check-all");
    new.click("link-check-all");
    first.until("Harbor is approved", |first| {
        first.approval_lines().get(1).map(|line| &line.1) == Some(&ApprovalShown::Approved)
    });
    new.until("the new device is linked on both servers", |new| {
        new.link_lines() == Some((vec![LinkShown::Linked], true))
    });
}
