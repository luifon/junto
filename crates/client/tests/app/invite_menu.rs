use crate::harness::{World, member};
use gpui::TestAppContext;

#[gpui::test]
fn invite_people_in_the_server_menu_opens_the_invite_form(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, _owner) = crate::admin_pages::owner_window(cx, &server);
    window.click("server-menu-button");
    assert!(window.shows("menu-invite-people"));
    window.click("menu-invite-people");
    window.until("the invite form opens directly", |window| {
        window.shows("invite-form")
    });
    assert!(window.shows("invite-create"));
}

#[gpui::test]
fn a_member_without_invite_permission_has_no_invite_shortcut(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    window.click("server-menu-button");
    assert!(!window.shows("menu-invite-people"));
    let account = window.view().person.account_id.unwrap();
    world
        .run(admin.handle.api().set_server_permissions(
            account,
            &junto_client_core::protocol::roles::SetServerPersonPermissions {
                allow: vec![junto_client_core::protocol::ServerPermission::CreateInvites],
                deny: Vec::new(),
            },
        ))
        .unwrap();
    window.until("granting the permission adds the shortcut", |window| {
        window.shows("menu-invite-people")
    });
    world
        .run(admin.handle.api().set_server_permissions(
            account,
            &junto_client_core::protocol::roles::SetServerPersonPermissions::default(),
        ))
        .unwrap();
    window.until("revoking the permission removes the shortcut", |window| {
        !window.shows("menu-invite-people")
    });
}
