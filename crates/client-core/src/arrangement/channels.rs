use std::collections::HashSet;

use junto_protocol::ids::ChannelId;
use serde::{Deserialize, Serialize};

pub const MOST_CHANNELS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Default,
    Favourites,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Layout {
    #[serde(default)]
    pub favourites: Vec<ChannelId>,
    #[serde(default)]
    pub favourites_collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Place {
        channel: ChannelId,
        to: Location,
        before: Option<ChannelId>,
    },
    Collapse {
        collapsed: bool,
    },
    MoveChannel {
        channel: ChannelId,
        up: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("too many favourite channels")]
    Limit,
    #[error("the channel is not a favourite")]
    NoChannel,
    #[error("a favourite channel is listed twice")]
    Duplicate,
}

impl Layout {
    pub fn is_empty(&self) -> bool {
        self.favourites.is_empty() && !self.favourites_collapsed
    }

    pub fn location(&self, channel: ChannelId) -> Location {
        if self.favourites.contains(&channel) {
            Location::Favourites
        } else {
            Location::Default
        }
    }

    pub fn channels(&self, location: Location) -> Option<&[ChannelId]> {
        match location {
            Location::Default => None,
            Location::Favourites => Some(&self.favourites),
        }
    }

    pub fn check(&self) -> Result<(), Error> {
        if self.favourites.len() > MOST_CHANNELS {
            return Err(Error::Limit);
        }
        let mut channels = HashSet::new();
        if !self
            .favourites
            .iter()
            .all(|channel| channels.insert(*channel))
        {
            return Err(Error::Duplicate);
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
                if to == Location::Favourites
                    && before.is_some_and(|id| !self.favourites.contains(&id))
                {
                    return Err(Error::NoChannel);
                }
                self.favourites.retain(|id| *id != channel);
                if to == Location::Favourites {
                    let position = before
                        .and_then(|id| self.favourites.iter().position(|held| *held == id))
                        .unwrap_or(self.favourites.len());
                    self.favourites.insert(position, channel);
                }
            }
            Change::Collapse { collapsed } => self.favourites_collapsed = collapsed,
            Change::MoveChannel { channel, up } => {
                let position = self
                    .favourites
                    .iter()
                    .position(|id| *id == channel)
                    .ok_or(Error::NoChannel)?;
                let next = if up {
                    position.checked_sub(1)
                } else {
                    Some(position + 1).filter(|next| *next < self.favourites.len())
                };
                if let Some(next) = next {
                    self.favourites.swap(position, next);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
