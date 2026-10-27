//! Computes the changes that make a server match a structure file (ADR 0204, ADR 0212).
//!
//! The function reads two documents and contacts nothing: the file, and the structure that
//! the server exported. An entry is matched by its identifier. An entry that the file does
//! not hold is kept, and an optional field that an entry does not hold keeps its value.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Display;

use junto_protocol::WorkspacePermission;
use junto_protocol::ids::StructureId;
use junto_protocol::structure::ChannelKind;
use junto_protocol::structure_file::{
    CategoryEntry, ChannelEntry, OLDEST_STRUCTURE_FORMAT, OverrideEntry, RoleEntry,
    STRUCTURE_FORMAT, StructureChange, StructureFile, WorkspaceEntry,
};

const NAME_MAX_CHARS: usize = 64;

/// One change with the line that tells a person what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub change: StructureChange,
    pub line: String,
}

/// Why a file cannot be applied. Every problem of a file is stated, not only the first.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .0.join("\n"))]
pub struct Problems(pub Vec<String>);

struct Planner<'a> {
    current: &'a StructureFile,
    changes: Vec<Planned>,
    problems: Vec<String>,
    fresh: &'a mut dyn FnMut() -> StructureId,
}

impl Planner<'_> {
    fn add(&mut self, change: StructureChange, line: String) {
        self.changes.push(Planned { change, line });
    }

    fn problem(&mut self, problem: String) {
        self.problems.push(problem);
    }

    fn check_name(&mut self, what: &str, id: StructureId, name: &str) {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            self.problem(format!("the {what} {id} has an empty name"));
        } else if trimmed.chars().count() > NAME_MAX_CHARS {
            self.problem(format!(
                "the name of the {what} {id} is longer than {NAME_MAX_CHARS} characters"
            ));
        } else if trimmed.chars().any(char::is_control) {
            self.problem(format!(
                "the name of the {what} {id} contains a control character"
            ));
        }
    }
}

fn set<T: Ord + Copy>(values: &[T]) -> BTreeSet<T> {
    values.iter().copied().collect()
}

fn listed<T: Display>(values: &[T]) -> String {
    if values.is_empty() {
        return "none".to_owned();
    }
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn names<T: serde::Serialize>(values: &[T]) -> String {
    let names: Vec<String> = values
        .iter()
        .filter_map(|value| serde_json::to_value(value).ok())
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    listed(&names)
}

/// The differences of one existing entry, as the parts of one line.
#[derive(Default)]
struct Differences(Vec<String>);

impl Differences {
    fn name(&mut self, from: &str, to: &str) -> Option<String> {
        let to = to.trim();
        (from != to).then(|| {
            self.0.push(format!("name {from:?} -> {to:?}"));
            to.to_owned()
        })
    }

    fn value<T: PartialEq + Copy>(
        &mut self,
        field: &str,
        from: Option<T>,
        to: Option<T>,
        show: impl Fn(T) -> String,
    ) -> Option<T> {
        let to = to?;
        (from != Some(to)).then(|| {
            let before = from.map_or_else(|| "unknown".to_owned(), &show);
            self.0.push(format!("{field} {before} -> {}", show(to)));
            to
        })
    }

    fn permissions<T: Ord + Copy + serde::Serialize>(
        &mut self,
        from: Option<&Vec<T>>,
        to: Option<&Vec<T>>,
    ) -> Option<Vec<T>> {
        let to = to?;
        let before = from.map(|from| set(from)).unwrap_or_default();
        let after = set(to);
        (before != after).then(|| {
            let added: Vec<T> = after.difference(&before).copied().collect();
            let removed: Vec<T> = before.difference(&after).copied().collect();
            self.0.push(format!(
                "permissions: add {}, remove {}",
                names(&added),
                names(&removed)
            ));
            after.into_iter().collect()
        })
    }

    fn line(&self) -> String {
        self.0.join("; ")
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn limit(value: Option<u32>) -> String {
    value.map_or_else(|| "none".to_owned(), |limit| limit.to_string())
}

fn reference(value: Option<StructureId>) -> String {
    value.map_or_else(|| "none".to_owned(), |id| id.to_string())
}

/// Refuses identifiers that two entries of one kind share, and marks that two entries hold.
fn check_file(planner: &mut Planner<'_>, file: &StructureFile) {
    if !(OLDEST_STRUCTURE_FORMAT..=STRUCTURE_FORMAT).contains(&file.format) {
        planner.problem(format!(
            "the file has the format {}; this tool reads the formats \
             {OLDEST_STRUCTURE_FORMAT} to {STRUCTURE_FORMAT}",
            file.format
        ));
    }
    for positions in std::iter::once(
        file.server_roles
            .iter()
            .map(|r| (r.position, false))
            .collect::<Vec<_>>(),
    )
    .chain(
        file.workspaces
            .iter()
            .map(|w| w.roles.iter().map(|r| (r.position, r.everyone)).collect()),
    ) {
        let mut seen_positions = HashSet::new();
        for (position, everyone) in positions {
            if let Some(position) = position
                && ((everyone && position != 0)
                    || (!everyone && position <= 0)
                    || !seen_positions.insert(position))
            {
                planner.problem(
                    "role positions must be distinct and positive; everyone must be zero"
                        .to_owned(),
                );
            }
        }
    }
    let mut seen: HashMap<&'static str, HashSet<StructureId>> = HashMap::new();
    let mut unique = |planner: &mut Planner<'_>, kind: &'static str, id: StructureId| {
        if !seen.entry(kind).or_default().insert(id) {
            planner.problem(format!("two {kind} entries have the identifier {id}"));
        }
    };
    let delete = &file.delete;
    let kept = |planner: &mut Planner<'_>, what: &str, id: StructureId, list: &[StructureId]| {
        if list.contains(&id) {
            planner.problem(format!(
                "the {what} {id} is an entry and is also listed for deletion"
            ));
        }
    };
    for role in &file.server_roles {
        unique(planner, "server role", role.id);
        planner.check_name("server role", role.id, &role.name);
        kept(planner, "server role", role.id, &delete.server_roles);
    }
    if file.workspaces.iter().filter(|entry| entry.default).count() > 1 {
        planner.problem("two workspaces are marked as the default workspace".to_owned());
    }
    for workspace in &file.workspaces {
        unique(planner, "workspace", workspace.id);
        planner.check_name("workspace", workspace.id, &workspace.name);
        kept(planner, "workspace", workspace.id, &delete.workspaces);
        if workspace.roles.iter().filter(|role| role.everyone).count() > 1 {
            planner.problem(format!(
                "the workspace {} has two roles marked as its everyone role",
                workspace.id
            ));
        }
        for role in &workspace.roles {
            unique(planner, "role", role.id);
            planner.check_name("role", role.id, &role.name);
            kept(planner, "role", role.id, &delete.roles);
        }
        for category in &workspace.categories {
            unique(planner, "category", category.id);
            planner.check_name("category", category.id, &category.name);
            kept(planner, "category", category.id, &delete.categories);
            check_overrides(
                planner,
                ("category", category.id),
                &category.overrides,
                &delete.roles,
            );
        }
        for channel in &workspace.channels {
            unique(planner, "channel", channel.id);
            planner.check_name("channel", channel.id, &channel.name);
            kept(planner, "channel", channel.id, &delete.channels);
            if let Some(Some(category)) = channel.category
                && delete.categories.contains(&category)
            {
                planner.problem(format!(
                    "the channel {} names the category {category}, which is listed for \
                     deletion",
                    channel.id
                ));
            }
            let voice_only = channel.user_limit.is_some() || channel.hides_occupants.is_some();
            if channel.kind == ChannelKind::Text && voice_only {
                planner.problem(format!(
                    "the text channel {} has a setting of voice channels (user_limit, \
                     hides_occupants)",
                    channel.id
                ));
            }
            if channel.synced == Some(true) {
                if !channel.overrides.is_empty() {
                    planner.problem(format!(
                        "the channel {} is synced with its category and states overrides; a \
                         synced channel takes the overrides of its category",
                        channel.id
                    ));
                }
                if channel.category == Some(None) {
                    planner.problem(format!(
                        "the channel {} is synced with its category and has no category",
                        channel.id
                    ));
                }
            }
            check_overrides(
                planner,
                ("channel", channel.id),
                &channel.overrides,
                &delete.roles,
            );
        }
    }
    for (what, list) in [
        ("channel", &delete.channels),
        ("category", &delete.categories),
        ("role", &delete.roles),
        ("server role", &delete.server_roles),
        ("workspace", &delete.workspaces),
    ] {
        let mut deleted = HashSet::new();
        for id in list {
            if !deleted.insert(*id) {
                planner.problem(format!("the {what} {id} is listed for deletion twice"));
            }
        }
    }
}

/// Refuses two overrides of one role at a place, an override of a role that the file deletes,
/// and a permission in both sets.
fn check_overrides(
    planner: &mut Planner<'_>,
    (what, id): (&str, StructureId),
    overrides: &[OverrideEntry],
    deleted_roles: &[StructureId],
) {
    let mut roles = HashSet::new();
    for entry in overrides {
        if !roles.insert(entry.role) {
            planner.problem(format!(
                "the {what} {id} has two overrides of the role {}",
                entry.role
            ));
        }
        if deleted_roles.contains(&entry.role) {
            planner.problem(format!(
                "the {what} {id} has an override of the role {}, which is listed for deletion",
                entry.role
            ));
        }
        if !set(&entry.allow).is_disjoint(&set(&entry.deny)) {
            planner.problem(format!(
                "the override of the role {} in the {what} {id} has a permission in both allow \
                 and deny",
                entry.role
            ));
        }
    }
}

/// The deletions that the file lists, after every other change: channels, categories,
/// workspace roles, workspaces, server roles. A listed entry that the server does not have
/// is no change. The server refuses the deletion of an everyone role and of the default
/// workspace, so the file is refused before.
fn deletions(planner: &mut Planner<'_>, places: &Places<'_>, file: &StructureFile) {
    for id in &file.delete.channels {
        if let Some((_, channel)) = places.channels.get(id) {
            planner.add(
                StructureChange::DeleteChannel { id: *id },
                format!("- channel {:?} ({id}): delete", channel.name),
            );
        }
    }
    for id in &file.delete.categories {
        let Some(workspace) = places.categories.get(id) else {
            continue;
        };
        let name = places
            .workspaces
            .get(workspace)
            .and_then(|found| found.categories.iter().find(|category| category.id == *id))
            .map_or("", |category| category.name.as_str());
        planner.add(
            StructureChange::DeleteCategory { id: *id },
            format!("- category {name:?} ({id}): delete; its channels stay without a category"),
        );
    }
    for id in &file.delete.roles {
        let Some(workspace) = places.roles.get(id) else {
            continue;
        };
        let role = places
            .workspaces
            .get(workspace)
            .and_then(|found| found.roles.iter().find(|role| role.id == *id));
        if role.is_some_and(|role| role.everyone) {
            planner.problem(format!(
                "the role {id} is the everyone role of the workspace {workspace}; it is not \
                 deleted"
            ));
            continue;
        }
        let name = role.map_or("", |role| role.name.as_str());
        planner.add(
            StructureChange::DeleteWorkspaceRole { id: *id },
            format!("- role {name:?} ({id}): delete"),
        );
    }
    for id in &file.delete.workspaces {
        let Some(found) = places.workspaces.get(id) else {
            continue;
        };
        if found.default {
            planner.problem(format!(
                "the workspace {id} is the default workspace of the server; it is not deleted"
            ));
            continue;
        }
        planner.add(
            StructureChange::DeleteWorkspace { id: *id },
            format!(
                "- workspace {:?} ({id}): delete with its {} roles, {} categories and {} \
                 channels, and with their content",
                found.name,
                found.roles.len(),
                found.categories.len(),
                found.channels.len()
            ),
        );
    }
    for id in &file.delete.server_roles {
        let role = planner
            .current
            .server_roles
            .iter()
            .find(|role| role.id == *id);
        if let Some(role) = role {
            let line = format!("- server role {:?} ({id}): delete", role.name);
            planner.add(StructureChange::DeleteServerRole { id: *id }, line);
        }
    }
}

/// Where the entries of the server are: the workspace of each role, category and channel.
struct Places<'a> {
    workspaces: HashMap<StructureId, &'a WorkspaceEntry>,
    roles: HashMap<StructureId, StructureId>,
    categories: HashMap<StructureId, StructureId>,
    channels: HashMap<StructureId, (StructureId, &'a ChannelEntry)>,
}

impl<'a> Places<'a> {
    fn of(current: &'a StructureFile) -> Self {
        let mut places = Self {
            workspaces: HashMap::new(),
            roles: HashMap::new(),
            categories: HashMap::new(),
            channels: HashMap::new(),
        };
        for workspace in &current.workspaces {
            places.workspaces.insert(workspace.id, workspace);
            for role in &workspace.roles {
                places.roles.insert(role.id, workspace.id);
            }
            for category in &workspace.categories {
                places.categories.insert(category.id, workspace.id);
            }
            for channel in &workspace.channels {
                places.channels.insert(channel.id, (workspace.id, channel));
            }
        }
        places
    }
}

/// Computes the changes that make the server, whose structure is `current`, match `file`.
/// `fresh` gives an identifier for the everyone role of a new workspace whose file entry
/// names none.
pub fn plan(
    file: &StructureFile,
    current: &StructureFile,
    fresh: &mut dyn FnMut() -> StructureId,
) -> Result<Vec<Planned>, Problems> {
    let mut planner = Planner {
        current,
        changes: Vec::new(),
        problems: Vec::new(),
        fresh,
    };
    check_file(&mut planner, file);
    if !planner.problems.is_empty() {
        return Err(Problems(planner.problems));
    }
    let places = Places::of(current);
    server_roles(&mut planner, file);
    let claimed_default = file.workspaces.iter().any(|entry| {
        places
            .workspaces
            .get(&entry.id)
            .is_some_and(|found| found.default)
    });
    for entry in &file.workspaces {
        workspace(&mut planner, &places, entry, claimed_default);
    }
    deletions(&mut planner, &places, file);
    role_positions(&mut planner, file);
    if planner.problems.is_empty() {
        Ok(planner.changes)
    } else {
        Err(Problems(planner.problems))
    }
}

fn server_roles(planner: &mut Planner<'_>, file: &StructureFile) {
    for entry in &file.server_roles {
        let id = entry.id;
        let existing = planner
            .current
            .server_roles
            .iter()
            .find(|role| role.id == id);
        let Some(existing) = existing else {
            let permissions = entry.permissions.clone().unwrap_or_default();
            let line = format!(
                "+ server role {:?} ({id}): create with permissions {}",
                entry.name.trim(),
                names(&permissions)
            );
            planner.add(
                StructureChange::CreateServerRole {
                    id,
                    name: entry.name.trim().to_owned(),
                    permissions,
                },
                line,
            );
            continue;
        };
        let mut differences = Differences::default();
        let name = differences.name(&existing.name, &entry.name);
        let permissions =
            differences.permissions(existing.permissions.as_ref(), entry.permissions.as_ref());
        if !differences.is_empty() {
            let line = format!(
                "~ server role {:?} ({id}): {}",
                existing.name,
                differences.line()
            );
            planner.add(
                StructureChange::UpdateServerRole {
                    id,
                    name,
                    permissions,
                },
                line,
            );
        }
    }
}

fn workspace(
    planner: &mut Planner<'_>,
    places: &Places<'_>,
    entry: &WorkspaceEntry,
    claimed_default: bool,
) {
    let by_identifier = places.workspaces.get(&entry.id).copied();
    let existing = match by_identifier {
        Some(found) => {
            if entry.default && !found.default {
                planner.problem(format!(
                    "the workspace {} is marked as the default workspace, and the server has \
                     another default workspace; the default workspace of a server does not \
                     change",
                    entry.id
                ));
                return;
            }
            Some(found)
        }
        // An entry with the mark and an unknown identifier stands for the default workspace
        // of this server: a file of another server is applied to this one.
        None if entry.default => {
            if claimed_default {
                planner.problem(format!(
                    "the workspace {} is marked as the default workspace, and another entry \
                     has the identifier of the default workspace of the server",
                    entry.id
                ));
                return;
            }
            planner
                .current
                .workspaces
                .iter()
                .find(|found| found.default)
        }
        None => None,
    };
    match existing {
        Some(found) => existing_workspace(planner, places, entry, found),
        None => new_workspace(planner, places, entry),
    }
}

fn existing_workspace(
    planner: &mut Planner<'_>,
    places: &Places<'_>,
    entry: &WorkspaceEntry,
    found: &WorkspaceEntry,
) {
    let id = found.id;
    let mut differences = Differences::default();
    if let Some(name) = differences.name(&found.name, &entry.name) {
        let line = format!(
            "~ workspace {:?} ({id}): {}",
            found.name,
            differences.line()
        );
        planner.add(StructureChange::RenameWorkspace { id, name }, line);
    }
    // The identifier that the changes use for each role of the file.
    let mut role_names: HashMap<StructureId, StructureId> = HashMap::new();
    // The entry of the file that stands for each role of the server.
    let mut standing: HashMap<StructureId, StructureId> = HashMap::new();
    for role in &entry.roles {
        let by_identifier = found.roles.iter().find(|existing| existing.id == role.id);
        if by_identifier.is_none()
            && foreign(planner, "role", role.id, places.roles.get(&role.id), id)
        {
            continue;
        }
        let existing = if role.everyone {
            if by_identifier.is_some_and(|existing| !existing.everyone) {
                planner.problem(format!(
                    "the role {} is marked as the everyone role, and the server has it as \
                     another role of the workspace",
                    role.id
                ));
                continue;
            }
            found.roles.iter().find(|existing| existing.everyone)
        } else {
            by_identifier
        };
        match existing {
            Some(existing) => {
                if let Some(other) = standing.insert(existing.id, role.id) {
                    planner.problem(format!(
                        "the role entries {other} and {} stand for the same role {} of the \
                         server",
                        role.id, existing.id
                    ));
                    continue;
                }
                role_names.insert(role.id, existing.id);
                update_role(planner, role, existing);
            }
            None => {
                role_names.insert(role.id, role.id);
                create_role(planner, id, role);
            }
        }
    }
    for existing in &found.roles {
        role_names.entry(existing.id).or_insert(existing.id);
    }
    let mut categories: HashSet<StructureId> = found
        .categories
        .iter()
        .map(|category| category.id)
        .collect();
    let mut rules: HashMap<StructureId, Rules> = found
        .categories
        .iter()
        .map(|category| (category.id, stored(&category.overrides)))
        .collect();
    for category in &entry.categories {
        let existing = found
            .categories
            .iter()
            .find(|existing| existing.id == category.id);
        match existing {
            Some(existing) => update_category(planner, category, existing),
            None => {
                let elsewhere = places.categories.get(&category.id);
                if foreign(planner, "category", category.id, elsewhere, id) {
                    continue;
                }
                create_category(planner, id, category);
            }
        }
        categories.insert(category.id);
        category_overrides(planner, category, &mut rules, &role_names);
    }
    for channel in &entry.channels {
        if !references_exist(planner, channel, &categories, &role_names) {
            continue;
        }
        let existing = found
            .channels
            .iter()
            .find(|existing| existing.id == channel.id);
        match existing {
            Some(existing) => {
                if !update_channel(planner, channel, existing) {
                    continue;
                }
            }
            None => {
                let elsewhere = places
                    .channels
                    .get(&channel.id)
                    .map(|(workspace, _)| workspace);
                if foreign(planner, "channel", channel.id, elsewhere, id) {
                    continue;
                }
                create_channel(planner, id, channel);
            }
        }
        channel_rules(planner, channel, existing, &rules, &role_names);
    }
}

/// States the problem of an entry whose identifier names an entry of another workspace.
fn foreign(
    planner: &mut Planner<'_>,
    what: &str,
    id: StructureId,
    elsewhere: Option<&StructureId>,
    workspace: StructureId,
) -> bool {
    match elsewhere {
        Some(other) if *other != workspace => {
            planner.problem(format!(
                "the {what} {id} belongs to the workspace {other} on the server; an entry \
                 does not move to another workspace"
            ));
            true
        }
        _ => false,
    }
}

fn new_workspace(planner: &mut Planner<'_>, places: &Places<'_>, entry: &WorkspaceEntry) {
    let id = entry.id;
    let everyone = entry.roles.iter().find(|role| role.everyone);
    let everyone_role = match everyone {
        Some(role) => role.id,
        None => (planner.fresh)(),
    };
    let mut taken = false;
    for role in &entry.roles {
        taken |= foreign(planner, "role", role.id, places.roles.get(&role.id), id);
    }
    for category in &entry.categories {
        let elsewhere = places.categories.get(&category.id);
        taken |= foreign(planner, "category", category.id, elsewhere, id);
    }
    for channel in &entry.channels {
        let elsewhere = places
            .channels
            .get(&channel.id)
            .map(|(workspace, _)| workspace);
        taken |= foreign(planner, "channel", channel.id, elsewhere, id);
    }
    if taken {
        return;
    }
    planner.add(
        StructureChange::CreateWorkspace {
            id,
            name: entry.name.trim().to_owned(),
            everyone_role,
        },
        format!("+ workspace {:?} ({id}): create", entry.name.trim()),
    );
    let mut role_names: HashMap<StructureId, StructureId> = HashMap::new();
    for role in &entry.roles {
        role_names.insert(role.id, role.id);
        if role.everyone {
            let line = format!(
                "~ role {:?} ({}): the everyone role of the new workspace, permissions {}",
                role.name.trim(),
                role.id,
                role.permissions
                    .as_ref()
                    .map_or_else(|| "as the server sets them".to_owned(), |held| names(held))
            );
            planner.add(
                StructureChange::UpdateWorkspaceRole {
                    id: role.id,
                    name: Some(role.name.trim().to_owned()),
                    permissions: role.permissions.clone(),
                },
                line,
            );
        } else {
            create_role(planner, id, role);
        }
    }
    let mut categories = HashSet::new();
    let mut rules: HashMap<StructureId, Rules> = HashMap::new();
    for category in &entry.categories {
        create_category(planner, id, category);
        categories.insert(category.id);
        category_overrides(planner, category, &mut rules, &role_names);
    }
    for channel in &entry.channels {
        if references_exist(planner, channel, &categories, &role_names) {
            create_channel(planner, id, channel);
            channel_rules(planner, channel, None, &rules, &role_names);
        }
    }
}

fn references_exist(
    planner: &mut Planner<'_>,
    channel: &ChannelEntry,
    categories: &HashSet<StructureId>,
    role_names: &HashMap<StructureId, StructureId>,
) -> bool {
    let mut exist = true;
    if let Some(Some(category)) = channel.category
        && !categories.contains(&category)
    {
        planner.problem(format!(
            "the channel {} names the category {category}, which is no category of its \
             workspace in the file or on the server",
            channel.id
        ));
        exist = false;
    }
    roles_exist(
        planner,
        ("channel", channel.id),
        &channel.overrides,
        role_names,
    ) && exist
}

/// Requires that every override names a role of the workspace, in the file or on the
/// server, and that no two overrides stand for one role of the server.
fn roles_exist(
    planner: &mut Planner<'_>,
    (what, id): (&str, StructureId),
    overrides: &[OverrideEntry],
    role_names: &HashMap<StructureId, StructureId>,
) -> bool {
    let mut exist = true;
    let mut named: HashSet<StructureId> = HashSet::new();
    for entry in overrides {
        let Some(role) = role_names.get(&entry.role) else {
            planner.problem(format!(
                "the {what} {id} has an override of the role {}, which is no role of its \
                 workspace in the file or on the server",
                entry.role
            ));
            exist = false;
            continue;
        };
        if !named.insert(*role) {
            planner.problem(format!(
                "the {what} {id} has two overrides that stand for the same role {role} of the \
                 server"
            ));
            exist = false;
        }
    }
    exist
}

fn create_role(planner: &mut Planner<'_>, workspace: StructureId, role: &RoleEntry) {
    let permissions = role.permissions.clone().unwrap_or_default();
    let line = format!(
        "+ role {:?} ({}): create with permissions {}",
        role.name.trim(),
        role.id,
        names(&permissions)
    );
    planner.add(
        StructureChange::CreateWorkspaceRole {
            id: role.id,
            workspace,
            name: role.name.trim().to_owned(),
            permissions,
        },
        line,
    );
}

fn update_role(planner: &mut Planner<'_>, role: &RoleEntry, existing: &RoleEntry) {
    let mut differences = Differences::default();
    let name = differences.name(&existing.name, &role.name);
    let permissions =
        differences.permissions(existing.permissions.as_ref(), role.permissions.as_ref());
    if !differences.is_empty() {
        let line = format!(
            "~ role {:?} ({}): {}",
            existing.name,
            existing.id,
            differences.line()
        );
        planner.add(
            StructureChange::UpdateWorkspaceRole {
                id: existing.id,
                name,
                permissions,
            },
            line,
        );
    }
}

fn create_category(planner: &mut Planner<'_>, workspace: StructureId, category: &CategoryEntry) {
    planner.add(
        StructureChange::CreateCategory {
            id: category.id,
            workspace,
            name: category.name.trim().to_owned(),
            position: category.position,
        },
        format!(
            "+ category {:?} ({}): create",
            category.name.trim(),
            category.id
        ),
    );
}

fn update_category(planner: &mut Planner<'_>, category: &CategoryEntry, existing: &CategoryEntry) {
    let mut differences = Differences::default();
    let name = differences.name(&existing.name, &category.name);
    let position = differences.value("position", existing.position, category.position, |value| {
        value.to_string()
    });
    if !differences.is_empty() {
        let line = format!(
            "~ category {:?} ({}): {}",
            existing.name,
            existing.id,
            differences.line()
        );
        planner.add(
            StructureChange::UpdateCategory {
                id: existing.id,
                name,
                position,
            },
            line,
        );
    }
}

/// The allow set and the deny set of one override.
type Sets = (BTreeSet<WorkspacePermission>, BTreeSet<WorkspacePermission>);

/// The role overrides of one place, by the identifier of the role on the server.
type Rules = BTreeMap<StructureId, Sets>;

/// The overrides of a place as the server states them.
fn stored(overrides: &[OverrideEntry]) -> Rules {
    overrides
        .iter()
        .map(|entry| (entry.role, (set(&entry.allow), set(&entry.deny))))
        .collect()
}

/// The overrides that a file states for a place, by the identifier of the role on the server.
fn stated(overrides: &[OverrideEntry], role_names: &HashMap<StructureId, StructureId>) -> Rules {
    overrides
        .iter()
        .filter_map(|entry| {
            let role = role_names.get(&entry.role)?;
            Some((*role, (set(&entry.allow), set(&entry.deny))))
        })
        .collect()
}

/// The overrides of a place after the stated ones: an override with two empty sets is removed,
/// and an override that is not stated is kept.
fn merged(mut base: Rules, stated: &Rules) -> Rules {
    for (role, sets) in stated {
        if sets.0.is_empty() && sets.1.is_empty() {
            base.remove(role);
        } else {
            base.insert(*role, sets.clone());
        }
    }
    base
}

#[derive(Clone, Copy)]
enum Place<'a> {
    Category(StructureId, &'a str),
    Channel(StructureId, &'a str),
}

/// The changes that set the stated overrides of a place. With `before`, an override that
/// equals the one of the server is no change; without it, the server state is not known and
/// every stated override is sent: an override that the server holds already changes nothing.
fn override_changes(
    planner: &mut Planner<'_>,
    place: Place<'_>,
    stated: &Rules,
    before: Option<&Rules>,
) {
    for (role, after) in stated {
        if let Some(before) = before
            && before.get(role).cloned().unwrap_or_default() == *after
        {
            continue;
        }
        let allow: Vec<_> = after.0.iter().copied().collect();
        let deny: Vec<_> = after.1.iter().copied().collect();
        let (what, id, name) = match place {
            Place::Category(id, name) => ("category", id, name),
            Place::Channel(id, name) => ("channel", id, name),
        };
        let line = if allow.is_empty() && deny.is_empty() {
            format!(
                "- override of the role {role} in the {what} {:?} ({id}): remove",
                name.trim()
            )
        } else {
            format!(
                "~ override of the role {role} in the {what} {:?} ({id}): allow {}, deny {}",
                name.trim(),
                names(&allow),
                names(&deny)
            )
        };
        let change = match place {
            Place::Category(..) => StructureChange::SetCategoryOverride {
                category: id,
                role: *role,
                allow,
                deny,
            },
            Place::Channel(..) => StructureChange::SetChannelOverride {
                channel: id,
                role: *role,
                allow,
                deny,
            },
        };
        planner.add(change, line);
    }
}

/// The changes of the overrides of a category, and its overrides after them in `rules`.
fn category_overrides(
    planner: &mut Planner<'_>,
    category: &CategoryEntry,
    rules: &mut HashMap<StructureId, Rules>,
    role_names: &HashMap<StructureId, StructureId>,
) {
    if !roles_exist(
        planner,
        ("category", category.id),
        &category.overrides,
        role_names,
    ) {
        return;
    }
    let stated = stated(&category.overrides, role_names);
    let before = rules.remove(&category.id).unwrap_or_default();
    let place = Place::Category(category.id, &category.name);
    override_changes(planner, place, &stated, Some(&before));
    rules.insert(category.id, merged(before, &stated));
}

/// The category of a channel after the changes of its entry.
fn category_after(channel: &ChannelEntry, existing: Option<&ChannelEntry>) -> Option<StructureId> {
    match channel.category {
        Some(category) => category,
        None => existing.and_then(|existing| existing.category.flatten()),
    }
}

/// What a channel uses after its creation or update, as the server decides it (ADR 0304):
/// whether it is synced with its category, and its own overrides. `None` when the server
/// state cannot be told from its export.
fn after_move(
    existing: Option<&ChannelEntry>,
    category: Option<StructureId>,
    rules: &HashMap<StructureId, Rules>,
) -> Option<(bool, Rules)> {
    // A new channel in a category is synced with it; a new channel without one has no
    // overrides.
    let Some(existing) = existing else {
        return Some((category.is_some(), Rules::new()));
    };
    let before = existing.category.flatten();
    let synced = before.is_some() && existing.synced == Some(true);
    let own = stored(&existing.overrides);
    if before == category {
        return Some((synced, own));
    }
    match (synced, before, category) {
        // A synced channel that moves takes the overrides of its new category, or keeps
        // those of its former category as its own when it leaves for none.
        (true, _, Some(_)) => Some((true, Rules::new())),
        (true, former, None) => Some((
            false,
            former
                .and_then(|former| rules.get(&former).cloned())
                .unwrap_or_default(),
        )),
        // A channel with its own overrides keeps them when it moves.
        (false, Some(_), _) => Some((false, own)),
        (false, None, _) if !own.is_empty() => Some((false, own)),
        // A channel without a category and without overrides takes the overrides of the
        // category that it moves into, unless it had overrides before; the export does not
        // tell which.
        (false, None, _) => None,
    }
}

/// The changes of the sync state and of the overrides of a channel, after its creation or
/// update.
fn channel_rules(
    planner: &mut Planner<'_>,
    channel: &ChannelEntry,
    existing: Option<&ChannelEntry>,
    rules: &HashMap<StructureId, Rules>,
    role_names: &HashMap<StructureId, StructureId>,
) {
    let id = channel.id;
    let name = channel.name.trim();
    let category = category_after(channel, existing);
    let category_rules = category
        .and_then(|category| rules.get(&category).cloned())
        .unwrap_or_default();
    let mut stated = stated(&channel.overrides, role_names);
    let state = after_move(existing, category, rules);
    let uses = match (channel.synced, category) {
        (Some(true), None) => {
            planner.problem(format!(
                "the channel {id} is synced with its category and has no category"
            ));
            return;
        }
        (Some(true), Some(_)) => {
            if state.as_ref().is_none_or(|(synced, _)| !synced) {
                planner.add(
                    StructureChange::SetChannelSync {
                        channel: id,
                        synced: true,
                    },
                    format!(
                        "~ channel {name:?} ({id}): sync with its category; its own \
                         overrides and the rules of people in it are replaced by those of the \
                         category"
                    ),
                );
            }
            Some(category_rules)
        }
        (Some(false), Some(_)) => match state {
            Some((false, own)) => Some(own),
            synced => {
                planner.add(
                    StructureChange::SetChannelSync {
                        channel: id,
                        synced: false,
                    },
                    format!(
                        "~ channel {name:?} ({id}): stop the sync with its category; it \
                         keeps the rules of the category as its own"
                    ),
                );
                // The overrides that the channel takes from its category are its own
                // after this change; those that the entry does not state are removed,
                // so that the channel has the overrides of the entry.
                for role in category_rules.keys() {
                    stated.entry(*role).or_default();
                }
                synced.map(|_| category_rules)
            }
        },
        (Some(false) | None, None) => state.map(|(_, own)| own),
        (None, Some(_)) => state.map(|(synced, own)| if synced { category_rules } else { own }),
    };
    override_changes(planner, Place::Channel(id, name), &stated, uses.as_ref());
}

fn create_channel(planner: &mut Planner<'_>, workspace: StructureId, channel: &ChannelEntry) {
    let id = channel.id;
    let kind = match channel.kind {
        ChannelKind::Text => "text",
        ChannelKind::Voice => "voice",
    };
    planner.add(
        StructureChange::CreateChannel {
            id,
            workspace,
            kind: channel.kind,
            name: channel.name.trim().to_owned(),
            category: channel.category.flatten(),
            position: channel.position,
        },
        format!("+ {kind} channel {:?} ({id}): create", channel.name.trim()),
    );
    if channel.permanent_messages == Some(true) {
        planner.add(
            StructureChange::UpdateChannel {
                id,
                name: None,
                category: None,
                position: None,
                permanent_messages: Some(true),
            },
            format!(
                "~ channel {:?} ({id}): permanent messages on",
                channel.name.trim()
            ),
        );
    }
    let user_limit = channel.user_limit.filter(Option::is_some);
    let hides_occupants = channel.hides_occupants.filter(|hides| *hides);
    if user_limit.is_some() || hides_occupants.is_some() {
        planner.add(
            StructureChange::UpdateCallSettings {
                id,
                user_limit,
                hides_occupants,
            },
            format!(
                "~ channel {:?} ({id}): user limit {}, hides occupants {}",
                channel.name.trim(),
                limit(user_limit.flatten()),
                hides_occupants.unwrap_or(false)
            ),
        );
    }
}

/// The changes of an existing channel. False when the entry cannot be applied.
fn update_channel(
    planner: &mut Planner<'_>,
    channel: &ChannelEntry,
    existing: &ChannelEntry,
) -> bool {
    let id = existing.id;
    if channel.kind != existing.kind {
        planner.problem(format!(
            "the channel {id} has another kind on the server; the kind of a channel does not \
             change"
        ));
        return false;
    }
    let mut differences = Differences::default();
    let name = differences.name(&existing.name, &channel.name);
    let category = differences.value("category", existing.category, channel.category, reference);
    let position = differences.value("position", existing.position, channel.position, |value| {
        value.to_string()
    });
    let permanent_messages = differences.value(
        "permanent messages",
        existing.permanent_messages,
        channel.permanent_messages,
        |value| value.to_string(),
    );
    if !differences.is_empty() {
        let line = format!(
            "~ channel {:?} ({id}): {}",
            existing.name,
            differences.line()
        );
        planner.add(
            StructureChange::UpdateChannel {
                id,
                name,
                category,
                position,
                permanent_messages,
            },
            line,
        );
    }
    let mut settings = Differences::default();
    let user_limit = settings.value("user limit", existing.user_limit, channel.user_limit, limit);
    let hides_occupants = settings.value(
        "hides occupants",
        existing.hides_occupants,
        channel.hides_occupants,
        |value| value.to_string(),
    );
    if !settings.is_empty() {
        let line = format!("~ channel {:?} ({id}): {}", existing.name, settings.line());
        planner.add(
            StructureChange::UpdateCallSettings {
                id,
                user_limit,
                hides_occupants,
            },
            line,
        );
    }
    true
}

fn planned_positions(
    entries: &[(StructureId, Option<i32>)],
    current: &[(StructureId, Option<i32>)],
) -> Vec<junto_protocol::structure_file::StructureRolePosition> {
    use junto_protocol::structure_file::StructureRolePosition;
    let new: Vec<_> = entries
        .iter()
        .filter(|(id, _)| !current.iter().any(|(c, _)| c == id))
        .map(|(id, _)| *id)
        .collect();
    let added = i32::try_from(new.len()).unwrap_or(i32::MAX);
    let mut ordered: Vec<_> = entries
        .iter()
        .filter_map(|(id, wanted)| {
            let wanted = (*wanted)?;
            let predicted = if let Some(index) = new.iter().position(|n| n == id) {
                i32::try_from(new.len() - index).ok()
            } else {
                current
                    .iter()
                    .find(|(c, _)| c == id)
                    .and_then(|(_, p)| *p)
                    .and_then(|p| p.checked_add(added))
            }?;
            Some((*id, wanted, predicted))
        })
        .collect();
    let mut slots: Vec<_> = ordered.iter().map(|(_, _, p)| *p).collect();
    slots.sort_unstable();
    ordered.sort_by_key(|(_, wanted, _)| *wanted);
    ordered
        .into_iter()
        .zip(slots)
        .filter_map(|((id, _, old), position)| {
            (old != position).then_some(StructureRolePosition { id, position })
        })
        .collect()
}

fn role_positions(planner: &mut Planner<'_>, file: &StructureFile) {
    let entries: Vec<_> = file
        .server_roles
        .iter()
        .map(|r| (r.id, r.position))
        .collect();
    let current: Vec<_> = planner
        .current
        .server_roles
        .iter()
        .map(|r| (r.id, r.position))
        .collect();
    let positions = planned_positions(&entries, &current);
    if !positions.is_empty() {
        planner.add(
            StructureChange::ReorderServerRoles {
                positions: positions.clone(),
            },
            position_line("server roles", &positions),
        );
    }
    for w in &file.workspaces {
        let current = planner
            .current
            .workspaces
            .iter()
            .find(|c| c.id == w.id)
            .or_else(|| {
                if w.default {
                    planner.current.workspaces.iter().find(|c| c.default)
                } else {
                    None
                }
            });
        let entries: Vec<_> = w
            .roles
            .iter()
            .filter(|r| !r.everyone)
            .map(|r| (r.id, r.position))
            .collect();
        let stored: Vec<_> = current
            .map(|w| {
                w.roles
                    .iter()
                    .filter(|r| !r.everyone)
                    .map(|r| (r.id, r.position))
                    .collect()
            })
            .unwrap_or_default();
        let positions = planned_positions(&entries, &stored);
        if !positions.is_empty() {
            let workspace = current.map_or(w.id, |c| c.id);
            planner.add(
                StructureChange::ReorderWorkspaceRoles {
                    workspace,
                    positions: positions.clone(),
                },
                position_line(&format!("workspace {} roles", w.id), &positions),
            );
        }
    }
}

fn position_line(
    scope: &str,
    positions: &[junto_protocol::structure_file::StructureRolePosition],
) -> String {
    let roles = positions
        .iter()
        .map(|p| format!("{} -> {}", p.id, p.position))
        .collect::<Vec<_>>()
        .join(", ");
    format!("~ {scope}: reorder {roles}")
}
