use std::collections::HashSet;

use junto_protocol::ids::ChannelId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MOST_SECTIONS: usize = 128;
pub const MOST_CHANNELS: usize = 4096;
pub const LONGEST_NAME: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SectionId(Uuid);

impl SectionId {
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }
}

impl std::fmt::Display for SectionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Default,
    Favourites,
    Section(SectionId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: SectionId,
    pub name: String,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub channels: Vec<ChannelId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub favourites: Vec<ChannelId>,
    #[serde(default)]
    pub favourites_collapsed: bool,
    #[serde(default)]
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Place {
        channel: ChannelId,
        to: Location,
        before: Option<ChannelId>,
    },
    Create {
        id: SectionId,
        name: String,
        channel: Option<ChannelId>,
    },
    Rename {
        id: SectionId,
        name: String,
    },
    Remove(SectionId),
    Collapse {
        location: Location,
        collapsed: bool,
    },
    MoveSection {
        id: SectionId,
        up: bool,
    },
    MoveChannel {
        channel: ChannelId,
        up: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("a section name must have 1 to 64 characters and no control character")]
    InvalidName,
    #[error("too many personal sections or channel records")]
    Limit,
    #[error("the personal section is gone")]
    NoSection,
    #[error("the channel is not in the personal section")]
    NoChannel,
    #[error("a personal section or channel is listed twice")]
    Duplicate,
}

pub fn valid_name(name: &str) -> bool {
    name == name.trim()
        && !name.is_empty()
        && name.chars().count() <= LONGEST_NAME
        && !name.chars().any(char::is_control)
}

impl Layout {
    pub fn is_empty(&self) -> bool {
        self.favourites.is_empty() && !self.favourites_collapsed && self.sections.is_empty()
    }

    pub fn location(&self, channel: ChannelId) -> Location {
        if self.favourites.contains(&channel) {
            return Location::Favourites;
        }
        self.sections
            .iter()
            .find(|section| section.channels.contains(&channel))
            .map_or(Location::Default, |section| Location::Section(section.id))
    }

    pub fn channels(&self, location: Location) -> Option<&[ChannelId]> {
        match location {
            Location::Default => None,
            Location::Favourites => Some(&self.favourites),
            Location::Section(id) => self
                .sections
                .iter()
                .find(|section| section.id == id)
                .map(|section| section.channels.as_slice()),
        }
    }

    fn channels_mut(&mut self, location: Location) -> Result<&mut Vec<ChannelId>, Error> {
        match location {
            Location::Favourites => Ok(&mut self.favourites),
            Location::Section(id) => self
                .sections
                .iter_mut()
                .find(|section| section.id == id)
                .map(|section| &mut section.channels)
                .ok_or(Error::NoSection),
            Location::Default => Err(Error::NoSection),
        }
    }

    pub fn check(&self) -> Result<(), Error> {
        if self.sections.len() > MOST_SECTIONS
            || self.favourites.len()
                + self
                    .sections
                    .iter()
                    .map(|section| section.channels.len())
                    .sum::<usize>()
                > MOST_CHANNELS
        {
            return Err(Error::Limit);
        }
        let mut ids = HashSet::new();
        for section in &self.sections {
            if !valid_name(&section.name) {
                return Err(Error::InvalidName);
            }
            if !ids.insert(section.id) {
                return Err(Error::Duplicate);
            }
        }
        let mut channels = HashSet::new();
        for channel in self
            .favourites
            .iter()
            .chain(self.sections.iter().flat_map(|section| &section.channels))
        {
            if !channels.insert(*channel) {
                return Err(Error::Duplicate);
            }
        }
        Ok(())
    }

    pub fn apply(&mut self, change: Change) -> Result<(), Error> {
        let mut next = self.clone();
        next.apply_inner(change)?;
        next.check()?;
        *self = next;
        Ok(())
    }

    fn apply_inner(&mut self, change: Change) -> Result<(), Error> {
        match change {
            Change::Place {
                channel,
                to,
                before,
            } => {
                if before == Some(channel) {
                    return Ok(());
                }
                if to != Location::Default {
                    let target = self.channels(to).ok_or(Error::NoSection)?;
                    if before.is_some_and(|id| !target.contains(&id)) {
                        return Err(Error::NoChannel);
                    }
                }
                self.favourites.retain(|id| *id != channel);
                for section in &mut self.sections {
                    section.channels.retain(|id| *id != channel);
                }
                if to != Location::Default {
                    let target = self.channels_mut(to)?;
                    let position = before
                        .and_then(|id| target.iter().position(|held| *held == id))
                        .unwrap_or(target.len());
                    target.insert(position, channel);
                }
            }
            Change::Create { id, name, channel } => {
                self.sections.push(Section {
                    id,
                    name: name.trim().to_owned(),
                    collapsed: false,
                    channels: Vec::new(),
                });
                if let Some(channel) = channel {
                    self.apply_inner(Change::Place {
                        channel,
                        to: Location::Section(id),
                        before: None,
                    })?;
                }
            }
            Change::Rename { id, name } => {
                self.sections
                    .iter_mut()
                    .find(|section| section.id == id)
                    .ok_or(Error::NoSection)?
                    .name = name.trim().to_owned();
            }
            Change::Remove(id) => {
                let position = self
                    .sections
                    .iter()
                    .position(|section| section.id == id)
                    .ok_or(Error::NoSection)?;
                self.sections.remove(position);
            }
            Change::Collapse {
                location,
                collapsed,
            } => match location {
                Location::Favourites => self.favourites_collapsed = collapsed,
                Location::Section(id) => {
                    self.sections
                        .iter_mut()
                        .find(|section| section.id == id)
                        .ok_or(Error::NoSection)?
                        .collapsed = collapsed
                }
                Location::Default => return Err(Error::NoSection),
            },
            Change::MoveSection { id, up } => {
                let position = self
                    .sections
                    .iter()
                    .position(|section| section.id == id)
                    .ok_or(Error::NoSection)?;
                if let Some(next) = neighbour(position, self.sections.len(), up) {
                    self.sections.swap(position, next);
                }
            }
            Change::MoveChannel { channel, up } => {
                let target = self.channels_mut(self.location(channel))?;
                let position = target
                    .iter()
                    .position(|id| *id == channel)
                    .ok_or(Error::NoChannel)?;
                if let Some(next) = neighbour(position, target.len(), up) {
                    target.swap(position, next);
                }
            }
        }
        Ok(())
    }
}

fn neighbour(position: usize, count: usize, up: bool) -> Option<usize> {
    if up {
        position.checked_sub(1)
    } else {
        Some(position + 1).filter(|next| *next < count)
    }
}

#[cfg(test)]
mod tests;
