//! The permission model of Junto and the single function that evaluates it.
//!
//! Every permission decision of the server and of the client goes through [`authorize`].

mod evaluate;
mod model;
mod permissions;

pub use evaluate::authorize;
pub use model::{
    Actor, Admission, AuditScope, Authorship, ChannelContent, ChannelGrant, ChannelScope, Decision,
    Denial, HeldRole, Membership, MessagePlace, OverlappingOverride, Override, OwnerAction,
    Participation, Permanence, Position, Reach, Request, RoleKey, RoleOverride, Target,
    WorkspaceScope,
};
pub use permissions::{
    ServerPermission, ServerPermissions, WorkspacePermission, WorkspacePermissions,
};
