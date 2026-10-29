use junto_client_core::protocol::{
    appearance::{AccountAppearance, RoleColor},
    ids::WorkspaceRoleId,
    roles::WorkspaceRole,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberGroup {
    pub workspace: bool,
    pub id: uuid::Uuid,
    pub name: String,
    pub position: i32,
    pub color: Option<RoleColor>,
}

pub fn appearance(
    roles: &[WorkspaceRole],
    held: &[WorkspaceRoleId],
    fallback: &AccountAppearance,
) -> (Option<RoleColor>, Option<MemberGroup>) {
    let assigned = || {
        roles
            .iter()
            .filter(|role| role.is_everyone || held.contains(&role.id))
    };
    let color = assigned()
        .filter(|role| role.color.is_some())
        .max_by_key(|role| (role.position, std::cmp::Reverse(role.id)))
        .and_then(|role| role.color)
        .or(fallback.color);
    let group = assigned()
        .filter(|role| role.hoist && !role.is_everyone)
        .max_by_key(|role| (role.position, std::cmp::Reverse(role.id)))
        .map(|role| MemberGroup {
            workspace: true,
            id: role.id.as_uuid(),
            name: crate::text::label(&role.name),
            position: role.position,
            color: role.color,
        })
        .or_else(|| {
            fallback.group.as_ref().map(|role| MemberGroup {
                workspace: false,
                id: role.id.as_uuid(),
                name: crate::text::label(&role.name),
                position: role.position,
                color: role.color,
            })
        });
    (color, group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_client_core::protocol::{
        appearance::HoistedRole,
        ids::{ServerRoleId, WorkspaceId},
    };

    #[test]
    fn colour_and_group_choose_independent_highest_roles_and_fall_back_separately() {
        let workspace = WorkspaceId::generate();
        let make = |position: i32, color, hoist, everyone| WorkspaceRole {
            id: WorkspaceRoleId::generate(),
            workspace_id: workspace,
            name: position.to_string(),
            position,
            color,
            hoist,
            is_everyone: everyone,
            permissions: Vec::new(),
        };
        let blue = RoleColor::from_rgb(51, 102, 153);
        let green = RoleColor::from_rgb(34, 119, 68);
        let fallback = AccountAppearance {
            color: Some(blue),
            group: Some(HoistedRole {
                id: ServerRoleId::generate(),
                name: "Server".to_owned(),
                position: 500,
                color: Some(blue),
            }),
        };
        let everyone = make(0, Some(green), false, true);
        let coloured = make(2, Some(blue), false, false);
        let grouped = make(3, None, true, false);
        let unheld = make(9, Some(green), true, false);
        let held = vec![coloured.id, grouped.id];
        let roles = vec![everyone, coloured, grouped, unheld];
        let (color, group) = appearance(&roles, &held, &fallback);
        assert_eq!(color, Some(blue));
        assert_eq!(group.unwrap().id, roles[2].id.as_uuid());
        let (color, group) = appearance(&roles, &[], &fallback);
        assert_eq!(color, Some(green));
        assert!(!group.unwrap().workspace);
        let (color, group) = appearance(&[], &[], &fallback);
        assert_eq!(color, Some(blue));
        assert_eq!(group.unwrap().id, fallback.group.unwrap().id.as_uuid());
    }
}
