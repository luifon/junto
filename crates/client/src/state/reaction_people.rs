use std::collections::HashMap;

use gpui::{Context, Task};
use junto_client_core::protocol::ids::{AccountId, MessageId, ServerId};
use junto_client_core::protocol::message::ReactionPeople;
use junto_client_core::protocol::{WorkspacePermission, events::Event};
use junto_client_core::{
    Owner,
    model::{Named, PlaceId},
};

use super::AppState;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub owner: Owner,
    pub place: PlaceId,
    pub message: MessageId,
    pub emoji: String,
    pub after: Option<AccountId>,
    pub before: Option<AccountId>,
}

impl Key {
    pub fn server_id(&self) -> ServerId {
        self.owner.entry.server_id
    }
}

pub(crate) struct Read {
    pub page: Option<ReactionPeople>,
    pub failed: bool,
    number: u64,
    task: Option<Task<()>>,
}

#[derive(Default)]
pub(crate) struct Reactions {
    entries: HashMap<Key, Read>,
    number: u64,
    #[cfg(feature = "test-support")]
    page_size: Option<u32>,
    #[cfg(feature = "test-support")]
    fixtures: std::collections::HashSet<Key>,
}

impl AppState {
    pub(crate) fn reaction_people_owner_matches(&self, key: &Key) -> bool {
        #[cfg(feature = "test-support")]
        if self.reactions.fixtures.contains(key) {
            return true;
        }
        self.owner(key.server_id()) == Some(key.owner)
    }

    #[cfg(feature = "test-support")]
    pub fn set_reaction_page_size(&mut self, size: u32, cx: &mut Context<Self>) {
        self.reactions.page_size = Some(size.clamp(1, 50));
        self.reactions.entries.clear();
        self.changed(cx);
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn show_reaction_people_fixture(
        &mut self,
        key: Key,
        page: ReactionPeople,
        cx: &mut Context<Self>,
    ) {
        self.reactions.fixtures.insert(key.clone());
        self.reactions.entries.insert(
            key,
            Read {
                page: Some(page),
                failed: false,
                number: 0,
                task: None,
            },
        );
        self.changed(cx);
    }

    pub(crate) fn reaction_people_allowed(&self, key: &Key) -> bool {
        #[cfg(feature = "test-support")]
        if self.reactions.fixtures.contains(key) {
            return true;
        }
        let Some(server) = self.client.server(key.server_id()) else {
            return false;
        };
        if server.owner() != Some(key.owner) {
            return false;
        }
        let Some(model) = server.live().view().model else {
            return false;
        };
        if model.account.id != key.owner.account_id || model.place(key.place).is_none() {
            return false;
        }
        if let Some(Named::Read(message)) = model.named(key.message)
            && message.deleted
        {
            return false;
        }
        match key.place {
            PlaceId::Channel(channel_id) | PlaceId::Thread { channel_id, .. } => {
                model
                    .permissions_in(channel_id)
                    .is_none_or(|held| held.contains(&WorkspacePermission::ReadMessageHistory))
                    || model.timeline(key.place).is_some_and(|timeline| {
                        !timeline.stale && timeline.message(key.message).is_some()
                    })
            }
            PlaceId::Direct(_) => true,
        }
    }

    pub(crate) fn reaction_emojis(&self, key: &Key) -> Option<Vec<String>> {
        if !self.reaction_people_allowed(key) {
            return None;
        }
        let model = self.client.server(key.server_id())?.live().view().model?;
        match model.named(key.message)? {
            Named::Read(message) => Some(
                message
                    .reactions
                    .iter()
                    .map(|reaction| reaction.emoji.clone())
                    .collect(),
            ),
            _ => None,
        }
    }

    pub(crate) fn reaction_people_read(&self, key: &Key) -> Option<&Read> {
        self.reaction_people_allowed(key)
            .then(|| self.reactions.entries.get(key))
            .flatten()
    }

    pub(crate) fn read_reaction_people(&mut self, key: Key, again: bool, cx: &mut Context<Self>) {
        if !self.reaction_people_allowed(&key) {
            return;
        }
        if !again && self.reactions.entries.contains_key(&key) {
            return;
        }
        let Some(server) = self.client.server(key.server_id()) else {
            return;
        };
        let api = server.api_for(key.owner.account_id);
        if !self.reactions.entries.contains_key(&key)
            && self.reactions.entries.len() >= 8
            && let Some(oldest) = self
                .reactions
                .entries
                .iter()
                .min_by_key(|(_, held)| held.number)
                .map(|(key, _)| key.clone())
        {
            self.reactions.entries.remove(&oldest);
        }
        self.reactions.number = self.reactions.number.wrapping_add(1);
        let number = self.reactions.number;
        self.reactions.entries.insert(
            key.clone(),
            Read {
                page: None,
                failed: false,
                number,
                task: None,
            },
        );
        let asked = key.clone();
        #[cfg(feature = "test-support")]
        let limit = self.reactions.page_size.unwrap_or(50);
        #[cfg(not(feature = "test-support"))]
        let limit = 50;
        let task = cx.spawn(async move |this, cx| {
            let result = api
                .reaction_people(
                    asked.message,
                    &asked.emoji,
                    asked.after,
                    asked.before,
                    limit,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.owner(asked.server_id()) != Some(asked.owner) {
                    return;
                }
                let Some(held) = this
                    .reactions
                    .entries
                    .get_mut(&asked)
                    .filter(|held| held.number == number)
                else {
                    return;
                };
                match result {
                    Ok(page) => held.page = Some(page),
                    Err(_) => held.failed = true,
                }
                held.task = None;
                this.changed(cx);
            });
        });
        if let Some(held) = self.reactions.entries.get_mut(&key) {
            held.task = Some(task);
        }
        self.changed(cx);
    }

    pub(super) fn reaction_people_changed(
        &mut self,
        server: ServerId,
        event: Option<&Event>,
        cx: &mut Context<Self>,
    ) {
        let message = match event {
            Some(Event::MessageChanged { message_id, .. }) => Some(*message_id),
            Some(
                Event::AccountProfileUpdated { .. }
                | Event::AccountDepartureChanged { .. }
                | Event::PermissionsChanged { .. }
                | Event::ChannelHistoryWithdrawn { .. }
                | Event::WorkspaceDeleted { .. }
                | Event::ChannelDeleted { .. }
                | Event::ChannelArchived { .. },
            )
            | None => None,
            _ => return,
        };
        let keys: Vec<Key> = self
            .reactions
            .entries
            .keys()
            .filter(|key| key.server_id() == server && message.is_none_or(|id| key.message == id))
            .cloned()
            .collect();
        if keys.is_empty() {
            return;
        }
        for key in keys {
            self.reactions.entries.remove(&key);
        }
        self.changed(cx);
    }

    pub(super) fn reaction_people_forget(&mut self, server: ServerId) {
        self.reactions
            .entries
            .retain(|key, _| key.server_id() != server);
    }
}
