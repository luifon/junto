use crate::{OverlappingOverride, ServerPermissions, WorkspacePermissions};

macro_rules! permission_override {
    ($name:ident, $permissions:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct $name {
            allow: $permissions,
            deny: $permissions,
        }

        impl $name {
            pub const NONE: Self = Self {
                allow: $permissions::EMPTY,
                deny: $permissions::EMPTY,
            };

            pub fn new(
                allow: $permissions,
                deny: $permissions,
            ) -> Result<Self, OverlappingOverride> {
                if allow.intersection(deny).is_empty() {
                    Ok(Self { allow, deny })
                } else {
                    Err(OverlappingOverride)
                }
            }

            pub const fn allow(self) -> $permissions {
                self.allow
            }

            pub const fn deny(self) -> $permissions {
                self.deny
            }

            pub const fn is_none(self) -> bool {
                self.allow.is_empty() && self.deny.is_empty()
            }

            /// The permissions that replacing `self` with `next` can add to a holder of the role.
            pub fn granted_by_change_to(self, next: Self) -> $permissions {
                next.allow
                    .difference(self.allow)
                    .union(self.deny.difference(next.deny))
            }
        }
    };
}

permission_override!(Override, WorkspacePermissions);
permission_override!(ServerOverride, ServerPermissions);
