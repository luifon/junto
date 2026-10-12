//! Person drops use the same invitation and move choices as the person menu.
use super::view::ShellView;
use junto_client_core::protocol::ids::{AccountId, ChannelId, ServerId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersonDrag {
    pub server_id: ServerId,
    pub channel_id: ChannelId,
    pub account_id: AccountId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonDrop {
    Invite,
    Move,
}

impl ShellView {
    pub fn person_drop(&self, person: PersonDrag, destination: ChannelId) -> Option<PersonDrop> {
        if self.selected.as_ref()?.id != person.server_id || person.channel_id == destination {
            return None;
        }
        let menu = self.person_menu(person.channel_id, person.account_id)?;
        if menu.destinations.iter().any(|(id, _)| *id == destination) {
            Some(PersonDrop::Move)
        } else if menu.invite_to.is_some()
            && self.call.as_ref().is_some_and(|call| {
                call.server_id == person.server_id && call.channel_id == Some(destination)
            })
        {
            Some(PersonDrop::Invite)
        } else {
            None
        }
    }
}
