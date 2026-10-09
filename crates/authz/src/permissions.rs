macro_rules! permission_set {
    (
        $(#[$permission_meta:meta])*
        permission $permission:ident;
        $(#[$set_meta:meta])*
        set $set:ident;
        $( $variant:ident = $bit:literal => $name:literal, )+
    ) => {
        $(#[$permission_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
        pub enum $permission {
            $(
                #[cfg_attr(feature = "serde", serde(rename = $name))]
                $variant = $bit,
            )+
        }

        impl $permission {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            const fn mask(self) -> u64 {
                1 << (self as u8)
            }
        }

        $(#[$set_meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
        pub struct $set(u64);

        impl $set {
            pub const EMPTY: Self = Self(0);
            pub const ALL: Self = Self(0 $(| (1 << $bit))+);

            /// Returns `None` when `bits` has a bit that names no permission.
            pub const fn from_bits(bits: u64) -> Option<Self> {
                if bits & !Self::ALL.0 == 0 {
                    Some(Self(bits))
                } else {
                    None
                }
            }

            pub const fn bits(self) -> u64 {
                self.0
            }

            pub const fn is_empty(self) -> bool {
                self.0 == 0
            }

            pub const fn contains(self, permission: $permission) -> bool {
                self.0 & permission.mask() != 0
            }

            pub const fn contains_all(self, other: Self) -> bool {
                other.0 & !self.0 == 0
            }

            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            pub const fn intersection(self, other: Self) -> Self {
                Self(self.0 & other.0)
            }

            pub const fn difference(self, other: Self) -> Self {
                Self(self.0 & !other.0)
            }

            pub const fn with(self, permission: $permission) -> Self {
                Self(self.0 | permission.mask())
            }

            pub fn iter(self) -> impl Iterator<Item = $permission> {
                $permission::ALL
                    .iter()
                    .copied()
                    .filter(move |permission| self.contains(*permission))
            }
        }

        impl From<$permission> for $set {
            fn from(permission: $permission) -> Self {
                Self(permission.mask())
            }
        }

        impl FromIterator<$permission> for $set {
            fn from_iter<I: IntoIterator<Item = $permission>>(permissions: I) -> Self {
                permissions
                    .into_iter()
                    .fold(Self::EMPTY, |set, permission| set.with(permission))
            }
        }

        impl std::fmt::Debug for $set {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter
                    .debug_set()
                    .entries(self.iter().map($permission::name))
                    .finish()
            }
        }
    };
}

permission_set! {
    /// A permission that acts on the whole server. The number is the bit of the stored set.
    permission ServerPermission;
    /// A set of server permissions.
    set ServerPermissions;
    Administrator = 0 => "administrator",
    CreateWorkspace = 1 => "create_workspace",
    CreateInvites = 2 => "create_invites",
    ApproveJoinRequests = 3 => "approve_join_requests",
    BanMembers = 4 => "ban_members",
    ViewServerAuditLog = 5 => "view_server_audit_log",
}

permission_set! {
    /// A permission inside one workspace. The number is the bit of the stored set.
    permission WorkspacePermission;
    /// A set of workspace permissions.
    set WorkspacePermissions;
    ViewChannel = 0 => "view_channel",
    ManageRoles = 1 => "manage_roles",
    ManageChannel = 2 => "manage_channel",
    ManageMessages = 3 => "manage_messages",
    MentionEveryone = 4 => "mention_everyone",
    ManageGlossary = 5 => "manage_glossary",
    RecordCalls = 6 => "record_calls",
    ManageRecordings = 7 => "manage_recordings",
    ExportChannel = 8 => "export_channel",
    MoveMembers = 9 => "move_members",
    MuteMembers = 10 => "mute_members",
    DeafenMembers = 11 => "deafen_members",
    DisconnectMembers = 12 => "disconnect_members",
    LockRoom = 13 => "lock_room",
    RunMeetingMode = 14 => "run_meeting_mode",
    ViewAuditEntries = 15 => "view_audit_entries",
}

impl WorkspacePermissions {
    /// The permissions of the "everyone" role of a new workspace.
    pub const EVERYONE_DEFAULT: Self = Self::EMPTY
        .with(WorkspacePermission::ViewChannel)
        .with(WorkspacePermission::LockRoom)
        .with(WorkspacePermission::RunMeetingMode);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_bits_are_stable() {
        let server: Vec<(u64, &str)> = ServerPermission::ALL
            .iter()
            .map(|permission| {
                (
                    ServerPermissions::from(*permission).bits(),
                    permission.name(),
                )
            })
            .collect();
        assert_eq!(
            server,
            [
                (1, "administrator"),
                (2, "create_workspace"),
                (4, "create_invites"),
                (8, "approve_join_requests"),
                (16, "ban_members"),
                (32, "view_server_audit_log"),
            ]
        );
        let workspace: Vec<(u64, &str)> = WorkspacePermission::ALL
            .iter()
            .map(|permission| {
                (
                    WorkspacePermissions::from(*permission).bits(),
                    permission.name(),
                )
            })
            .collect();
        assert_eq!(
            workspace,
            [
                (1 << 0, "view_channel"),
                (1 << 1, "manage_roles"),
                (1 << 2, "manage_channel"),
                (1 << 3, "manage_messages"),
                (1 << 4, "mention_everyone"),
                (1 << 5, "manage_glossary"),
                (1 << 6, "record_calls"),
                (1 << 7, "manage_recordings"),
                (1 << 8, "export_channel"),
                (1 << 9, "move_members"),
                (1 << 10, "mute_members"),
                (1 << 11, "deafen_members"),
                (1 << 12, "disconnect_members"),
                (1 << 13, "lock_room"),
                (1 << 14, "run_meeting_mode"),
                (1 << 15, "view_audit_entries"),
            ]
        );
    }

    #[test]
    fn from_bits_rejects_unknown_bits() {
        assert_eq!(
            ServerPermissions::from_bits(0),
            Some(ServerPermissions::EMPTY)
        );
        assert_eq!(
            ServerPermissions::from_bits(63),
            Some(ServerPermissions::ALL)
        );
        assert_eq!(ServerPermissions::from_bits(64), None);
        assert_eq!(
            WorkspacePermissions::from_bits(0xFFFF),
            Some(WorkspacePermissions::ALL)
        );
        assert_eq!(WorkspacePermissions::from_bits(0x1_0000), None);
    }

    #[test]
    fn set_operations() {
        let lock = WorkspacePermissions::from(WorkspacePermission::LockRoom);
        let both = lock.with(WorkspacePermission::ManageRoles);
        assert!(both.contains(WorkspacePermission::ManageRoles));
        assert!(both.contains_all(lock));
        assert!(!lock.contains_all(both));
        assert_eq!(
            both.difference(lock),
            WorkspacePermission::ManageRoles.into()
        );
        assert_eq!(both.intersection(lock), lock);
        assert_eq!(lock.union(both), both);
        assert_eq!(
            both.iter().collect::<Vec<_>>(),
            [
                WorkspacePermission::ManageRoles,
                WorkspacePermission::LockRoom
            ]
        );
        assert_eq!(both.iter().collect::<WorkspacePermissions>(), both);
        assert!(WorkspacePermissions::EMPTY.is_empty());
    }

    #[test]
    fn everyone_default_matches_the_member_defaults() {
        assert_eq!(
            WorkspacePermissions::EVERYONE_DEFAULT
                .iter()
                .collect::<Vec<_>>(),
            [
                WorkspacePermission::ViewChannel,
                WorkspacePermission::LockRoom,
                WorkspacePermission::RunMeetingMode
            ]
        );
    }
}
