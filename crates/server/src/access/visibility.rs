//! Batched standing for search, notices and their bounded delivery candidates.
use super::{Access, channel_scope, role_key};
use crate::{db, error::ApiResult};
use junto_authz::{
    Actor, Contact, HeldRole, Membership, Position, Request, WorkspacePermission, WorkspaceScope,
};
use junto_protocol::ids::{AccountId, ChannelId, ConversationId, WorkspaceId, WorkspaceRoleId};
use serde::Deserialize;
use sqlx::{PgConnection, types::Json};
use uuid::Uuid;

#[derive(Default)]
pub struct Places {
    pub workspaces: Vec<WorkspaceId>,
    pub channels: Vec<ChannelId>,
    pub directs: Vec<ConversationId>,
}

impl Places {
    pub fn ids(&self) -> Vec<Uuid> {
        self.channels
            .iter()
            .map(|id| id.as_uuid())
            .chain(self.directs.iter().map(|id| id.as_uuid()))
            .collect()
    }
}

#[derive(Deserialize)]
struct Workspace {
    id: WorkspaceId,
    member: bool,
    roles: Vec<(WorkspaceRoleId, i64, bool)>,
}

#[derive(Deserialize)]
struct Channel {
    id: ChannelId,
    workspace_id: WorkspaceId,
    overrides: Vec<(ChannelId, WorkspaceRoleId, bool, i64, i64)>,
}

#[derive(Deserialize)]
struct Direct {
    id: ConversationId,
    shared: bool,
    owner: bool,
    roles: Vec<i64>,
    /// The other participant is inactive or deleted: the conversation is read only
    /// (ADR 0121).
    departed: bool,
}

#[derive(Deserialize)]
struct Standing {
    account_id: AccountId,
    owner: bool,
    roles: Vec<i64>,
    workspaces: Vec<Workspace>,
    channels: Vec<Channel>,
    directs: Vec<Direct>,
}

fn make_actor(owner: bool, roles: Vec<i64>) -> ApiResult<Actor> {
    Ok(Actor {
        position: if owner {
            Position::Owner
        } else {
            Position::Member
        },
        server_roles: roles
            .into_iter()
            .map(db::server_permissions)
            .collect::<ApiResult<_>>()?,
    })
}

pub async fn places(connection: &mut PgConnection, access: &Access) -> ApiResult<Places> {
    Ok(load(connection, &[access.account_id], None, true)
        .await?
        .pop()
        .map(|(_, places)| places)
        .unwrap_or_default())
}

/// With named places, no unrelated channel or conversation is loaded. Notification delivery
/// passes only the place identifiers of its at most 500 candidate messages.
pub async fn load(
    connection: &mut PgConnection,
    accounts: &[AccountId],
    named: Option<&[Uuid]>,
    directs: bool,
) -> ApiResult<Vec<(AccountId, Places)>> {
    let rows: Vec<Json<Standing>> = sqlx::query_scalar("with channels_selected as materialized ( select id,workspace_id from channels where deleted_at is null and
             ($2::uuid[] is null or id=any($2)) ), overrides as materialized ( select
             o.channel_id,jsonb_agg(jsonb_build_array(o.channel_id,o.role_id,r.is_everyone,o.allow,o.deny)) as rules
             from channel_overrides o join workspace_roles r on r.id=o.role_id where o.channel_id in(select id from
             channels_selected) group by o.channel_id ), channels_json as ( select
             coalesce(jsonb_agg(jsonb_build_object( 'id' ,c.id, 'workspace_id' ,c.workspace_id, 'overrides'
             ,coalesce(o.rules, '[]' ::jsonb))), '[]' ::jsonb) as value from channels_selected c left join overrides o
             on o.channel_id=c.id ) select jsonb_build_object( 'account_id' ,a.id, 'owner'
             ,coalesce(a.id=s.owner_account_id,false), 'roles' ,array(select r.permissions from server_roles r join
             account_server_roles ar on ar.role_id=r.id where ar.account_id=a.id), 'channels' ,cj.value, 'workspaces'
             ,coalesce((select jsonb_agg(jsonb_build_object( 'id' ,w.id, 'member' ,exists(select 1 from
             workspace_members wm where wm.workspace_id=w.id and wm.account_id=a.id), 'roles' ,(select
             jsonb_agg(jsonb_build_array(r.id,r.permissions,r.is_everyone)) from workspace_roles r where
             r.workspace_id=w.id and (r.is_everyone or exists( select 1 from member_roles mr where mr.role_id=r.id and
             mr.account_id=a.id))))) from workspaces w where w.deleted_at is null and ($2::uuid[] is null or w.id in(select workspace_id from
             channels_selected))), '[]' ::jsonb), 'directs' ,coalesce((select jsonb_agg(jsonb_build_object( 'id' ,c.id,
             'departed' ,peer.status in ('inactive', 'deleted'),
             'owner' ,coalesce(peer.id=s.owner_account_id,false), 'roles' ,array(select r.permissions from server_roles
             r join account_server_roles ar on ar.role_id=r.id where ar.account_id=peer.id), 'shared' ,exists(select 1
             from workspace_members mine join workspace_members theirs using(workspace_id) where mine.account_id=a.id
             and theirs.account_id=peer.id))) from conversations c join accounts peer on peer.id=case when
             c.low_account_id=a.id then c.high_account_id else c.low_account_id end where $3 and peer.status in ('active', 'inactive', 'deleted')
             and (c.low_account_id=a.id or c.high_account_id=a.id) and ($2::uuid[] is null or c.id=any($2))), '[]'
             ::jsonb)) from accounts a cross join (select owner_account_id from server_state limit 1) s cross join channels_json cj where a.id=any($1) and
             a.status= 'active'")
        .bind(accounts)
        .bind(named)
        .bind(directs)
        .fetch_all(connection)
        .await?;
    rows.into_iter()
        .map(|Json(row)| {
            let actor = make_actor(row.owner, row.roles)?;
            let mut scopes = std::collections::HashMap::new();
            let mut places = Places::default();
            for workspace in row.workspaces {
                let mut everyone = Default::default();
                let mut roles = Vec::new();
                for (id, bits, all) in workspace.roles {
                    let permissions = db::workspace_permissions(bits)?;
                    if all {
                        everyone = permissions;
                    } else {
                        roles.push(HeldRole {
                            key: role_key(id),
                            permissions,
                        });
                    }
                }
                let scope = WorkspaceScope {
                    membership: if workspace.member {
                        Membership::Member
                    } else {
                        Membership::NotMember
                    },
                    everyone,
                    roles,
                };
                if junto_authz::authorize(&actor, &Request::ViewWorkspace(&scope)).is_allowed() {
                    places.workspaces.push(workspace.id);
                }
                scopes.insert(workspace.id, scope);
            }
            for channel in row.channels {
                if let Some(workspace) = scopes.get(&channel.workspace_id) {
                    let scope = channel_scope(workspace.clone(), &channel.overrides)?;
                    if junto_authz::authorize(
                        &actor,
                        &Request::Channel {
                            scope: &scope,
                            permission: WorkspacePermission::ViewChannel,
                        },
                    )
                    .is_allowed()
                    {
                        places.channels.push(channel.id);
                    }
                }
            }
            for direct in row.directs {
                let contact = if direct.shared {
                    Contact::SharedWorkspace
                } else if make_actor(direct.owner, direct.roles)?.is_administrator() {
                    Contact::Administrator
                } else {
                    Contact::Unrelated
                };
                if direct.departed
                    || junto_authz::authorize(&actor, &Request::Contact(contact)).is_allowed()
                {
                    places.directs.push(direct.id);
                }
            }
            Ok((row.account_id, places))
        })
        .collect()
}
