//! The structure of the server as one document, and changes of it in one transaction
//! (ADR 0204, ADR 0211).

use std::collections::HashMap;

use junto_authz::{Request, ServerPermission, ServerPermissions, WorkspacePermissions};
use junto_protocol::call::UpdateCallSettings;
use junto_protocol::ids::{
    CategoryId, ChannelId, ServerRoleId, StructureId, WorkspaceId, WorkspaceRoleId,
};
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::structure::{ChannelKind, RuleSubject, SetChannelOverride, UpdateChannel};
use junto_protocol::structure_file::{
    ApplyStructureChanges, CategoryEntry, ChannelEntry, CreatedEntry, Deletions,
    OLDEST_STRUCTURE_FORMAT, OverrideEntry, RoleEntry, STRUCTURE_FORMAT, ServerRoleEntry,
    StructureApplied, StructureChange, StructureExport, StructureFile, StructureKind,
    WorkspaceEntry,
};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::access::Access;
use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult};
use crate::routes::channels::{self, NewChannel};
use crate::routes::{calls, server_roles, workspace_roles, workspaces};
use crate::state::AppState;
use crate::{store, validate};

impl StoredText for StructureKind {
    fn stored(self) -> &'static str {
        match self {
            Self::ServerRole => "server_role",
            Self::Workspace => "workspace",
            Self::WorkspaceRole => "workspace_role",
            Self::Category => "category",
            Self::Channel => "channel",
        }
    }

    fn from_stored(text: &str) -> Option<Self> {
        [
            Self::ServerRole,
            Self::Workspace,
            Self::WorkspaceRole,
            Self::Category,
            Self::Channel,
        ]
        .into_iter()
        .find(|kind| kind.stored() == text)
    }
}

/// The statement that tells whether an object of the kind has this identifier of its own. A
/// deleted channel keeps its identifier.
const fn own_identifier(kind: StructureKind) -> &'static str {
    match kind {
        StructureKind::ServerRole => "select exists (select 1 from server_roles where id = $1)",
        StructureKind::Workspace => "select exists (select 1 from workspaces where id = $1)",
        StructureKind::WorkspaceRole => {
            "select exists (select 1 from workspace_roles where id = $1)"
        }
        StructureKind::Category => "select exists (select 1 from categories where id = $1)",
        StructureKind::Channel => "select exists (select 1 from channels where id = $1)",
    }
}

/// The revision of a structure: a hash of the document. Two equal structures have the same
/// revision, and every change of the structure changes it, whichever statement made the
/// change. The server stores no counter: one counter row would be written by every change
/// of the structure, so that no two of them could run at the same time (ADR 0211).
pub fn revision_of(structure: &StructureFile) -> ApiResult<String> {
    use sha2::{Digest, Sha256};
    let document = serde_json::to_vec(structure).map_err(|error| ApiError::internal(&error))?;
    let hash = Sha256::digest(&document);
    Ok(hash[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The identifier of the object that an entry names: the object that a file created under
/// this identifier, or else the object with this identifier of its own. The caller finds out
/// whether that object exists.
async fn resolve(
    connection: &mut PgConnection,
    kind: StructureKind,
    entry: StructureId,
) -> ApiResult<Uuid> {
    let stored: Option<Uuid> = sqlx::query_scalar(
        "select object_id from structure_identifiers where kind = $1 and identifier = $2",
    )
    .bind(kind.stored())
    .bind(entry.as_uuid())
    .fetch_optional(connection)
    .await?;
    Ok(stored.unwrap_or(entry.as_uuid()))
}

/// Refuses an identifier for a new entry that already names an object of the kind.
async fn require_unused(
    connection: &mut PgConnection,
    kind: StructureKind,
    entry: StructureId,
) -> ApiResult<()> {
    let stored: bool = sqlx::query_scalar(
        "select exists (
             select 1 from structure_identifiers where kind = $1 and identifier = $2)",
    )
    .bind(kind.stored())
    .bind(entry.as_uuid())
    .fetch_one(&mut *connection)
    .await?;
    let own: bool = sqlx::query_scalar(own_identifier(kind))
        .bind(entry.as_uuid())
        .fetch_one(connection)
        .await?;
    if stored || own {
        return Err(ApiError::conflict(format!(
            "the identifier {entry} is in use; an identifier of a deleted channel is not used again"
        )));
    }
    Ok(())
}

/// Stores the identifier under which a file created an object.
async fn register(
    connection: &mut PgConnection,
    created: &mut Vec<CreatedEntry>,
    kind: StructureKind,
    entry: StructureId,
    object_id: Uuid,
) -> ApiResult<()> {
    sqlx::query(
        "insert into structure_identifiers (kind, identifier, object_id) values ($1, $2, $3)",
    )
    .bind(kind.stored())
    .bind(entry.as_uuid())
    .bind(object_id)
    .execute(connection)
    .await
    .map_err(ApiError::concurrent)?;
    created.push(CreatedEntry {
        kind,
        entry,
        id: object_id,
    });
    Ok(())
}

/// The statements of the export. Their number does not depend on the size of the
/// structure. They are constants so that a test can read their plans.
pub const EXPORT_SERVER_ROLES: &str =
    "select coalesce(i.identifier, r.id), r.name, r.permissions, r.position
     from server_roles r
     left join structure_identifiers i on i.kind = 'server_role' and i.object_id = r.id
     order by r.id";
pub const EXPORT_WORKSPACES: &str = "select w.id, coalesce(i.identifier, w.id), w.name
     from workspaces w
     left join structure_identifiers i on i.kind = 'workspace' and i.object_id = w.id
     where w.deleted_at is null
     order by w.id";
pub const EXPORT_ROLES: &str =
    "select r.workspace_id, coalesce(i.identifier, r.id), r.name, r.permissions, r.is_everyone, r.position
     from workspace_roles r
     left join structure_identifiers i on i.kind = 'workspace_role' and i.object_id = r.id
     order by r.is_everyone desc, r.id";
pub const EXPORT_CATEGORIES: &str =
    "select c.workspace_id, coalesce(i.identifier, c.id), c.name, c.position
     from categories c
     left join structure_identifiers i on i.kind = 'category' and i.object_id = c.id
     order by c.position, c.id";
pub const EXPORT_CHANNELS: &str =
    "select c.id, c.workspace_id, coalesce(i.identifier, c.id), c.name, c.kind,
            coalesce(ci.identifier, c.category_id), c.position, c.permanent_messages,
            c.user_limit, c.hides_occupants
     from channels c
     left join structure_identifiers i on i.kind = 'channel' and i.object_id = c.id
     left join structure_identifiers ci on ci.kind = 'category' and ci.object_id = c.category_id
     where c.deleted_at is null
     order by c.workspace_id, c.position, c.id";
pub const EXPORT_OVERRIDES: &str =
    "select o.channel_id, coalesce(i.identifier, o.role_id), o.allow, o.deny
     from channel_overrides o
     left join structure_identifiers i on i.kind = 'workspace_role' and i.object_id = o.role_id
     order by o.channel_id, o.role_id";

type ChannelRow = (
    Uuid,
    WorkspaceId,
    Uuid,
    String,
    String,
    Option<Uuid>,
    i32,
    bool,
    Option<i32>,
    bool,
);

fn entry_of<'a>(
    entries: &'a mut [WorkspaceEntry],
    place: &HashMap<WorkspaceId, usize>,
    workspace_id: WorkspaceId,
) -> ApiResult<&'a mut WorkspaceEntry> {
    place
        .get(&workspace_id)
        .and_then(|index| entries.get_mut(*index))
        .ok_or_else(|| ApiError::internal(&"a structure row names an unknown workspace"))
}

/// The whole structure of the server and its revision, from the snapshot of the
/// transaction. Only a caller with the administrator permission reads it, because it holds
/// every channel and every override.
pub async fn export(connection: &mut PgConnection, access: &Access) -> ApiResult<StructureExport> {
    access.require(Request::Server(ServerPermission::Administrator))?;
    let structure = read(connection).await?;
    Ok(StructureExport {
        revision: revision_of(&structure)?,
        structure,
    })
}

/// The whole structure of the server as the transaction sees it.
async fn read(connection: &mut PgConnection) -> ApiResult<StructureFile> {
    let default_workspace = store::default_workspace(&mut *connection).await?;

    let server_roles: Vec<(Uuid, String, i64, i32)> = sqlx::query_as(EXPORT_SERVER_ROLES)
        .fetch_all(&mut *connection)
        .await?;
    let workspaces: Vec<(WorkspaceId, Uuid, String)> = sqlx::query_as(EXPORT_WORKSPACES)
        .fetch_all(&mut *connection)
        .await?;
    let roles: Vec<(WorkspaceId, Uuid, String, i64, bool, i32)> = sqlx::query_as(EXPORT_ROLES)
        .fetch_all(&mut *connection)
        .await?;
    let categories: Vec<(WorkspaceId, Uuid, String, i32)> = sqlx::query_as(EXPORT_CATEGORIES)
        .fetch_all(&mut *connection)
        .await?;
    let channels: Vec<ChannelRow> = sqlx::query_as(EXPORT_CHANNELS)
        .fetch_all(&mut *connection)
        .await?;
    let overrides: Vec<(Uuid, Uuid, i64, i64)> = sqlx::query_as(EXPORT_OVERRIDES)
        .fetch_all(&mut *connection)
        .await?;

    let mut overrides_of: HashMap<Uuid, Vec<OverrideEntry>> = HashMap::new();
    for (channel_id, role, allow, deny) in overrides {
        overrides_of
            .entry(channel_id)
            .or_default()
            .push(OverrideEntry {
                role: StructureId::from_uuid(role),
                allow: db::workspace_list(db::workspace_permissions(allow)?),
                deny: db::workspace_list(db::workspace_permissions(deny)?),
            });
    }
    let mut entries: Vec<WorkspaceEntry> = Vec::with_capacity(workspaces.len());
    let mut place: HashMap<WorkspaceId, usize> = HashMap::new();
    for (id, entry, name) in workspaces {
        place.insert(id, entries.len());
        entries.push(WorkspaceEntry {
            id: StructureId::from_uuid(entry),
            name,
            default: id == default_workspace,
            roles: Vec::new(),
            categories: Vec::new(),
            channels: Vec::new(),
        });
    }
    for (workspace_id, entry, name, permissions, everyone, position) in roles {
        entry_of(&mut entries, &place, workspace_id)?
            .roles
            .push(RoleEntry {
                position: Some(position),
                id: StructureId::from_uuid(entry),
                name,
                everyone,
                permissions: Some(db::workspace_list(db::workspace_permissions(permissions)?)),
            });
    }
    for (workspace_id, entry, name, position) in categories {
        entry_of(&mut entries, &place, workspace_id)?
            .categories
            .push(CategoryEntry {
                id: StructureId::from_uuid(entry),
                name,
                position: Some(position),
            });
    }
    for row in channels {
        let (id, workspace_id, entry, name, kind, category, position, permanent, limit, hides) =
            row;
        let kind = ChannelKind::read(&kind)?;
        let voice = kind == ChannelKind::Voice;
        entry_of(&mut entries, &place, workspace_id)?
            .channels
            .push(ChannelEntry {
                id: StructureId::from_uuid(entry),
                name,
                kind,
                category: Some(category.map(StructureId::from_uuid)),
                position: Some(position),
                permanent_messages: Some(permanent),
                user_limit: voice
                    .then(|| limit.map(db::count).transpose())
                    .transpose()?,
                hides_occupants: voice.then_some(hides),
                overrides: overrides_of.remove(&id).unwrap_or_default(),
            });
    }
    Ok(StructureFile {
        format: STRUCTURE_FORMAT,
        server_roles: server_roles
            .into_iter()
            .map(|(entry, name, permissions, position)| {
                Ok(ServerRoleEntry {
                    position: Some(position),
                    id: StructureId::from_uuid(entry),
                    name,
                    permissions: Some(db::server_list(db::server_permissions(permissions)?)),
                })
            })
            .collect::<ApiResult<_>>()?,
        workspaces: entries,
        delete: Deletions::default(),
    })
}

/// Applies the changes of one request in the transaction of the caller: all of them or, with
/// an error, none. Each change runs the function of its own route, with the same checks,
/// audit entries and events. The caller needs the administrator permission, because the
/// request names the revision of the whole structure, which only an administrator reads.
///
/// The transaction reads the whole structure before it changes it, and it is serializable
/// (ADR 0009). A change of the structure that commits while it runs either makes one of the
/// two transactions run again, or commits beside it with the result of this request first
/// and that change after it. So the changes are applied to the structure of the named
/// revision or not at all.
pub async fn apply(
    app: &AppState,
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    request: &ApplyStructureChanges,
) -> ApiResult<StructureApplied> {
    access.require(Request::Server(ServerPermission::Administrator))?;
    let current = revision_of(&read(&mut *connection).await?)?;
    if current != request.revision {
        return Err(ApiError::conflict(format!(
            "the structure has the revision {current} and the request names another; nothing \
             was changed. Make a new plan"
        )));
    }
    let mut created = Vec::new();
    for (index, change) in request.changes.iter().enumerate() {
        apply_one(app, &mut *connection, access, now, change, &mut created)
            .await
            .map_err(|error| error.in_change(index + 1, change.name()))?;
    }
    let revision = if request.changes.is_empty() {
        current
    } else {
        revision_of(&read(connection).await?)?
    };
    Ok(StructureApplied { revision, created })
}

async fn workspace_of(
    connection: &mut PgConnection,
    statement: &'static str,
    object_id: Uuid,
) -> ApiResult<WorkspaceId> {
    sqlx::query_scalar(statement)
        .bind(object_id)
        .fetch_optional(connection)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn apply_one(
    app: &AppState,
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    change: &StructureChange,
    created: &mut Vec<CreatedEntry>,
) -> ApiResult<()> {
    use StructureKind::{Category, Channel, ServerRole as ServerRoleKind, Workspace};
    let role_kind = StructureKind::WorkspaceRole;
    match change {
        StructureChange::ReorderServerRoles { positions } => {
            if positions.len() > 250 {
                return Err(ApiError::invalid("at most 250 role positions may change"));
            }
            let mut resolved = Vec::new();
            for p in positions {
                resolved.push((resolve(connection, ServerRoleKind, p.id).await?, p.position));
            }
            crate::role_order::reorder(connection, access, now, None, &resolved).await
        }
        StructureChange::ReorderWorkspaceRoles {
            workspace,
            positions,
        } => {
            if positions.len() > 250 {
                return Err(ApiError::invalid("at most 250 role positions may change"));
            }
            let workspace =
                WorkspaceId::from_uuid(resolve(connection, Workspace, *workspace).await?);
            let mut resolved = Vec::new();
            for p in positions {
                resolved.push((resolve(connection, role_kind, p.id).await?, p.position));
            }
            crate::role_order::reorder(connection, access, now, Some(workspace), &resolved).await
        }
        StructureChange::CreateServerRole {
            id,
            name,
            permissions,
        } => {
            require_unused(&mut *connection, ServerRoleKind, *id).await?;
            let held: ServerPermissions = permissions.iter().copied().collect();
            let role = ServerRole {
                position: 1,
                id: ServerRoleId::generate(),
                name: validate::name("name", name)?,
                permissions: db::server_list(held),
            };
            server_roles::create(
                &mut *connection,
                access,
                now,
                &role,
                app.config.limits.most_server_roles,
            )
            .await?;
            register(connection, created, ServerRoleKind, *id, role.id.as_uuid()).await
        }
        StructureChange::UpdateServerRole {
            id,
            name,
            permissions,
        } => {
            let role_id = resolve(&mut *connection, ServerRoleKind, *id).await?;
            let name = optional_name(name.as_deref())?;
            let held = permissions
                .as_ref()
                .map(|permissions| permissions.iter().copied().collect());
            let role_id = ServerRoleId::from_uuid(role_id);
            server_roles::update(connection, access, now, role_id, name, held).await?;
            Ok(())
        }
        StructureChange::CreateWorkspace {
            id,
            name,
            everyone_role,
        } => {
            require_unused(&mut *connection, Workspace, *id).await?;
            require_unused(&mut *connection, role_kind, *everyone_role).await?;
            let name = validate::name("name", name)?;
            let (workspace, everyone) =
                workspaces::create(&mut *connection, access, now, &name).await?;
            let workspace_id = workspace.id.as_uuid();
            register(&mut *connection, created, Workspace, *id, workspace_id).await?;
            let everyone_id = everyone.id.as_uuid();
            register(connection, created, role_kind, *everyone_role, everyone_id).await
        }
        StructureChange::RenameWorkspace { id, name } => {
            let workspace_id = resolve(&mut *connection, Workspace, *id).await?;
            let name = validate::name("name", name)?;
            let workspace_id = WorkspaceId::from_uuid(workspace_id);
            workspaces::rename(connection, access, now, workspace_id, &name).await?;
            Ok(())
        }
        StructureChange::CreateWorkspaceRole {
            id,
            workspace,
            name,
            permissions,
        } => {
            require_unused(&mut *connection, role_kind, *id).await?;
            let workspace_id = resolve(&mut *connection, Workspace, *workspace).await?;
            let held: WorkspacePermissions = permissions.iter().copied().collect();
            let role = WorkspaceRole {
                position: 1,
                id: WorkspaceRoleId::generate(),
                workspace_id: WorkspaceId::from_uuid(workspace_id),
                name: validate::name("name", name)?,
                permissions: db::workspace_list(held),
                is_everyone: false,
            };
            let ceiling = app.config.limits.most_roles_per_workspace;
            workspace_roles::create(&mut *connection, access, now, &role, ceiling).await?;
            register(connection, created, role_kind, *id, role.id.as_uuid()).await
        }
        StructureChange::UpdateWorkspaceRole {
            id,
            name,
            permissions,
        } => {
            let role_id = resolve(&mut *connection, role_kind, *id).await?;
            let workspace_id = workspace_of(
                &mut *connection,
                "select workspace_id from workspace_roles where id = $1",
                role_id,
            )
            .await?;
            let name = optional_name(name.as_deref())?;
            let held = permissions
                .as_ref()
                .map(|permissions| permissions.iter().copied().collect());
            let ids = (workspace_id, WorkspaceRoleId::from_uuid(role_id));
            workspace_roles::update(connection, access, now, ids, name, held).await?;
            Ok(())
        }
        StructureChange::CreateCategory {
            id,
            workspace,
            name,
            position,
        } => {
            require_unused(&mut *connection, Category, *id).await?;
            let workspace_id = resolve(&mut *connection, Workspace, *workspace).await?;
            let name = validate::name("name", name)?;
            let ids = (WorkspaceId::from_uuid(workspace_id), CategoryId::generate());
            let category =
                channels::create_category_in(&mut *connection, access, now, ids, &name, *position)
                    .await?;
            register(connection, created, Category, *id, category.id.as_uuid()).await
        }
        StructureChange::UpdateCategory { id, name, position } => {
            let category_id = resolve(&mut *connection, Category, *id).await?;
            let workspace_id = workspace_of(
                &mut *connection,
                "select workspace_id from categories where id = $1",
                category_id,
            )
            .await?;
            let name = optional_name(name.as_deref())?;
            let ids = (workspace_id, CategoryId::from_uuid(category_id));
            let name = name.as_deref();
            channels::update_category_in(connection, access, now, ids, name, *position).await?;
            Ok(())
        }
        StructureChange::CreateChannel {
            id,
            workspace,
            kind,
            name,
            category,
            position,
        } => {
            require_unused(&mut *connection, Channel, *id).await?;
            let workspace_id = resolve(&mut *connection, Workspace, *workspace).await?;
            let category_id = match category {
                Some(category) => Some(CategoryId::from_uuid(
                    resolve(&mut *connection, Category, *category).await?,
                )),
                None => None,
            };
            let name = validate::name("name", name)?;
            let new = NewChannel {
                id: ChannelId::generate(),
                workspace_id: WorkspaceId::from_uuid(workspace_id),
                name: &name,
                kind: *kind,
                category_id,
                position: *position,
            };
            channels::create_channel_in(&mut *connection, access, now, &new).await?;
            register(connection, created, Channel, *id, new.id.as_uuid()).await
        }
        StructureChange::UpdateChannel {
            id,
            name,
            category,
            position,
            permanent_messages,
        } => {
            let channel_id = resolve(&mut *connection, Channel, *id).await?;
            let category_id = match category {
                Some(Some(category)) => Some(Some(CategoryId::from_uuid(
                    resolve(&mut *connection, Category, *category).await?,
                ))),
                Some(None) => Some(None),
                None => None,
            };
            let name = optional_name(name.as_deref())?;
            let update = UpdateChannel {
                name: None,
                category_id,
                position: *position,
                permanent_messages: *permanent_messages,
            };
            let channel_id = ChannelId::from_uuid(channel_id);
            let name = name.as_deref();
            channels::update_channel_in(connection, access, now, channel_id, name, &update).await?;
            Ok(())
        }
        StructureChange::UpdateCallSettings {
            id,
            user_limit,
            hides_occupants,
        } => {
            let channel_id = resolve(&mut *connection, Channel, *id).await?;
            let update = UpdateCallSettings {
                user_limit: *user_limit,
                hides_occupants: *hides_occupants,
            };
            calls::check_call_settings(app, &update)?;
            let channel_id = ChannelId::from_uuid(channel_id);
            calls::update_call_settings_in(connection, access, now, channel_id, &update).await?;
            Ok(())
        }
        StructureChange::SetChannelOverride {
            channel,
            role,
            allow,
            deny,
        } => {
            let channel_id = resolve(&mut *connection, Channel, *channel).await?;
            let role_id = resolve(&mut *connection, role_kind, *role).await?;
            let change = crate::routes::rules::Change::set(&SetChannelOverride {
                allow: allow.clone(),
                deny: deny.clone(),
            })?;
            let subject = RuleSubject::Role {
                role_id: WorkspaceRoleId::from_uuid(role_id),
            };
            let ceiling = app.config.limits.most_person_rules_per_place;
            crate::routes::rules::set_channel_rule_in(
                connection,
                (&app.calls, ceiling),
                access,
                now,
                (ChannelId::from_uuid(channel_id), subject),
                change,
            )
            .await?;
            Ok(())
        }
        StructureChange::DeleteChannel { id } => {
            let channel_id = ChannelId::from_uuid(resolve(&mut *connection, Channel, *id).await?);
            channels::delete_channel_in(connection, &app.calls, access, now, channel_id).await
        }
        StructureChange::DeleteCategory { id } => {
            let category_id = resolve(&mut *connection, Category, *id).await?;
            let workspace_id = workspace_of(
                &mut *connection,
                "select workspace_id from categories where id = $1",
                category_id,
            )
            .await?;
            let ids = (workspace_id, CategoryId::from_uuid(category_id));
            channels::delete_category_in(connection, access, now, ids).await
        }
        StructureChange::DeleteWorkspaceRole { id } => {
            let role_id = resolve(&mut *connection, role_kind, *id).await?;
            let workspace_id = workspace_of(
                &mut *connection,
                "select workspace_id from workspace_roles where id = $1",
                role_id,
            )
            .await?;
            let ids = (workspace_id, WorkspaceRoleId::from_uuid(role_id));
            workspace_roles::delete(connection, access, now, ids).await
        }
        StructureChange::DeleteServerRole { id } => {
            let role_id = resolve(&mut *connection, ServerRoleKind, *id).await?;
            let role_id = ServerRoleId::from_uuid(role_id);
            server_roles::delete(connection, access, now, role_id).await
        }
        StructureChange::DeleteWorkspace { id } => {
            let workspace_id = resolve(&mut *connection, Workspace, *id).await?;
            let workspace_id = WorkspaceId::from_uuid(workspace_id);
            workspaces::delete(connection, access, now, workspace_id).await
        }
    }
}

fn optional_name(name: Option<&str>) -> ApiResult<Option<String>> {
    name.map(|name| validate::name("name", name)).transpose()
}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(StructureFile)))]
struct FileTypes;

/// Rebuilds every object with its keys in order, so that the document does not depend on
/// the order in which a map was filled.
fn sorted(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let ordered: std::collections::BTreeMap<String, serde_json::Value> = map
                .into_iter()
                .map(|(key, value)| (key, sorted(value)))
                .collect();
            serde_json::Value::Object(ordered.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sorted).collect())
        }
        other => other,
    }
}

/// The JSON Schema of the structure file (ADR 0204): the committed file
/// `docs/api/structure-file.json`.
pub fn file_schema() -> Result<serde_json::Value, serde_json::Error> {
    use utoipa::OpenApi;
    let components = serde_json::to_value(FileTypes::openapi().components)?;
    Ok(sorted(serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Junto structure file",
        "description": format!(
            "The structure of a Junto server as the setup tool writes it: format \
             {STRUCTURE_FORMAT}. The tool reads the formats {OLDEST_STRUCTURE_FORMAT} to \
             {STRUCTURE_FORMAT}. An entry is matched by its identifier; an entry that is absent \
             is kept; a deletion is listed under `delete`."
        ),
        "$ref": "#/components/schemas/StructureFile",
        "components": components,
    })))
}
