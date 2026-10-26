//! How the person arranges the server book (ADR 0775): named groups of servers with a
//! colour that can be collapsed, the order of the workspaces of each server, and the
//! servers whose workspaces are collapsed. The order of the servers stays the order of the
//! entries of the book (ADR 0780). A group stands at the place of its first server, and
//! its servers follow in the order of the book.

use std::collections::HashSet;

use junto_protocol::ids::{ServerId, WorkspaceId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const LONGEST_GROUP_NAME: usize = 64;
pub const MOST_GROUPS: usize = 128;
/// How many workspaces of one server the order names at most.
pub const MOST_ORDERED_WORKSPACES: usize = 512;

/// Names one group of the server book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupId(Uuid);

impl GroupId {
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

/// The colour of a group: one of the colours of the theme of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupColour {
    #[default]
    Bone,
    Terracotta,
    Ochre,
    Olive,
    Cypress,
    Bronze,
    Cinnabar,
}

impl GroupColour {
    pub const ALL: [GroupColour; 7] = [
        GroupColour::Bone,
        GroupColour::Terracotta,
        GroupColour::Ochre,
        GroupColour::Olive,
        GroupColour::Cypress,
        GroupColour::Bronze,
        GroupColour::Cinnabar,
    ];
}

/// One group of servers. Without a name the window names it by its servers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerGroup {
    pub id: GroupId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub colour: GroupColour,
    #[serde(default)]
    pub collapsed: bool,
}

/// What the person arranged of one server. A server without a record is in no group, has
/// its workspaces expanded, and lists them in the order of the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerPlace {
    pub server_id: ServerId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupId>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub workspaces_collapsed: bool,
    /// The workspaces in the order of the person. A workspace that it does not name
    /// follows those that it names, in the order of the server.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_order: Vec<WorkspaceId>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl ServerPlace {
    fn new(server_id: ServerId) -> Self {
        Self {
            server_id,
            group: None,
            workspaces_collapsed: false,
            workspace_order: Vec::new(),
        }
    }

    fn is_default(&self) -> bool {
        self.group.is_none() && !self.workspaces_collapsed && self.workspace_order.is_empty()
    }
}

/// The arrangement of the server book. It is stored in the file of the book.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Arrangement {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<ServerGroup>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerPlace>,
}

/// A top-level part of the server book: a server outside a group, or a group with its
/// servers in the order of the book.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shelf {
    Server(ServerId),
    Group {
        group: ServerGroup,
        servers: Vec<ServerId>,
    },
}

/// A server or a group, as a place that another one is moved next to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Server(ServerId),
    Group(GroupId),
}

/// Where a server or a group is moved to. A server moved next to a server of a group joins
/// that group; a group moved next to a server of a group stands next to that group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    Before(Item),
    After(Item),
    /// The end of a group, for a server. A group is put after that group.
    InGroup(GroupId),
    /// The end of the server book, outside every group.
    End,
}

/// Where a workspace is moved to among the workspaces of its server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceSpot {
    Before(WorkspaceId),
    After(WorkspaceId),
}

/// One change of the arrangement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    MoveServer {
        server: ServerId,
        to: Spot,
    },
    /// A server dropped on another one: a new group of the two at the place of the other
    /// one, or the group of the other one.
    GroupWith {
        server: ServerId,
        target: ServerId,
    },
    /// A new group of the server alone at its place, or after its group.
    NewGroup {
        server: ServerId,
        name: String,
    },
    /// The server leaves its group and stands directly after it.
    LeaveGroup {
        server: ServerId,
    },
    MoveGroup {
        group: GroupId,
        to: Spot,
    },
    RenameGroup {
        group: GroupId,
        name: String,
    },
    ColourGroup {
        group: GroupId,
        colour: GroupColour,
    },
    CollapseGroup {
        group: GroupId,
        collapsed: bool,
    },
    /// The group goes; its servers stay at its place in their order.
    DissolveGroup {
        group: GroupId,
    },
    CollapseWorkspaces {
        server: ServerId,
        collapsed: bool,
    },
    /// The workspaces of every server, and every group.
    CollapseEverything {
        collapsed: bool,
    },
    /// Moves a workspace among `shown`, the workspaces that the server lists now.
    MoveWorkspace {
        server: ServerId,
        workspace: WorkspaceId,
        to: WorkspaceSpot,
        shown: Vec<WorkspaceId>,
    },
}

/// Why a change was not made. Nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ArrangeError {
    #[error("the server {0} is not saved")]
    NoServer(ServerId),
    #[error("the group is gone")]
    NoGroup(GroupId),
    #[error("the workspace is not listed")]
    NoWorkspace(WorkspaceId),
    #[error("the server book holds {MOST_GROUPS} groups")]
    TooManyGroups,
}

/// A group name as it is stored: without control characters and outer spaces, cut to
/// [`LONGEST_GROUP_NAME`] characters.
pub fn group_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(LONGEST_GROUP_NAME)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl Arrangement {
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.servers.is_empty()
    }

    pub fn place(&self, server_id: ServerId) -> Option<&ServerPlace> {
        self.servers
            .iter()
            .find(|place| place.server_id == server_id)
    }

    pub fn group(&self, group_id: GroupId) -> Option<&ServerGroup> {
        self.groups.iter().find(|group| group.id == group_id)
    }

    pub fn group_of(&self, server_id: ServerId) -> Option<GroupId> {
        self.place(server_id).and_then(|place| place.group)
    }

    pub fn workspaces_collapsed(&self, server_id: ServerId) -> bool {
        self.place(server_id)
            .is_some_and(|place| place.workspaces_collapsed)
    }

    /// Puts `items` in the order of the person for the workspaces of a server. Items that
    /// the order does not name follow, in the order that they had.
    pub fn sort_workspaces<T>(
        &self,
        server_id: ServerId,
        items: &mut [T],
        id: impl Fn(&T) -> WorkspaceId,
    ) {
        let Some(place) = self.place(server_id) else {
            return;
        };
        if place.workspace_order.is_empty() {
            return;
        }
        let order = &place.workspace_order;
        items.sort_by_key(|item| {
            let id = id(item);
            order
                .iter()
                .position(|named| *named == id)
                .unwrap_or(usize::MAX)
        });
    }

    /// Checks the bounds of a stored arrangement.
    pub fn check(&self) -> Result<(), String> {
        if self.groups.len() > MOST_GROUPS {
            return Err("too many groups".to_owned());
        }
        let mut groups = HashSet::new();
        for group in &self.groups {
            if !groups.insert(group.id) {
                return Err("a group is listed twice".to_owned());
            }
            if group.name != group_name(&group.name) {
                return Err("a group has an invalid name".to_owned());
            }
        }
        let mut servers = HashSet::new();
        for place in &self.servers {
            if !servers.insert(place.server_id) {
                return Err(format!("server {} is arranged twice", place.server_id));
            }
            if place.workspace_order.len() > MOST_ORDERED_WORKSPACES {
                return Err(format!(
                    "server {} orders too many workspaces",
                    place.server_id
                ));
            }
        }
        Ok(())
    }

    /// The arrangement of the servers of `order` only: no record of another server, no
    /// group without a server, no reference to a group that is gone, and no record that
    /// arranges nothing.
    pub fn normalised(&self, order: &[ServerId]) -> Arrangement {
        let known: HashSet<ServerId> = order.iter().copied().collect();
        let groups: HashSet<GroupId> = self.groups.iter().map(|group| group.id).collect();
        let servers: Vec<ServerPlace> = self
            .servers
            .iter()
            .filter(|place| known.contains(&place.server_id))
            .map(|place| ServerPlace {
                group: place.group.filter(|group| groups.contains(group)),
                ..place.clone()
            })
            .filter(|place| !place.is_default())
            .collect();
        let used: HashSet<GroupId> = servers.iter().filter_map(|place| place.group).collect();
        Arrangement {
            groups: self
                .groups
                .iter()
                .filter(|group| used.contains(&group.id))
                .cloned()
                .collect(),
            servers,
        }
    }

    /// The top-level parts of the server book for the servers in `order`.
    pub fn shelves(&self, order: &[ServerId]) -> Vec<Shelf> {
        let mut shelves: Vec<Shelf> = Vec::new();
        for server in order {
            let group = self
                .group_of(*server)
                .and_then(|group_id| self.group(group_id));
            match group {
                None => shelves.push(Shelf::Server(*server)),
                Some(group) => {
                    let held = shelves.iter_mut().find_map(|shelf| match shelf {
                        Shelf::Group {
                            group: held,
                            servers,
                        } if held.id == group.id => Some(servers),
                        _ => None,
                    });
                    match held {
                        Some(servers) => servers.push(*server),
                        None => shelves.push(Shelf::Group {
                            group: group.clone(),
                            servers: vec![*server],
                        }),
                    }
                }
            }
        }
        shelves
    }

    /// Makes one change for the servers of `order`, which is the order of the book and
    /// takes the new order. On an error nothing changes.
    pub fn apply(&mut self, order: &mut Vec<ServerId>, change: Change) -> Result<(), ArrangeError> {
        let mut work = Work {
            shelves: self.shelves(order),
            arrangement: self.clone(),
        };
        work.apply(order, change)?;
        let (new_order, arrangement) = work.finish();
        *order = new_order;
        *self = arrangement.normalised(order);
        Ok(())
    }
}

/// A change in progress on the top-level parts of the book.
struct Work {
    shelves: Vec<Shelf>,
    arrangement: Arrangement,
}

/// Where a server is in the shelves: the index of its shelf, and its index in the group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Found {
    shelf: usize,
    inside: Option<usize>,
}

impl Work {
    fn find_server(&self, server: ServerId) -> Option<Found> {
        self.shelves
            .iter()
            .enumerate()
            .find_map(|(index, shelf)| match shelf {
                Shelf::Server(id) if *id == server => Some(Found {
                    shelf: index,
                    inside: None,
                }),
                Shelf::Group { servers, .. } => {
                    servers
                        .iter()
                        .position(|id| *id == server)
                        .map(|inside| Found {
                            shelf: index,
                            inside: Some(inside),
                        })
                }
                Shelf::Server(_) => None,
            })
    }

    fn find_group(&self, group_id: GroupId) -> Option<usize> {
        self.shelves
            .iter()
            .position(|shelf| matches!(shelf, Shelf::Group { group, .. } if group.id == group_id))
    }

    fn group_id_at(&self, shelf: usize) -> Option<GroupId> {
        match self.shelves.get(shelf)? {
            Shelf::Group { group, .. } => Some(group.id),
            Shelf::Server(_) => None,
        }
    }

    fn group_mut(&mut self, group_id: GroupId) -> Result<&mut ServerGroup, ArrangeError> {
        self.shelves
            .iter_mut()
            .find_map(|shelf| match shelf {
                Shelf::Group { group, .. } if group.id == group_id => Some(group),
                _ => None,
            })
            .ok_or(ArrangeError::NoGroup(group_id))
    }

    /// Takes a server out of its shelf. A group that it leaves empty goes.
    fn take_server(&mut self, server: ServerId) -> Result<(), ArrangeError> {
        let found = self
            .find_server(server)
            .ok_or(ArrangeError::NoServer(server))?;
        match found.inside {
            None => {
                self.shelves.remove(found.shelf);
            }
            Some(inside) => {
                let empty = match &mut self.shelves[found.shelf] {
                    Shelf::Group { servers, .. } => {
                        servers.remove(inside);
                        servers.is_empty()
                    }
                    Shelf::Server(_) => false,
                };
                if empty {
                    self.shelves.remove(found.shelf);
                }
            }
        }
        Ok(())
    }

    /// The top-level index that a spot names for a group or a server outside a group.
    fn top_level(&self, spot: Spot) -> Result<usize, ArrangeError> {
        let shelf_of = |item: Item| match item {
            Item::Server(server) => self
                .find_server(server)
                .map(|found| found.shelf)
                .ok_or(ArrangeError::NoServer(server)),
            Item::Group(group) => self.find_group(group).ok_or(ArrangeError::NoGroup(group)),
        };
        Ok(match spot {
            Spot::Before(item) => shelf_of(item)?,
            Spot::After(item) => shelf_of(item)? + 1,
            Spot::InGroup(group) => shelf_of(Item::Group(group))? + 1,
            Spot::End => self.shelves.len(),
        })
    }

    fn put_server(&mut self, server: ServerId, to: Spot) -> Result<(), ArrangeError> {
        let inside = |work: &Work, anchor: ServerId, after: bool| {
            let found = work
                .find_server(anchor)
                .ok_or(ArrangeError::NoServer(anchor))?;
            Ok::<_, ArrangeError>((found, usize::from(after)))
        };
        let (found, offset) = match to {
            Spot::Before(Item::Server(anchor)) => inside(self, anchor, false)?,
            Spot::After(Item::Server(anchor)) => inside(self, anchor, true)?,
            Spot::InGroup(group) => {
                let shelf = self.find_group(group).ok_or(ArrangeError::NoGroup(group))?;
                if let Shelf::Group { servers, .. } = &mut self.shelves[shelf] {
                    servers.push(server);
                }
                return Ok(());
            }
            Spot::Before(Item::Group(_)) | Spot::After(Item::Group(_)) | Spot::End => {
                let place = self.top_level(to)?;
                self.shelves.insert(place, Shelf::Server(server));
                return Ok(());
            }
        };
        match found.inside {
            None => self
                .shelves
                .insert(found.shelf + offset, Shelf::Server(server)),
            Some(index) => {
                if let Shelf::Group { servers, .. } = &mut self.shelves[found.shelf] {
                    servers.insert(index + offset, server);
                }
            }
        }
        Ok(())
    }

    fn new_group(&self, name: &str) -> Result<ServerGroup, ArrangeError> {
        let groups = self
            .shelves
            .iter()
            .filter(|shelf| matches!(shelf, Shelf::Group { .. }))
            .count();
        if groups >= MOST_GROUPS {
            return Err(ArrangeError::TooManyGroups);
        }
        Ok(ServerGroup {
            id: GroupId::generate(),
            name: group_name(name),
            colour: GroupColour::default(),
            collapsed: false,
        })
    }

    fn place_mut(&mut self, server: ServerId) -> &mut ServerPlace {
        let places = &mut self.arrangement.servers;
        let index = match places.iter().position(|place| place.server_id == server) {
            Some(index) => index,
            None => {
                places.push(ServerPlace::new(server));
                places.len() - 1
            }
        };
        &mut places[index]
    }

    fn apply(&mut self, order: &[ServerId], change: Change) -> Result<(), ArrangeError> {
        match change {
            Change::MoveServer { server, to } => {
                self.find_server(server)
                    .ok_or(ArrangeError::NoServer(server))?;
                if matches!(to, Spot::Before(Item::Server(anchor)) | Spot::After(Item::Server(anchor)) if anchor == server)
                {
                    return Ok(());
                }
                let before = self.shelves.clone();
                self.take_server(server)?;
                if let Err(error) = self.put_server(server, to) {
                    // The spot was the group that the server left empty: it stays.
                    self.shelves = before;
                    return Err(error);
                }
            }
            Change::GroupWith { server, target } => {
                if server == target {
                    return Ok(());
                }
                self.find_server(server)
                    .ok_or(ArrangeError::NoServer(server))?;
                let found = self
                    .find_server(target)
                    .ok_or(ArrangeError::NoServer(target))?;
                if found.inside.is_some() {
                    self.take_server(server)?;
                    return self.put_server(server, Spot::After(Item::Server(target)));
                }
                let group = self.new_group("")?;
                self.take_server(server)?;
                let shelf = self
                    .find_server(target)
                    .ok_or(ArrangeError::NoServer(target))?
                    .shelf;
                self.shelves[shelf] = Shelf::Group {
                    group,
                    servers: vec![target, server],
                };
            }
            Change::NewGroup { server, name } => {
                let found = self
                    .find_server(server)
                    .ok_or(ArrangeError::NoServer(server))?;
                let group = self.new_group(&name)?;
                match found.inside {
                    None => {
                        self.shelves[found.shelf] = Shelf::Group {
                            group,
                            servers: vec![server],
                        };
                    }
                    Some(_) => {
                        let left = self.group_id_at(found.shelf);
                        self.take_server(server)?;
                        let place = match left.and_then(|left| self.find_group(left)) {
                            Some(shelf) => shelf + 1,
                            None => found.shelf,
                        };
                        self.shelves.insert(
                            place,
                            Shelf::Group {
                                group,
                                servers: vec![server],
                            },
                        );
                    }
                }
            }
            Change::LeaveGroup { server } => {
                let found = self
                    .find_server(server)
                    .ok_or(ArrangeError::NoServer(server))?;
                if found.inside.is_none() {
                    return Ok(());
                }
                let left = self.group_id_at(found.shelf);
                self.take_server(server)?;
                let place = match left.and_then(|left| self.find_group(left)) {
                    Some(shelf) => shelf + 1,
                    None => found.shelf,
                };
                self.shelves.insert(place, Shelf::Server(server));
            }
            Change::MoveGroup { group, to } => {
                let shelf = self.find_group(group).ok_or(ArrangeError::NoGroup(group))?;
                let anchor_is_group = |item: Item| match item {
                    Item::Group(other) => other == group,
                    Item::Server(server) => self
                        .find_server(server)
                        .is_some_and(|found| found.shelf == shelf),
                };
                let to_itself = match to {
                    Spot::Before(item) | Spot::After(item) => anchor_is_group(item),
                    Spot::InGroup(other) => other == group,
                    Spot::End => false,
                };
                if to_itself {
                    return Ok(());
                }
                let moved = self.shelves.remove(shelf);
                match self.top_level(to) {
                    Ok(place) => self.shelves.insert(place, moved),
                    Err(error) => {
                        self.shelves.insert(shelf, moved);
                        return Err(error);
                    }
                }
            }
            Change::RenameGroup { group, name } => {
                self.group_mut(group)?.name = group_name(&name);
            }
            Change::ColourGroup { group, colour } => {
                self.group_mut(group)?.colour = colour;
            }
            Change::CollapseGroup { group, collapsed } => {
                self.group_mut(group)?.collapsed = collapsed;
            }
            Change::DissolveGroup { group } => {
                let shelf = self.find_group(group).ok_or(ArrangeError::NoGroup(group))?;
                if let Shelf::Group { servers, .. } = self.shelves.remove(shelf) {
                    for (offset, server) in servers.into_iter().enumerate() {
                        self.shelves.insert(shelf + offset, Shelf::Server(server));
                    }
                }
            }
            Change::CollapseWorkspaces { server, collapsed } => {
                self.find_server(server)
                    .ok_or(ArrangeError::NoServer(server))?;
                self.place_mut(server).workspaces_collapsed = collapsed;
            }
            Change::CollapseEverything { collapsed } => {
                for server in order {
                    self.place_mut(*server).workspaces_collapsed = collapsed;
                }
                for shelf in &mut self.shelves {
                    if let Shelf::Group { group, .. } = shelf {
                        group.collapsed = collapsed;
                    }
                }
            }
            Change::MoveWorkspace {
                server,
                workspace,
                to,
                shown,
            } => {
                self.find_server(server)
                    .ok_or(ArrangeError::NoServer(server))?;
                let mut list = shown;
                self.arrangement
                    .sort_workspaces(server, &mut list, |id| *id);
                let from = list
                    .iter()
                    .position(|id| *id == workspace)
                    .ok_or(ArrangeError::NoWorkspace(workspace))?;
                let (anchor, after) = match to {
                    WorkspaceSpot::Before(anchor) => (anchor, false),
                    WorkspaceSpot::After(anchor) => (anchor, true),
                };
                if anchor == workspace {
                    return Ok(());
                }
                if !list.contains(&anchor) {
                    return Err(ArrangeError::NoWorkspace(anchor));
                }
                let moved = list.remove(from);
                let place = list
                    .iter()
                    .position(|id| *id == anchor)
                    .ok_or(ArrangeError::NoWorkspace(anchor))?
                    + usize::from(after);
                list.insert(place, moved);
                list.truncate(MOST_ORDERED_WORKSPACES);
                self.place_mut(server).workspace_order = list;
            }
        }
        Ok(())
    }

    /// The order of the servers and the arrangement that the shelves hold.
    fn finish(self) -> (Vec<ServerId>, Arrangement) {
        let Work {
            shelves,
            mut arrangement,
        } = self;
        let mut order = Vec::new();
        let mut groups = Vec::new();
        for place in &mut arrangement.servers {
            place.group = None;
        }
        for shelf in shelves {
            match shelf {
                Shelf::Server(server) => order.push(server),
                Shelf::Group { group, servers } => {
                    for server in servers {
                        order.push(server);
                        let place = match arrangement
                            .servers
                            .iter()
                            .position(|place| place.server_id == server)
                        {
                            Some(index) => index,
                            None => {
                                arrangement.servers.push(ServerPlace::new(server));
                                arrangement.servers.len() - 1
                            }
                        };
                        arrangement.servers[place].group = Some(group.id);
                    }
                    groups.push(group);
                }
            }
        }
        arrangement.groups = groups;
        (order, arrangement)
    }
}

#[cfg(test)]
mod tests;
