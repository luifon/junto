//! `roles`: the roles of the server and of a workspace, their permissions, the accounts
//! that hold them and their order (ADR 0309).

use std::cmp::Reverse;
use std::fmt::Display;

use junto_client_core::api::ServerApi;
use junto_client_core::protocol::ServerPermission;
use junto_client_core::protocol::ids::{AccountId, ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_client_core::protocol::roles::{
    CreateServerRole, CreateWorkspaceRole, ServerRole, ServerRolePosition, UpdateServerRole,
    UpdateWorkspaceRole, WorkspaceRole, WorkspaceRolePosition,
};

use crate::admin::{identifier, listed, names, permission, position, word};
use crate::{Arguments, Client, Failure, end_session, failed, logged_in, shown, usage};

enum Action {
    ServerRoles,
    NewServerRole(CreateServerRole),
    ChangeServerRole(ServerRoleId, UpdateServerRole),
    DeleteServerRole(ServerRoleId),
    GiveServerRole(AccountId, ServerRoleId),
    TakeServerRole(AccountId, ServerRoleId),
    MoveServerRole(ServerRoleId, i32),
    WorkspaceRoles(WorkspaceId),
    NewWorkspaceRole(WorkspaceId, CreateWorkspaceRole),
    ChangeWorkspaceRole(WorkspaceId, WorkspaceRoleId, UpdateWorkspaceRole),
    DeleteWorkspaceRole(WorkspaceId, WorkspaceRoleId),
    GiveWorkspaceRole(WorkspaceId, AccountId, WorkspaceRoleId),
    TakeWorkspaceRole(WorkspaceId, AccountId, WorkspaceRoleId),
    MoveWorkspaceRole(WorkspaceId, WorkspaceRoleId, i32),
}

/// A permission of the server by the name that the server writes.
pub(crate) fn server_permission(text: &str) -> Result<ServerPermission, Failure> {
    ServerPermission::ALL
        .iter()
        .find(|held| held.name() == text)
        .copied()
        .ok_or_else(|| usage(format!("{text} is no permission of the server")))
}

/// A list of permissions that commas separate. `one` reads one name.
fn list<T>(text: &str, one: fn(&str) -> Result<T, Failure>) -> Result<Vec<T>, Failure> {
    if text.is_empty() {
        return Err(usage("a list of permissions names at least one permission"));
    }
    text.split(',').map(one).collect()
}

/// The permissions of a new role: the list of `--permissions`, or none.
fn given<T>(arguments: &Arguments, one: fn(&str) -> Result<T, Failure>) -> Result<Vec<T>, Failure> {
    match arguments.value("permissions") {
        Some(text) => list(text, one),
        None => Ok(Vec::new()),
    }
}

/// The whole list of the permissions of a role: the word at `at`, or `--none` for a role
/// without a permission.
fn whole<T>(
    words: &[String],
    at: usize,
    arguments: &Arguments,
    one: fn(&str) -> Result<T, Failure>,
) -> Result<Vec<T>, Failure> {
    match (words.get(at), arguments.flag("none")) {
        (Some(_), true) => Err(usage("a list of permissions and --none exclude each other")),
        (Some(text), false) => list(text, one),
        (None, true) => Ok(Vec::new()),
        (None, false) => Err(usage(
            "the list of the permissions is missing; --none gives the role no permission",
        )),
    }
}

/// The actions of `roles server`. `words` start with the name of the action.
fn server_roles(
    action: &str,
    words: &[String],
    arguments: &Arguments,
) -> Result<Option<Action>, Failure> {
    let role = |at: usize| identifier::<ServerRoleId>(words, at, "the role");
    let account = || identifier::<AccountId>(words, 1, "the account");
    Ok(Some(match action {
        "list" => Action::ServerRoles,
        "create" => Action::NewServerRole(CreateServerRole {
            name: word(words, 1, "the name")?.to_owned(),
            permissions: given(arguments, server_permission)?,
        }),
        "rename" => Action::ChangeServerRole(
            role(1)?,
            UpdateServerRole {
                name: Some(word(words, 2, "the name")?.to_owned()),
                permissions: None,
            },
        ),
        "permissions" => Action::ChangeServerRole(
            role(1)?,
            UpdateServerRole {
                name: None,
                permissions: Some(whole(words, 2, arguments, server_permission)?),
            },
        ),
        "delete" => Action::DeleteServerRole(role(1)?),
        "give" => Action::GiveServerRole(account()?, role(2)?),
        "take" => Action::TakeServerRole(account()?, role(2)?),
        "move" => Action::MoveServerRole(role(1)?, position(word(words, 2, "the position")?)?),
        _ => return Ok(None),
    }))
}

/// The actions of `roles` for a workspace. `words` start with the name of the action,
/// and the workspace follows it.
fn workspace_roles(
    action: &str,
    words: &[String],
    arguments: &Arguments,
) -> Result<Option<Action>, Failure> {
    let workspace = || identifier::<WorkspaceId>(words, 1, "the workspace");
    let role = |at: usize| identifier::<WorkspaceRoleId>(words, at, "the role");
    let account = || identifier::<AccountId>(words, 2, "the account");
    Ok(Some(match action {
        "list" => Action::WorkspaceRoles(workspace()?),
        "create" => Action::NewWorkspaceRole(
            workspace()?,
            CreateWorkspaceRole {
                name: word(words, 2, "the name")?.to_owned(),
                permissions: given(arguments, permission)?,
            },
        ),
        "rename" => Action::ChangeWorkspaceRole(
            workspace()?,
            role(2)?,
            UpdateWorkspaceRole {
                name: Some(word(words, 3, "the name")?.to_owned()),
                permissions: None,
            },
        ),
        "permissions" => Action::ChangeWorkspaceRole(
            workspace()?,
            role(2)?,
            UpdateWorkspaceRole {
                name: None,
                permissions: Some(whole(words, 3, arguments, permission)?),
            },
        ),
        "delete" => Action::DeleteWorkspaceRole(workspace()?, role(2)?),
        "give" => Action::GiveWorkspaceRole(workspace()?, account()?, role(3)?),
        "take" => Action::TakeWorkspaceRole(workspace()?, account()?, role(3)?),
        "move" => Action::MoveWorkspaceRole(
            workspace()?,
            role(2)?,
            position(word(words, 3, "the position")?)?,
        ),
        _ => return Ok(None),
    }))
}

/// Reads the action of `roles` from `words`: `server` and the name of an action for the
/// roles of the server, or the name of an action and a workspace.
fn parse(words: &[String], arguments: &Arguments) -> Result<Action, Failure> {
    let action = word(words, 0, "the action of roles")?;
    if action == "server" {
        let words = words.get(1..).unwrap_or_default();
        let action = word(words, 0, "the action of roles server")?;
        return server_roles(action, words, arguments)?
            .ok_or_else(|| usage(format!("unknown action of roles server: {action}")));
    }
    workspace_roles(action, words, arguments)?
        .ok_or_else(|| usage(format!("unknown action of roles: {action}")))
}

fn role_line(
    id: impl Display,
    position: i32,
    is_everyone: bool,
    name: &str,
    permissions: &str,
) -> String {
    let everyone = if is_everyone { "  everyone" } else { "" };
    format!(
        "role: {id}  position {position}{everyone}  {}  permissions {permissions}",
        shown(name)
    )
}

fn server_role_line(role: &ServerRole) -> String {
    let permissions: Vec<&str> = role.permissions.iter().map(|held| held.name()).collect();
    role_line(
        role.id,
        role.position,
        false,
        &role.name,
        &listed(&permissions),
    )
}

fn workspace_role_line(role: &WorkspaceRole) -> String {
    role_line(
        role.id,
        role.position,
        role.is_everyone,
        &role.name,
        &names(&role.permissions),
    )
}

/// The server roles from the highest position to the lowest.
fn server_role_lines(mut roles: Vec<ServerRole>) -> Vec<String> {
    roles.sort_by_key(|role| (Reverse(role.position), role.id));
    roles.iter().map(server_role_line).collect()
}

/// The roles of a workspace in the order of the state of the client: the "everyone" role,
/// then from the highest position to the lowest.
fn workspace_role_lines(mut roles: Vec<WorkspaceRole>) -> Vec<String> {
    roles.sort_by_key(|role| (!role.is_everyone, Reverse(role.position), role.id));
    roles.iter().map(workspace_role_line).collect()
}

/// The new positions that give `role` the position `to`. `roles` are the roles that can
/// move, each with the position that it holds now. Each role between the old position and
/// the new one takes the position of the role beside it on the side of the old position.
/// The request then gives only positions that its own roles hold, which is what the server
/// accepts (ADR 0309). The result is empty when the role holds the position already.
fn moved<R: Copy + PartialEq>(
    roles: &[(R, i32)],
    role: R,
    to: i32,
) -> Result<Vec<(R, i32)>, Failure> {
    let mut order = roles.to_vec();
    order.sort_by_key(|(_, position)| Reverse(*position));
    let Some(from) = order.iter().position(|(held, _)| *held == role) else {
        return Err(Failure::Failed(
            "the role is not among the roles that can be moved".to_owned(),
        ));
    };
    let Some(target) = order.iter().position(|(_, position)| *position == to) else {
        let positions: Vec<i32> = order.iter().map(|(_, position)| *position).collect();
        return Err(usage(format!(
            "no role holds the position {to}; the positions are {}",
            listed(&positions)
        )));
    };
    let (first, last) = (from.min(target), from.max(target));
    let span = &order[first..=last];
    let mut ids: Vec<R> = span.iter().map(|(held, _)| *held).collect();
    let moving = ids.remove(from - first);
    ids.insert(target - first, moving);
    let held = span.iter().map(|(_, position)| *position);
    let changes = ids.into_iter().zip(held);
    Ok(changes.filter(|change| !span.contains(change)).collect())
}

async fn perform(api: &ServerApi, action: Action) -> Result<Vec<String>, Failure> {
    let done = |()| vec!["done".to_owned()];
    Ok(match action {
        Action::ServerRoles => server_role_lines(api.server_roles().await.map_err(failed)?),
        Action::NewServerRole(role) => vec![server_role_line(
            &api.create_server_role(&role).await.map_err(failed)?,
        )],
        Action::ChangeServerRole(role_id, change) => vec![server_role_line(
            &api.update_server_role(role_id, &change)
                .await
                .map_err(failed)?,
        )],
        Action::DeleteServerRole(role_id) => {
            done(api.delete_server_role(role_id).await.map_err(failed)?)
        }
        Action::GiveServerRole(account_id, role_id) => done(
            api.assign_server_role(account_id, role_id)
                .await
                .map_err(failed)?,
        ),
        Action::TakeServerRole(account_id, role_id) => done(
            api.unassign_server_role(account_id, role_id)
                .await
                .map_err(failed)?,
        ),
        Action::MoveServerRole(role_id, to) => {
            let roles = api.server_roles().await.map_err(failed)?;
            let held: Vec<(ServerRoleId, i32)> =
                roles.iter().map(|role| (role.id, role.position)).collect();
            let positions: Vec<ServerRolePosition> = moved(&held, role_id, to)?
                .into_iter()
                .map(|(role_id, position)| ServerRolePosition { role_id, position })
                .collect();
            // A role that holds the position already needs no request.
            if !positions.is_empty() {
                api.reorder_server_roles(positions).await.map_err(failed)?;
            }
            done(())
        }
        Action::WorkspaceRoles(workspace_id) => {
            workspace_role_lines(api.workspace_roles(workspace_id).await.map_err(failed)?)
        }
        Action::NewWorkspaceRole(workspace_id, role) => vec![workspace_role_line(
            &api.create_workspace_role(workspace_id, &role)
                .await
                .map_err(failed)?,
        )],
        Action::ChangeWorkspaceRole(workspace_id, role_id, change) => vec![workspace_role_line(
            &api.update_workspace_role(workspace_id, role_id, &change)
                .await
                .map_err(failed)?,
        )],
        Action::DeleteWorkspaceRole(workspace_id, role_id) => done(
            api.delete_workspace_role(workspace_id, role_id)
                .await
                .map_err(failed)?,
        ),
        Action::GiveWorkspaceRole(workspace_id, account_id, role_id) => done(
            api.assign_workspace_role(workspace_id, account_id, role_id)
                .await
                .map_err(failed)?,
        ),
        Action::TakeWorkspaceRole(workspace_id, account_id, role_id) => done(
            api.unassign_workspace_role(workspace_id, account_id, role_id)
                .await
                .map_err(failed)?,
        ),
        Action::MoveWorkspaceRole(workspace_id, role_id, to) => {
            let roles = api.workspace_roles(workspace_id).await.map_err(failed)?;
            // The "everyone" role does not move, and no role takes its position.
            let held: Vec<(WorkspaceRoleId, i32)> = roles
                .iter()
                .filter(|role| !role.is_everyone)
                .map(|role| (role.id, role.position))
                .collect();
            let positions: Vec<WorkspaceRolePosition> = moved(&held, role_id, to)?
                .into_iter()
                .map(|(role_id, position)| WorkspaceRolePosition { role_id, position })
                .collect();
            if !positions.is_empty() {
                api.reorder_workspace_roles(workspace_id, positions)
                    .await
                    .map_err(failed)?;
            }
            done(())
        }
    })
}

/// `roles`: one action in one process.
pub(crate) async fn run(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let words = arguments.positional.get(1..).unwrap_or_default();
    // An action that the tool does not know is told before the server is contacted.
    let action = parse(words, arguments)?;
    let server = logged_in(client, arguments).await?;
    let lines = perform(&server.api(), action).await;
    end_session(&server).await;
    for line in lines? {
        crate::live::say(&line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::WorkspacePermission;

    use super::*;

    /// The action of `roles`, from the words after the name of the command.
    fn parsed(words: &[&str]) -> Result<Action, Failure> {
        let arguments = match Arguments::parse(words.iter().map(|word| (*word).to_owned())) {
            Ok(arguments) => arguments,
            Err(_) => panic!("the arguments are not read"),
        };
        parse(&arguments.positional, &arguments)
    }

    fn refused(words: &[&str]) -> bool {
        matches!(parsed(words), Err(Failure::Usage(_)))
    }

    /// The new positions of a move, or nothing when the move is refused.
    fn positions(roles: &[(char, i32)], role: char, to: i32) -> Option<Vec<(char, i32)>> {
        moved(roles, role, to).ok()
    }

    #[test]
    fn the_roles_of_the_server_take_their_actions_after_the_word_server() {
        let (role_id, account_id) = (ServerRoleId::generate(), AccountId::generate());
        let (role, account) = (role_id.to_string(), account_id.to_string());
        let (role, account) = (role.as_str(), account.as_str());

        let list = parsed(&["server", "list"]);
        assert!(matches!(list, Ok(Action::ServerRoles)));
        let Ok(Action::NewServerRole(created)) = parsed(&[
            "server",
            "create",
            "inviters",
            "--permissions",
            "create_invites,approve_join_requests",
        ]) else {
            panic!("the new role is not read");
        };
        assert_eq!(
            created,
            CreateServerRole {
                name: "inviters".to_owned(),
                permissions: vec![
                    ServerPermission::CreateInvites,
                    ServerPermission::ApproveJoinRequests
                ],
            }
        );
        assert!(matches!(
            parsed(&["server", "create", "guests"]),
            Ok(Action::NewServerRole(created)) if created.permissions.is_empty()
        ));
        assert!(matches!(
            parsed(&["server", "rename", role, "hosts"]),
            Ok(Action::ChangeServerRole(id, change))
                if id == role_id
                    && change.name.as_deref() == Some("hosts")
                    && change.permissions.is_none()
        ));
        assert!(matches!(
            parsed(&["server", "delete", role]),
            Ok(Action::DeleteServerRole(id)) if id == role_id
        ));
        assert!(matches!(
            parsed(&["server", "give", account, role]),
            Ok(Action::GiveServerRole(to, id)) if to == account_id && id == role_id
        ));
        assert!(matches!(
            parsed(&["server", "take", account, role]),
            Ok(Action::TakeServerRole(from, id)) if from == account_id && id == role_id
        ));
        assert!(matches!(
            parsed(&["server", "move", role, "3"]),
            Ok(Action::MoveServerRole(id, 3)) if id == role_id
        ));

        assert!(refused(&["server"]));
        assert!(refused(&["server", "destroy"]));
        assert!(refused(&["server", "create"]));
        assert!(refused(&["server", "rename", role]));
        assert!(refused(&["server", "rename", "hosts", "guests"]));
        assert!(refused(&["server", "give", account]));
        assert!(refused(&["server", "move", role]));
        assert!(refused(&["server", "move", role, "first"]));
        for held in ServerPermission::ALL {
            assert!(matches!(server_permission(held.name()), Ok(read) if read == *held));
        }
    }

    #[test]
    fn the_roles_of_a_workspace_take_the_workspace_after_the_action() {
        let (workspace_id, role_id) = (WorkspaceId::generate(), WorkspaceRoleId::generate());
        let account_id = AccountId::generate();
        let (workspace, role) = (workspace_id.to_string(), role_id.to_string());
        let (workspace, role) = (workspace.as_str(), role.as_str());
        let account = account_id.to_string();
        let account = account.as_str();

        assert!(matches!(
            parsed(&["list", workspace]),
            Ok(Action::WorkspaceRoles(id)) if id == workspace_id
        ));
        let Ok(Action::NewWorkspaceRole(in_workspace, created)) = parsed(&[
            "create",
            workspace,
            "helpers",
            "--permissions",
            "manage_messages",
        ]) else {
            panic!("the new role is not read");
        };
        assert_eq!(in_workspace, workspace_id);
        assert_eq!(
            created,
            CreateWorkspaceRole {
                name: "helpers".to_owned(),
                permissions: vec![WorkspacePermission::ManageMessages],
            }
        );
        assert!(matches!(
            parsed(&["rename", workspace, role, "leads"]),
            Ok(Action::ChangeWorkspaceRole(in_workspace, id, change))
                if in_workspace == workspace_id
                    && id == role_id
                    && change.name.as_deref() == Some("leads")
                    && change.permissions.is_none()
        ));
        assert!(matches!(
            parsed(&["delete", workspace, role]),
            Ok(Action::DeleteWorkspaceRole(in_workspace, id))
                if in_workspace == workspace_id && id == role_id
        ));
        assert!(matches!(
            parsed(&["give", workspace, account, role]),
            Ok(Action::GiveWorkspaceRole(in_workspace, to, id))
                if in_workspace == workspace_id && to == account_id && id == role_id
        ));
        assert!(matches!(
            parsed(&["take", workspace, account, role]),
            Ok(Action::TakeWorkspaceRole(in_workspace, from, id))
                if in_workspace == workspace_id && from == account_id && id == role_id
        ));
        assert!(matches!(
            parsed(&["move", workspace, role, "2"]),
            Ok(Action::MoveWorkspaceRole(in_workspace, id, 2))
                if in_workspace == workspace_id && id == role_id
        ));

        assert!(refused(&[]));
        assert!(refused(&["destroy", workspace]));
        assert!(refused(&["list"]));
        assert!(refused(&["list", "Acme"]));
        assert!(refused(&["create", workspace]));
        assert!(refused(&["rename", workspace, role]));
        assert!(refused(&["give", workspace, account]));
        assert!(refused(&["move", workspace, role]));
        assert!(refused(&["move", workspace, role, "up"]));
    }

    #[test]
    fn the_permissions_of_a_role_are_one_whole_list_of_its_level_or_none() {
        let workspace = WorkspaceId::generate().to_string();
        let workspace = workspace.as_str();
        let server_role = ServerRoleId::generate().to_string();
        let server_role = server_role.as_str();
        let role = WorkspaceRoleId::generate().to_string();
        let role = role.as_str();

        let of_server = |options: &[&str]| {
            let mut words = vec!["server", "permissions", server_role];
            words.extend(options);
            match parsed(&words) {
                Ok(Action::ChangeServerRole(_, change)) => Some(change),
                _ => None,
            }
        };
        assert_eq!(
            of_server(&["ban_members,view_server_audit_log"]),
            Some(UpdateServerRole {
                name: None,
                permissions: Some(vec![
                    ServerPermission::BanMembers,
                    ServerPermission::ViewServerAuditLog
                ]),
            })
        );
        assert_eq!(
            of_server(&["--none"]),
            Some(UpdateServerRole {
                name: None,
                permissions: Some(Vec::new()),
            })
        );
        assert_eq!(of_server(&[]), None);
        assert_eq!(of_server(&["ban_members", "--none"]), None);
        assert_eq!(of_server(&["ban_members,"]), None);
        assert_eq!(of_server(&["ban_members, administrator"]), None);
        assert_eq!(of_server(&["BanMembers"]), None);
        // A permission of a workspace is no permission of a server role.
        assert_eq!(of_server(&["view_channel"]), None);

        let of_workspace = |options: &[&str]| {
            let mut words = vec!["permissions", workspace, role];
            words.extend(options);
            match parsed(&words) {
                Ok(Action::ChangeWorkspaceRole(_, _, change)) => Some(change),
                _ => None,
            }
        };
        assert_eq!(
            of_workspace(&["view_channel,lock_room"]),
            Some(UpdateWorkspaceRole {
                name: None,
                permissions: Some(vec![
                    WorkspacePermission::ViewChannel,
                    WorkspacePermission::LockRoom
                ]),
            })
        );
        assert_eq!(
            of_workspace(&["--none"]),
            Some(UpdateWorkspaceRole {
                name: None,
                permissions: Some(Vec::new()),
            })
        );
        assert_eq!(of_workspace(&[]), None);
        assert_eq!(of_workspace(&["view_channel", "--none"]), None);
        assert_eq!(of_workspace(&["administrator"]), None);

        assert!(refused(&[
            "create",
            workspace,
            "helpers",
            "--permissions",
            "ban_members"
        ]));
        assert!(refused(&[
            "server",
            "create",
            "inviters",
            "--permissions",
            "view_channel"
        ]));
        assert!(refused(&[
            "server",
            "create",
            "inviters",
            "--permissions",
            ""
        ]));
    }

    #[test]
    fn the_line_of_a_role_shows_its_position_its_name_and_its_permissions() {
        let inviters = ServerRole {
            position: 2,
            id: ServerRoleId::generate(),
            name: "inviters\u{1b}[2J".to_owned(),
            permissions: vec![
                ServerPermission::CreateInvites,
                ServerPermission::ApproveJoinRequests,
            ],
        };
        assert_eq!(
            server_role_line(&inviters),
            format!(
                "role: {}  position 2  inviters[2J  permissions \
                 create_invites,approve_join_requests",
                inviters.id
            )
        );
        let hosts = ServerRole {
            position: 1,
            id: ServerRoleId::generate(),
            name: "hosts".to_owned(),
            permissions: Vec::new(),
        };
        assert_eq!(
            server_role_line(&hosts),
            format!("role: {}  position 1  hosts  permissions none", hosts.id)
        );
        // The list goes from the highest position to the lowest.
        assert_eq!(
            server_role_lines(vec![hosts.clone(), inviters.clone()]),
            [server_role_line(&inviters), server_role_line(&hosts)]
        );

        let workspace_id = WorkspaceId::generate();
        let role = |name: &str, position: i32| WorkspaceRole {
            position,
            id: WorkspaceRoleId::generate(),
            workspace_id,
            name: name.to_owned(),
            permissions: vec![WorkspacePermission::ViewChannel],
            is_everyone: position == 0,
        };
        let everyone = role("everyone", 0);
        let (helpers, leads) = (role("helpers", 1), role("leads", 2));
        assert_eq!(
            workspace_role_line(&everyone),
            format!(
                "role: {}  position 0  everyone  everyone  permissions view_channel",
                everyone.id
            )
        );
        assert_eq!(
            workspace_role_line(&leads),
            format!(
                "role: {}  position 2  leads  permissions view_channel",
                leads.id
            )
        );
        // The "everyone" role stands first, then the other roles from the highest position.
        assert_eq!(
            workspace_role_lines(vec![helpers.clone(), leads.clone(), everyone.clone()]),
            [
                workspace_role_line(&everyone),
                workspace_role_line(&leads),
                workspace_role_line(&helpers)
            ]
        );
    }

    #[test]
    fn a_move_gives_each_role_between_the_two_positions_the_position_beside_it() {
        // The roles in the order in which a server gave them, not in the order of the
        // positions.
        let roles = [('c', 2), ('a', 4), ('d', 1), ('b', 3)];

        // Down: the roles between move up one position each.
        assert_eq!(
            positions(&roles, 'a', 2),
            Some(vec![('b', 4), ('c', 3), ('a', 2)])
        );
        assert_eq!(
            positions(&roles, 'a', 1),
            Some(vec![('b', 4), ('c', 3), ('d', 2), ('a', 1)])
        );
        // Up: the roles between move down one position each.
        assert_eq!(
            positions(&roles, 'd', 3),
            Some(vec![('d', 3), ('b', 2), ('c', 1)])
        );
        assert_eq!(positions(&roles, 'c', 3), Some(vec![('c', 3), ('b', 2)]));

        // The request gives the positions that its roles hold, and no other position.
        for (role, to) in [('a', 1), ('d', 4), ('b', 2), ('c', 4)] {
            let Some(changes) = positions(&roles, role, to) else {
                panic!("the move of {role} is not computed");
            };
            let mut given: Vec<i32> = changes.iter().map(|(_, position)| *position).collect();
            let mut taken: Vec<i32> = changes
                .iter()
                .filter_map(|(changed, _)| roles.iter().find(|(held, _)| held == changed))
                .map(|(_, position)| *position)
                .collect();
            given.sort_unstable();
            taken.sort_unstable();
            assert_eq!(given, taken);
            assert!(changes.contains(&(role, to)));
        }
    }

    #[test]
    fn a_move_to_the_own_position_changes_nothing_and_another_position_must_be_held() {
        let roles = [('a', 4), ('b', 3), ('c', 1)];
        assert_eq!(positions(&roles, 'b', 3), Some(Vec::new()));

        // The position 2 is held by no role, and the position 0 belongs to no role that
        // can move.
        for to in [2, 0, 5, -1] {
            let Err(Failure::Usage(problem)) = moved(&roles, 'a', to) else {
                panic!("the position {to} is taken");
            };
            assert!(problem.ends_with("the positions are 4,3,1"), "{problem}");
        }
        // A role that the list does not hold is no usage failure: the server gave the list.
        assert!(matches!(moved(&roles, 'd', 3), Err(Failure::Failed(_))));
        assert!(matches!(moved(&[], 'a', 1), Err(Failure::Failed(_))));
    }
}
