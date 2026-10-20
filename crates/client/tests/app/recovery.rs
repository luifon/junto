//! The recovery of an account whose devices are lost (ADR 0104, ADR 0112): the server owner
//! gives the key of a new device access to the account on the page of the people, and the
//! account lists every recovery, also those that a device acknowledged (ADR 0118).

use gpui::TestAppContext;
use junto_client::state::account::{PageView, RecoveryLine, RecoveryList};
use junto_client::state::admin::view::people::NoRecovery;
use junto_client::state::admin::view::{Scope, ServerPage};
use junto_client_core::protocol::roles::CreateServerRole;

use crate::admin_pages::{
    account, awaited, click_in_page, invite, joined, open_page, owner_window_at, sel,
};
use crate::harness::{Window, World, named};
use crate::support::{FaultProxy, logged_in};

impl Window {
    /// The recoveries that the open recoveries page lists. Absent while the page reads
    /// them or is not open.
    fn recoveries(&mut self) -> Option<Vec<RecoveryLine>> {
        match self.account()?.page {
            PageView::Recoveries(RecoveryList::Listed { rows, .. }) => Some(rows),
            _ => None,
        }
    }

    fn open_recoveries(&mut self) {
        self.open_account();
        self.click("account-nav-recoveries");
    }
}

#[gpui::test]
fn the_owner_recovers_an_account_onto_a_new_device_and_the_account_lists_the_recovery(
    cx: &mut TestAppContext,
    new_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_owner_directory, mut owner, owner_handle) =
        owner_window_at(cx, &server, &proxy.address());
    let ada = logged_in(&owner_handle);
    let code = invite(&world, &owner_handle, Vec::new());
    let (lost, member) = joined(&world, &server, &code, "Bruno");
    let bruno = logged_in(&member);
    let code = invite(&world, &owner_handle, Vec::new());
    let (_dora_device, dora) = joined(&world, &server, &code, "Dora");
    let dora = logged_in(&dora);
    // A server role that the owner can give, so that the row of an account has switches.
    let helpers = world
        .run(owner_handle.api().create_server_role(&CreateServerRole {
            name: "Helpers".to_owned(),
            permissions: Vec::new(),
        }))
        .unwrap()
        .id;
    // The only device of the account is lost with its data directory.
    drop(member);
    drop(lost);

    // A new installation asks to link its device, and shows its key with the way of a
    // recovery.
    let (_new_directory, mut new) = named(new_cx, "Bruno");
    new.find_server(&server.address());
    new.click("request-link");
    new.until("the key of the new device is shown", |window| {
        window.shows("link-wait")
    });
    assert!(new.shows("link-wait-recovery"));
    let key = new.state.read_with(&new.cx, |state, _| {
        let server_id = state.client().servers()[0].id();
        state.link_wait(server_id).unwrap().fingerprint
    });
    new.click("dialog-close");
    assert!(new.shows("waiting-for-link"));

    // The owner offers the recovery of the account of another person only.
    owner.open_administration();
    open_page(&mut owner, "admin-nav-people", |window| {
        matches!(
            window.admin().map(|view| view.scope),
            Some(Scope::Server {
                page: ServerPage::People(_),
                ..
            })
        )
    });
    let (own, other) = awaited(&mut owner, "the page lists both accounts", |window| {
        Some((account(window, ada)?, account(window, bruno)?))
    });
    assert_eq!(own.recover, Some(Err(NoRecovery::OwnAccount)));
    assert_eq!(other.recover, Some(Ok(())));
    click_in_page(&mut owner, sel("account-recover", bruno));
    owner.until("the form of the recovery is shown", |window| {
        window.shows("recovery-form")
    });

    // The roles of another account are open under its row while the form is open.
    click_in_page(&mut owner, sel("account-roles-toggle", dora));
    owner.until("the roles of the other account are drawn", |window| {
        window.shows(sel("account-roles", dora))
    });
    assert!(owner.shows("recovery-form"));

    // A text that is no key is told at the form, and nothing is sent.
    owner.in_shell(|shell, window, cx| shell.type_recovery("not a key", "", window, cx));
    click_in_page(&mut owner, "recovery-confirm");
    assert!(owner.shows("recovery-invalid"));
    assert!(owner.shows("recovery-form") && !owner.shows("account-recovered"));

    // The key as the new device shows it, in groups, recovers the account. While the
    // request is on its way neither Cancel, the action of another row, its Roles control
    // nor one of its open role switches releases the row of the form (ADR 0730).
    owner.in_shell(|shell, window, cx| shell.type_recovery(&key, "Replacement", window, cx));
    proxy.hold_request("POST", "/recovery");
    click_in_page(&mut owner, "recovery-confirm");
    owner.until("the recovery is on its way", |_| {
        !proxy
            .seen(&format!("/api/v1/accounts/{bruno}/recovery"))
            .is_empty()
    });
    click_in_page(&mut owner, "recovery-cancel");
    click_in_page(&mut owner, sel("account-ban", dora));
    assert!(owner.shows("recovery-form") && !owner.shows("account-confirm"));
    let switch = sel(&format!("account-role-{dora}"), helpers);
    click_in_page(&mut owner, switch);
    click_in_page(&mut owner, sel("account-roles-toggle", dora));
    owner.settle();
    assert!(owner.shows("recovery-form") && owner.shows(sel("account-roles", dora)));
    assert!(
        proxy
            .seen(&format!("/api/v1/accounts/{dora}/server-roles/{helpers}"))
            .is_empty(),
        "a role switch was sent while the recovery was on its way"
    );
    proxy.release();
    owner.until("the row tells the recovery", |window| {
        window.shows("account-recovered")
    });
    assert!(!owner.shows("recovery-form") && !owner.shows("account-refused"));

    // The new device finds its account by itself, and is told of the recovery.
    new.until_ready();
    assert_eq!(new.view().person.account_id, Some(bruno));
    new.until("the notice of the recovery is shown", |window| {
        window.shows("recovery-notice")
    });

    // The page of the recoveries lists it: it added this device, which has not seen it.
    new.open_recoveries();
    let rows = new.until_value("the recovery is listed", Window::recoveries);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].this_device && rows[0].unseen && !rows[0].by_host);
    new.press("escape");

    // An acknowledged recovery stays in the list of the account.
    new.click("recovery-seen");
    new.until("the notice is gone", |window| {
        !window.shows("recovery-notice")
    });
    new.open_recoveries();
    new.until("the recovery is listed as seen", |window| {
        window
            .recoveries()
            .is_some_and(|rows| rows.len() == 1 && !rows[0].unseen)
    });

    // "Done" puts the result away in the row of the owner.
    click_in_page(&mut owner, "recovered-done");
    assert!(!owner.shows("account-recovered"));
}
