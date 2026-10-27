//! The permission model of Junto and the single function that evaluates it.
//!
//! Every permission decision of the server and of the client goes through [`authorize`].

mod evaluate;
mod model;
mod overrides;
mod permissions;
pub use overrides::{Override, ServerOverride};

pub use evaluate::authorize;
pub use model::{
    Acquaintance, Actor, Admission, AuditScope, Authorship, ChannelContent, ChannelGrant,
    ChannelScope, Contact, Decision, Denial, Departing, HeldRole, Membership, MessagePlace,
    OverlappingOverride, OwnerAction, Participation, Permanence, Position, Reach, Request, RoleKey,
    RoleOverride, Target, WorkspaceScope,
};
pub use permissions::{
    ServerPermission, ServerPermissions, WorkspacePermission, WorkspacePermissions,
};

mod role_order;
pub use role_order::{RoleRank, outranks, role_rank};
