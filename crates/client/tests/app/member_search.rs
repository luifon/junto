use crate::harness::{World, member};
use gpui::TestAppContext;
use junto_client::{shell::Open, state::view::Members};

#[gpui::test]
fn member_search_finds_an_offscreen_member_and_clears_an_empty_result(cx: &mut TestAppContext) {
    use sqlx::ConnectOptions;
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let target: junto_client_core::protocol::ids::AccountId = world.run(async {
        let options: sqlx::postgres::PgConnectOptions = server.database_url().parse().unwrap();
        let mut database = options.connect().await.unwrap();
        sqlx::query("insert into accounts (id,name,status,created_at) select gen_random_uuid(),'Member ' || lpad(n::text,3,'0'),'active',now() from generate_series(1,60) n").execute(&mut database).await.unwrap();
        sqlx::query("insert into workspace_members(workspace_id,account_id,joined_at) select state.default_workspace_id,accounts.id,now() from accounts,server_state state where accounts.name like 'Member %'").execute(&mut database).await.unwrap();
        sqlx::query_scalar("select id from accounts where name='Member 060'").fetch_one(&mut database).await.unwrap()
    });
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    window.in_shell(|shell, window, cx| shell.open(Open::Members, window, cx));
    window.until("the full member list has loaded", |window| matches!(window.view().workspace().map(|workspace| &workspace.members), Some(Members::Loaded(rows)) if rows.len() >= 60));
    assert!(window.shows("member-search"));
    let target_row: &'static str = Box::leak(format!("member-row:{target}").into_boxed_str());
    if let Some(bounds) = window.cx.debug_bounds(target_row) {
        assert!(!bounds.intersects(&window.cx.debug_bounds("member-list").unwrap()));
    }
    window.click("member-search");
    window.simulate(|cx| cx.simulate_input("mEMbEr 060"));
    window.until("search finds a member beyond the visible rows", |window| {
        window
            .cx
            .debug_bounds(target_row)
            .zip(window.cx.debug_bounds("member-list"))
            .is_some_and(|(row, viewport)| row.intersects(&viewport))
    });
    let own = window.own_account();
    assert!(!window.shows_named(format!("member-row:{own}")));
    window.press("secondary-a");
    window.simulate(|cx| cx.simulate_input("No such member"));
    window.until("the empty result is stated", |window| {
        window.shows("member-search-empty")
    });
    window.click("member-search-clear");
    window.until("clearing restores the list", |window| {
        !window.shows("member-search-empty") && window.shows_named(format!("member-row:{own}"))
    });
}

#[gpui::test]
fn filtering_a_pressed_row_never_messages_its_replacement_and_a_new_server_clears_the_query(
    cx: &mut TestAppContext,
) {
    use crate::admin_pages::{invite, joined};
    use gpui::{Modifiers, MouseButton};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Member");
    let (_alpha_device, alpha) = joined(
        &world,
        &server,
        &invite(&world, &admin.handle, Vec::new()),
        "Alpha",
    );
    let (_bravo_device, bravo) = joined(
        &world,
        &server,
        &invite(&world, &admin.handle, Vec::new()),
        "Bravo",
    );
    let alpha_id = crate::support::logged_in(&alpha);
    let bravo_id = crate::support::logged_in(&bravo);
    window.in_shell(|shell, window, cx| {
        shell.open(Open::Members, window, cx);
        shell.type_member_search("Alpha", window, cx);
    });
    window.until("Alpha is the filtered row", |window| {
        window.shows_named(format!("message-member:{alpha_id}"))
            && !window.shows_named(format!("message-member:{bravo_id}"))
    });
    let selector: &'static str = Box::leak(format!("message-member:{alpha_id}").into_boxed_str());
    let bounds = window.cx.debug_bounds(selector).unwrap();
    let at = bounds.center();
    window.simulate(|cx| cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none()));
    window.in_shell(|shell, window, cx| shell.type_member_search("Bravo", window, cx));
    window.until("Bravo replaces the filtered row", |window| {
        window.shows_named(format!("message-member:{bravo_id}"))
            && !window.shows_named(format!("message-member:{alpha_id}"))
    });
    window.simulate(|cx| cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none()));
    window.settle();
    assert!(window.view().lists().unwrap().conversation.is_none());
    window.click_named(format!("message-member:{bravo_id}"));
    window.until("an intentional press messages Bravo", |window| {
        window
            .view()
            .lists()
            .and_then(|lists| lists.conversation.as_ref())
            .is_some_and(|person| person.account_id == Some(bravo_id))
    });
    let writes: Vec<_> = proxy
        .all()
        .into_iter()
        .filter(|request| request.method == "POST" && request.path.ends_with("/conversations"))
        .collect();
    assert_eq!(writes.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&writes[0].body).unwrap();
    assert_eq!(body["account_id"], bravo_id.to_string());
    window.in_shell(|shell, window, cx| shell.type_member_search("no match", window, cx));
    let mill = world.server("Mill");
    let other = world.owner(&mill);
    window.join_further(&world, &mill, &other);
    window.in_shell(|shell, window, cx| shell.open(Open::Members, window, cx));
    let own = window.own_account();
    window.until(
        "the new server starts with an unfiltered roster",
        |window| {
            window.shows_named(format!("member-row:{own}")) && !window.shows("member-search-empty")
        },
    );
}
