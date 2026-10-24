//! The logout of the person (ADR 0712): the window ends the session of the device on one
//! server, keeps the account and the key, and logs in again only when the person asks.

use gpui::TestAppContext;
use junto_client::state::view::ServerStatus;

use crate::harness::{World, member, open};
use crate::support::FaultProxy;

const LOGIN: &str = "/api/v1/auth/sessions";
const SESSION: &str = "/api/v1/auth/session";

#[gpui::test]
fn a_person_logs_out_and_the_device_logs_in_again_only_when_the_person_asks(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let account_id = window.view().person.account_id.unwrap();

    // "Log out" in the menu of the server ends the session on the server.
    window.click("server-menu-button");
    window.until("the menu offers the logout", |window| {
        window
            .view()
            .selected
            .is_some_and(|selected| selected.can_log_out)
    });
    window.click("menu-log-out");
    window.until("the logout is shown", |window| {
        window.status() == Some(ServerStatus::LoggedOutByPerson)
    });
    assert!(window.shows("logged-out-by-person") && window.shows("log-in"));
    window.until("the server has ended the session", |_| {
        proxy.seen(SESSION).iter().any(|seen| {
            seen.method == "DELETE"
                && seen
                    .answer
                    .as_ref()
                    .is_some_and(|(status, _)| *status == 204)
        })
    });
    assert!(window.view().servers[0].workspaces.is_empty());
    let logins = proxy.seen(LOGIN).len();

    // The menu of a server without a session offers no logout, at the same place.
    window.click("server-menu-button");
    assert!(window.shows("menu-log-out"));
    assert!(!window.view().selected.unwrap().can_log_out);
    window.press("escape");

    // A second start keeps the logout: the window shows it from the server book and does
    // not log in.
    window.close();
    let mut window = open(cx, directory.path());
    window.until("the logout is shown after a start", |window| {
        window.shows("logged-out-by-person")
    });
    assert_eq!(window.status(), Some(ServerStatus::LoggedOutByPerson));
    assert_eq!(proxy.seen(LOGIN).len(), logins);

    // "Log in" logs in to the same account with the same key.
    window.click_state_action("log-in");
    window.until_ready();
    assert_eq!(window.view().person.account_id, Some(account_id));
    assert!(proxy.seen(LOGIN).len() > logins);
    let logins = proxy.seen(LOGIN).len();

    // From then on a start logs in by itself again.
    window.close();
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(window.view().person.account_id, Some(account_id));
    assert!(proxy.seen(LOGIN).len() > logins);
}
