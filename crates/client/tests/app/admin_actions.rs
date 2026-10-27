use super::*;
use junto_client::state::admin::RoleRef;
use junto_client::state::admin::view::roles::RoleRow;

fn type_field(window: &mut Window, field: &'static str, value: &str) {
    reveal_in_page(window, field);
    window.click(field);
    window.press("secondary-a backspace");
    crate::rehearsal::type_into(window, field, value);
}

fn roles(window: &mut Window) -> Option<Vec<RoleRow>> {
    server_page!(window, Roles)?.roles.loaded().cloned()
}

#[gpui::test]
fn server_roles_are_created_renamed_changed_ordered_given_taken_and_deleted(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Mira");
    let account_id = logged_in(&member);
    window.open_administration();
    open_page(&mut window, "admin-nav-server-roles", |window| {
        roles(window).is_some()
    });

    let mut created = Vec::new();
    for name in ["Readers", "Helpers"] {
        click_in_page(&mut window, "role-new");
        type_field(&mut window, "role-name", name);
        click_in_page(&mut window, "role-form-send");
        let row = awaited(&mut window, "the new server role is drawn", |window| {
            if !idle(window) {
                return None;
            }
            roles(window)?.into_iter().find(|row| row.name == name)
        });
        let RoleRef::Server(id) = row.role else {
            panic!("not a server role")
        };
        assert!(window.shows(sel("role-row", id)));
        assert_drawn_label(&mut window, name);
        let stored = world.run(owner.api().server_roles()).unwrap();
        assert!(
            stored
                .iter()
                .any(|role| role.id == id && role.name == name && role.permissions.is_empty())
        );
        created.push(id);
    }
    let (readers, helpers) = (created[0], created[1]);
    click_in_page(&mut window, sel("role-rename", helpers));
    type_field(&mut window, "role-name", "Operators");
    click_in_page(&mut window, "role-form-send");
    window.until("the renamed role is shown", |window| {
        idle(window)
            && roles(window).is_some_and(|rows| rows.iter().any(|row| row.name == "Operators"))
    });
    reveal_in_page(&mut window, sel("role-row", helpers));
    assert_drawn_label(&mut window, "Operators");
    assert!(
        world
            .run(owner.api().server_roles())
            .unwrap()
            .iter()
            .any(|row| row.id == helpers && row.name == "Operators")
    );

    click_in_page(&mut window, sel("role-permissions", helpers));
    for on in [true, false, true] {
        click_in_page(&mut window, "role-permission-create_invites");
        window.until("the server permission has the chosen state", |window| {
            idle(window)
                && roles(window).is_some_and(|rows| {
                    rows.iter().any(|row| {
                        row.role == RoleRef::Server(helpers)
                            && row.permissions.iter().any(|permission| {
                                permission.permission.name() == "create_invites"
                                    && permission.on == on
                            })
                    })
                })
        });
        reveal_in_page(&mut window, "role-permission-create_invites");
        assert_drawn_selection(&mut window, "Create invites for the role Operators", on);
        let stored = world.run(owner.api().server_roles()).unwrap();
        assert_eq!(
            stored
                .iter()
                .find(|row| row.id == helpers)
                .unwrap()
                .permissions
                .contains(&ServerPermission::CreateInvites),
            on
        );
    }
    click_in_page(&mut window, sel("role-permissions", helpers));
    for (button, expected) in [
        ("role-up", vec![helpers, readers]),
        ("role-down", vec![readers, helpers]),
    ] {
        click_in_page(&mut window, sel(button, helpers));
        window.until("the role order is drawn", |window| {
            idle(window)
                && roles(window).is_some_and(|rows| {
                    rows.iter().map(|row| row.role).collect::<Vec<_>>()
                        == expected
                            .iter()
                            .copied()
                            .map(RoleRef::Server)
                            .collect::<Vec<_>>()
                })
        });
        reveal_in_page(&mut window, sel("role-row", expected[0]));
        reveal_in_page(&mut window, sel("role-row", expected[1]));
        let first = window
            .cx
            .debug_bounds(sel("role-row", expected[0]))
            .unwrap();
        let second = window
            .cx
            .debug_bounds(sel("role-row", expected[1]))
            .unwrap();
        assert!(
            first.bottom() <= second.top(),
            "roles are drawn in the requested order"
        );
        let mut stored = world.run(owner.api().server_roles()).unwrap();
        stored.sort_by_key(|row| std::cmp::Reverse(row.position));
        assert_eq!(
            stored.iter().map(|row| row.id).collect::<Vec<_>>(),
            expected
        );
    }

    open_page(&mut window, "admin-nav-people", |window| {
        server_page!(window, People).is_some()
    });
    click_in_page(&mut window, sel("account-roles-toggle", account_id));
    for given in [true, false] {
        click_in_page(
            &mut window,
            sel(&format!("account-role-{account_id}"), helpers),
        );
        window.until("the account shows its server roles", |window| {
            idle(window)
                && account(window, account_id)
                    .and_then(|row| row.assignable)
                    .is_some_and(|choices| {
                        choices.iter().any(|choice| {
                            choice.role == RoleRef::Server(helpers) && choice.held == given
                        })
                    })
        });
        reveal_in_page(
            &mut window,
            sel(&format!("account-role-{account_id}"), helpers),
        );
        assert_drawn_selection(&mut window, "The role Operators for Mira", given);
        let stored = world.run(owner.api().accounts()).unwrap();
        assert_eq!(
            stored
                .iter()
                .find(|row| row.id == account_id)
                .unwrap()
                .server_role_ids
                .contains(&helpers),
            given
        );
    }
    open_page(&mut window, "admin-nav-server-roles", |window| {
        roles(window).is_some()
    });
    click_in_page(&mut window, sel("role-delete", helpers));
    assert!(window.shows("role-confirm"));
    assert!(
        world
            .run(owner.api().server_roles())
            .unwrap()
            .iter()
            .any(|row| row.id == helpers)
    );
    click_in_page(&mut window, "role-confirm");
    window.until("the deleted server role leaves the page", |window| {
        roles(window)
            .is_some_and(|rows| rows.iter().all(|row| row.role != RoleRef::Server(helpers)))
    });
    reveal_in_page(&mut window, sel("role-row", readers));
    assert!(!window.shows(sel("role-row", helpers)));
    assert!(
        world
            .run(owner.api().server_roles())
            .unwrap()
            .iter()
            .all(|row| row.id != helpers)
    );
}

#[gpui::test]
fn an_override_can_allow_a_permission_and_be_removed_as_a_whole(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let workspace = default_workspace(&world, &owner);
    let channel = text_channel(&world, &owner, workspace, "plans");
    open_channel_settings(&mut window, "plans", channel);
    open_page(&mut window, "channel-nav-permissions", shows_permissions);
    awaited(&mut window, "the everyone role is read", everyone_role);
    click_in_page(&mut window, "override-manage_messages-allow");
    let row = awaited(&mut window, "the allow is shown", |window| {
        if !idle(window) {
            return None;
        }
        everyone_role(window).filter(|row| {
            permission_line(row, WorkspacePermission::ManageMessages).current == Choice::Allow
        })
    });
    reveal_in_page(&mut window, "override-manage_messages-allow");
    assert_drawn_label(&mut window, "Manage messages: Allow, chosen");
    let stored = world.run(owner.api().channel_rules(channel)).unwrap();
    assert!(
        stored
            .of_role(role_of(&row))
            .is_some_and(|rule| rule.allow.contains(&WorkspacePermission::ManageMessages))
    );
    click_in_page(&mut window, "override-lock_room-deny");
    window.until("both changes are shown", |window| {
        idle(window)
            && everyone_role(window).is_some_and(|row| row.allowed() == 1 && row.denied() == 1)
    });
    click_in_page(&mut window, sel("override-clear", role_of(&row)));
    window.until("the whole override is gone", |window| {
        everyone_role(window).is_some_and(|row| !row.has_rule)
    });
    reveal_in_page(&mut window, "override-manage_messages-neutral");
    assert_drawn_label(&mut window, "Manage messages: Neutral, chosen");
    reveal_in_page(&mut window, "override-lock_room-neutral");
    assert_drawn_label(&mut window, "Lock room: Neutral, chosen");
    assert!(!window.shows(sel("override-clear", role_of(&row))));
    assert!(
        world
            .run(owner.api().channel_rules(channel))
            .unwrap()
            .rules
            .is_empty()
    );
}

#[gpui::test]
fn archive_and_retention_settings_are_saved_and_can_be_set_to_never(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let workspace = default_workspace(&world, &owner);
    window.open_administration();
    open_page(&mut window, "admin-nav-archive", |window| {
        server_page!(window, Archive).is_some()
    });
    for (index, (control, title)) in [
        ("days-archive-period", "Archive period"),
        ("days-file-retention", "Files of messages"),
        ("days-recording-retention", "Recordings"),
        ("days-direct-retention", "Files of direct messages"),
    ]
    .into_iter()
    .enumerate()
    {
        for days in [Some(14), None] {
            click_in_page(&mut window, control);
            if let Some(days) = days {
                type_field(&mut window, "days-field", &days.to_string());
                click_in_page(&mut window, "admin-form-send");
            } else {
                click_in_page(&mut window, "days-never");
            }
            window.until("the retention setting is shown", |window| {
                let Some(page) = server_page!(window, Archive) else {
                    return false;
                };
                let value = match index {
                    0 => page
                        .settings
                        .and_then(|value| value.loaded().map(|days| days.archive_period)),
                    1 => page
                        .settings
                        .and_then(|value| value.loaded().map(|days| days.file_retention)),
                    2 => page
                        .settings
                        .and_then(|value| value.loaded().map(|days| days.recording_retention)),
                    _ => page
                        .direct_retention
                        .and_then(|value| value.loaded().copied()),
                };
                idle(window) && value == Some(days) && !window.shows("days-field")
            });
            reveal_in_page(&mut window, control);
            assert_drawn_setting(
                &mut window,
                control,
                title,
                if days.is_some() { "14 days" } else { "Never" },
            );
            let api = owner.api();
            let stored = match index {
                0 => world.run(api.archive_period(workspace)).unwrap().days,
                1 => world.run(api.file_retention(workspace)).unwrap().days,
                2 => world.run(api.recording_retention(workspace)).unwrap().days,
                _ => world.run(api.direct_message_retention()).unwrap().days,
            };
            assert_eq!(stored, days);
        }
    }
}

#[gpui::test]
fn an_invite_grants_the_chosen_workspace_and_can_be_revoked_in_the_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let default = default_workspace(&world, &owner);
    let second = world
        .run(owner.api().create_workspace("Second".to_owned()))
        .unwrap();
    window.open_administration();
    open_page(&mut window, "admin-nav-invites", |window| {
        server_page!(window, Invites).is_some()
    });
    click_in_page(&mut window, "invite-new");
    click_in_page(&mut window, sel("invite-workspace", default));
    click_in_page(&mut window, sel("invite-workspace", second.id));
    click_in_page(&mut window, sel("invite-workspace", default));
    click_in_page(&mut window, "invite-create");
    let made = awaited(&mut window, "the new invite code is shown", |window| {
        server_page!(window, Invites)?.made
    });
    reveal_in_page(&mut window, "invite-code-copy");
    assert!(window.shows("invite-code") && window.shows("invite-code-copy"));
    window
        .cx
        .write_to_clipboard(gpui::ClipboardItem::new_string(
            "previous clipboard value".to_owned(),
        ));
    click_in_page(&mut window, "invite-code-copy");
    let copied = window
        .cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    assert_eq!(copied, made.code.expose());
    assert_drawn_label(&mut window, "Copied");
    let stored = world.run(owner.api().invites()).unwrap();
    assert_eq!(
        stored
            .iter()
            .find(|row| row.id == made.invite_id)
            .unwrap()
            .workspace_ids,
        [second.id]
    );
    let (_device, member) = joined(&world, &server, &InviteCode::new(copied), "Mira");
    let workspaces = world.run(member.api().workspaces()).unwrap();
    assert_eq!(
        workspaces
            .iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>(),
        [second.id]
    );
    click_in_page(&mut window, "invite-code-done");
    assert!(!window.shows("invite-code-copy"));

    click_in_page(&mut window, "invite-new");
    click_in_page(&mut window, "invite-create");
    let unused = awaited(&mut window, "a second invite is shown", |window| {
        server_page!(window, Invites)?.made
    });
    click_in_page(&mut window, "invite-code-done");
    click_in_page(&mut window, sel("invite-revoke", unused.invite_id));
    window.until("the invite is shown as revoked", |window| {
        invite_line(window, unused.invite_id).is_some_and(|row| row.state == InviteState::Revoked)
    });
    reveal_in_page(&mut window, sel("invite-row", unused.invite_id));
    assert_drawn_label(&mut window, "Revoked");
    assert!(!window.shows(sel("invite-revoke", unused.invite_id)));
    let stored = world.run(owner.api().invites()).unwrap();
    assert!(
        stored
            .iter()
            .find(|row| row.id == unused.invite_id)
            .unwrap()
            .revocation
            .is_some()
    );
    let device = Device::new();
    let refused = world.run(async {
        device
            .add(&server.address())
            .await
            .dispatch(join(&unused.code, "Nora"))
            .await
    });
    assert!(refused.is_err());
}

#[gpui::test]
fn a_join_request_is_approved_for_a_nondefault_workspace(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let default = default_workspace(&world, &owner);
    let second = world
        .run(owner.api().create_workspace("Second".to_owned()))
        .unwrap();
    let (_device, requester, request) = asks_to_join(&world, &server, "Mira");
    window.open_administration();
    open_page(&mut window, "admin-nav-requests", |window| {
        server_page!(window, JoinRequests).is_some()
    });
    click_in_page(&mut window, sel("request-workspace", default));
    click_in_page(&mut window, sel("request-workspace", second.id));
    click_in_page(&mut window, sel("request-workspace", default));
    click_in_page(&mut window, sel("request-approve", request));
    window.until("the approved request leaves the page", |window| {
        window.shows("requests-none")
    });
    world.run(requester.dispatch(AccessAction::Check)).unwrap();
    let id = logged_in(&requester);
    assert!(
        world
            .run(owner.api().workspace_members(second.id))
            .unwrap()
            .iter()
            .any(|row| row.account_id == id)
    );
    assert!(
        world
            .run(owner.api().workspace_members(default))
            .unwrap()
            .iter()
            .all(|row| row.account_id != id)
    );
}

#[gpui::test]
fn an_administrator_revokes_another_accounts_access_token(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Mira");
    grant_server(
        &world,
        &owner,
        logged_in(&member),
        ServerPermission::Administrator,
    );
    let made = world
        .run(member.api().create_access_token(&CreateAccessToken {
            name: "Other account".to_owned(),
            actions: vec![TokenAction::ReadStructure],
            expires_at: OffsetDateTime::now_utc() + Duration::days(7),
        }))
        .unwrap();
    let with_token = with_access_token(&world, &server, made.token.expose());
    assert!(world.run(with_token.workspaces()).is_ok());
    window.open_administration();
    open_page(&mut window, "admin-nav-tokens", |window| {
        server_page!(window, Tokens).is_some()
    });
    let row = awaited(
        &mut window,
        "the other account's token is drawn",
        |window| token_line(window, made.access_token.id),
    );
    assert!(!row.own);
    assert_eq!(row.created_by, logged_in(&member));
    click_in_page(&mut window, sel("token-revoke", row.id));
    window.until("the revoked token leaves the page", |window| {
        window.shows("no-tokens")
    });
    assert_eq!(
        world.run(with_token.workspaces()).unwrap_err().category(),
        Some(ErrorCategory::SessionEnded)
    );
}

#[gpui::test]
fn a_member_redeems_a_further_workspace_and_switches_with_its_row(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let default = default_workspace(&world, &admin.handle);
    let second = world
        .run(admin.handle.api().create_workspace("Second".to_owned()))
        .unwrap();
    text_channel(&world, &admin.handle, second.id, "second-plans");
    let code = invite(&world, &admin.handle, vec![second.id]);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Mira");
    let id = window.own_account();
    assert_eq!(window.view().workspace().unwrap().id, default);
    window.open_administration();
    open_page(&mut window, "admin-nav-workspaces", |window| {
        server_page!(window, Workspaces).is_some()
    });
    click_in_page(&mut window, "redeem-open");
    type_field(&mut window, "redeem-code", code.expose());
    click_in_page(&mut window, "admin-form-send");
    window.until("the further workspace is joined", |window| {
        window.shows("redeemed")
            && server_page!(window, Workspaces).is_some_and(|page| {
                page.rows
                    .iter()
                    .any(|row| row.id == second.id && row.member)
            })
    });
    assert!(
        world
            .run(admin.handle.api().workspace_members(second.id))
            .unwrap()
            .iter()
            .any(|row| row.account_id == id)
    );
    window.click("admin-close");
    if window.shows("expand-server-book") {
        window.click("expand-server-book");
    }
    for workspace in [second.id, default, second.id] {
        window.until("the workspace row is drawn", |window| {
            window.shows(sel("workspace-row", workspace.as_uuid().as_u128()))
        });
        window.click(sel("workspace-row", workspace.as_uuid().as_u128()));
        window.until("the chosen workspace is shown", |window| {
            window
                .view()
                .workspace()
                .is_some_and(|shown| shown.id == workspace)
        });
    }
    window.until("the chosen workspace's channels are drawn", |window| {
        window.shows_channel("second-plans")
    });
    assert_eq!(
        world
            .run(admin.handle.api().workspace_members(default))
            .unwrap()
            .iter()
            .filter(|row| row.account_id == id)
            .count(),
        1
    );
}
