use std::collections::HashMap;

use gpui::{Context, EntityId, Task};
use junto_client_core::protocol::{
    ids::{AccountId, ChannelId, ConversationId, ServerId, WorkspaceId},
    structure::ChannelKind,
};
use junto_client_core::{flow::FlowError, model::PlaceId};

use super::{
    AppState,
    contacts::{ContactSearch, LONGEST_PART, PAGE},
    view::{Unreads, shown_model},
};
use crate::text;

const MAX_READS: usize = 4;
const MAX_ROWS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Destination {
    Server(ServerId),
    Channel(ServerId, WorkspaceId, ChannelId),
    Conversation(ServerId, ConversationId),
    Person(ServerId, AccountId),
}

impl Destination {
    pub fn server(self) -> ServerId {
        match self {
            Self::Server(server)
            | Self::Channel(server, ..)
            | Self::Conversation(server, ..)
            | Self::Person(server, ..) => server,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchRow {
    pub destination: Destination,
    pub name: String,
    pub context: String,
    pub unread: bool,
    pub voice: bool,
    pub account_id: Option<AccountId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwitchView {
    pub rows: Vec<SwitchRow>,
    pub reading: bool,
    pub more: bool,
    pub failed: bool,
    pub limited: bool,
}

#[derive(Default)]
struct Search {
    session: u64,
    part: String,
    contacts: HashMap<ServerId, ContactSearch>,
    tasks: HashMap<ServerId, Task<()>>,
}

#[derive(Default)]
pub(super) struct SwitchSearches {
    sequence: u64,
    windows: HashMap<EntityId, Search>,
}

impl SwitchSearches {
    pub(super) fn window_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.windows.keys().copied()
    }
}

fn matches(part: &str, row: &SwitchRow) -> bool {
    let text = format!("{} {}", row.name, row.context).to_lowercase();
    part.split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

fn sorted(mut rows: Vec<SwitchRow>, part: &str) -> Vec<SwitchRow> {
    rows.retain(|row| matches(part, row));
    rows.sort_by_key(|row| {
        (
            !row.unread,
            row.name.to_lowercase(),
            row.context.to_lowercase(),
            row.destination,
        )
    });
    rows
}

impl AppState {
    pub fn open_switcher(&mut self, window: EntityId, cx: &mut Context<Self>) {
        self.switchers.sequence += 1;
        self.switchers.windows.insert(
            window,
            Search {
                session: self.switchers.sequence,
                ..Default::default()
            },
        );
        cx.notify();
    }

    pub fn close_switcher(&mut self, window: EntityId, cx: &mut Context<Self>) {
        self.switchers.windows.remove(&window);
        self.follow_presence_current(cx);
        cx.notify();
    }

    pub fn type_switcher(&mut self, window: EntityId, part: &str, cx: &mut Context<Self>) {
        let Some(search) = self.switchers.windows.get_mut(&window) else {
            return;
        };
        search.part = part.trim().chars().take(LONGEST_PART).collect();
        self.read_switcher(window, cx);
        cx.notify();
    }

    pub fn more_switcher(&mut self, window: EntityId, retry: bool, cx: &mut Context<Self>) {
        let Some(search) = self.switchers.windows.get_mut(&window) else {
            return;
        };
        for contacts in search.contacts.values_mut() {
            if retry {
                contacts.failed = None;
            } else {
                contacts.more_wanted = true;
            }
        }
        self.read_switcher(window, cx);
        cx.notify();
    }

    pub(super) fn refresh_switchers(&mut self, cx: &mut Context<Self>) {
        let windows: Vec<_> = self.switchers.windows.keys().copied().collect();
        for window in windows {
            self.read_switcher(window, cx);
        }
    }

    pub(super) fn invalidate_switcher(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        for search in self.switchers.windows.values_mut() {
            search.contacts.remove(&server_id);
            search.tasks.remove(&server_id);
        }
        self.refresh_switchers(cx);
    }

    fn read_switcher(&mut self, window: EntityId, cx: &mut Context<Self>) {
        self.switchers.sequence += 1;
        let contact_generation = self.switchers.sequence;
        let servers = self.snapshots();
        let Some(search) = self.switchers.windows.get_mut(&window) else {
            return;
        };
        search.contacts.retain(|id, contacts| {
            servers.iter().any(|server| {
                server.id == *id
                    && shown_model(server)
                        .is_some_and(|model| contacts.open == Some((*id, model.account.id)))
            })
        });
        search
            .tasks
            .retain(|id, _| search.contacts.contains_key(id));
        if search.part.is_empty() {
            return;
        }
        for server in &servers {
            let Some(model) = shown_model(server) else {
                continue;
            };
            let contacts = search.contacts.entry(server.id).or_insert_with(|| {
                let mut contacts = ContactSearch::default();
                contacts.open(server.id, model.account.id);
                contacts.session = contact_generation;
                contacts
            });
            contacts.want(&search.part);
        }
        let mut room = MAX_READS.saturating_sub(
            search
                .contacts
                .values()
                .filter(|contacts| contacts.reading.is_some())
                .count(),
        );
        let session = search.session;
        for (server_id, contacts) in &mut search.contacts {
            if room == 0 {
                break;
            }
            let Some(server) = self.client.server(*server_id) else {
                continue;
            };
            let Some((_, account_id)) = contacts.open else {
                continue;
            };
            let Some(read) = contacts.next_read() else {
                continue;
            };
            room -= 1;
            let contact_session = contacts.session;
            let server_id = *server_id;
            let api = server.api_for(account_id);
            let task = cx.spawn(async move |this, cx| {
                let answer = api
                    .contacts(&read.part, read.after.as_deref(), Some(PAGE))
                    .await
                    .map_err(|error| text::flow_error(&FlowError::from(&error)));
                let _ = this.update(cx, |this, cx| {
                    if let Some(search) = this
                        .switchers
                        .windows
                        .get_mut(&window)
                        .filter(|search| search.session == session)
                        && let Some(contacts) =
                            search.contacts.get_mut(&server_id).filter(|contacts| {
                                contacts.open == Some((server_id, account_id))
                                    && contacts.session == contact_session
                            })
                    {
                        contacts.answered(contact_session, read, answer);
                        search.tasks.remove(&server_id);
                        this.read_switcher(window, cx);
                        this.follow_presence_current(cx);
                        cx.notify();
                    }
                });
            });
            search.tasks.insert(server_id, task);
        }
    }

    pub fn switcher_view(&self, window: EntityId) -> Option<SwitchView> {
        let search = self.switchers.windows.get(&window)?;
        let servers = self.snapshots();
        let mut view = SwitchView::default();
        for server in &servers {
            let model = shown_model(server);
            let unreads = model.map(Unreads::of);
            let unread = unreads.as_ref().map(Unreads::total).unwrap_or_default();
            view.rows.push(SwitchRow {
                destination: Destination::Server(server.id),
                name: server.name.clone(),
                context: server.address.clone(),
                unread: unread.unread(),
                voice: false,
                account_id: None,
            });
            let (Some(model), Some(unreads)) = (model, unreads) else {
                continue;
            };
            for workspace in &model.workspaces {
                for channel in &workspace.channels {
                    let unread = unreads.at(PlaceId::Channel(channel.channel.id));
                    view.rows.push(SwitchRow {
                        destination: Destination::Channel(
                            server.id,
                            workspace.workspace.id,
                            channel.channel.id,
                        ),
                        name: channel.channel.name.clone(),
                        context: text::switcher::workspace_context(
                            &workspace.workspace.name,
                            &server.name,
                            &server.address,
                        ),
                        unread: unread.unread(),
                        voice: channel.channel.kind == ChannelKind::Voice,
                        account_id: None,
                    });
                }
            }
            let contacts = search
                .contacts
                .get(&server.id)
                .and_then(|contacts| contacts.view(&servers));
            for conversation in &model.conversations {
                let other = if conversation.low_account_id == model.account.id {
                    conversation.high_account_id
                } else {
                    conversation.low_account_id
                };
                let name = self
                    .known
                    .people
                    .name(server.id, other)
                    .or_else(|| {
                        model
                            .workspaces
                            .iter()
                            .flat_map(|workspace| workspace.members.loaded().into_iter().flatten())
                            .find(|member| member.account_id == other)
                            .map(|member| super::account::shown_name(&member.name))
                    })
                    .or_else(|| {
                        contacts.as_ref().and_then(|contacts| {
                            contacts
                                .rows
                                .as_ref()?
                                .iter()
                                .find(|contact| contact.account_id == other)
                                .map(|contact| contact.name.clone())
                        })
                    })
                    .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned());
                view.rows.push(SwitchRow {
                    destination: Destination::Conversation(server.id, conversation.id),
                    name,
                    context: text::switcher::server_context(&server.name, &server.address),
                    unread: unreads.at(PlaceId::Direct(conversation.id)).unread(),
                    voice: false,
                    account_id: Some(other),
                });
            }
            if search.part.is_empty() {
                continue;
            }
            let Some(contacts) = contacts else {
                continue;
            };
            view.reading |= contacts.reading && contacts.failed.is_none();
            view.more |= contacts.more;
            view.failed |= contacts.failed.is_some();
            for contact in contacts.rows.into_iter().flatten() {
                if model.conversation_with(contact.account_id).is_some() {
                    continue;
                }
                view.rows.push(SwitchRow {
                    destination: Destination::Person(server.id, contact.account_id),
                    name: contact.name,
                    context: text::switcher::server_context(&server.name, &server.address),
                    unread: false,
                    voice: false,
                    account_id: Some(contact.account_id),
                });
            }
        }
        view.rows = sorted(view.rows, &search.part);
        view.limited = view.rows.len() > MAX_ROWS;
        view.rows.truncate(MAX_ROWS);
        Some(view)
    }

    pub fn choose_switcher(
        &mut self,
        window: EntityId,
        destination: Destination,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(row) = self.switcher_view(window).and_then(|view| {
            view.rows
                .into_iter()
                .find(|row| row.destination == destination)
        }) else {
            return false;
        };
        match destination {
            Destination::Server(server) => self.select_server(server, cx),
            Destination::Channel(server, workspace, channel) => {
                self.show_channel(server, workspace, channel, cx)
            }
            Destination::Conversation(server, conversation) => {
                self.select_conversation(server, conversation, cx)
            }
            Destination::Person(server, account) => {
                self.select_server(server, cx);
                self.ask_conversation(server, account, Some(row.name), cx);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unread_results_precede_alphabetical_results_and_words_match_the_origin() {
        let server = ServerId::generate();
        let rows = vec![
            SwitchRow {
                destination: Destination::Server(server),
                name: "Alpha".into(),
                context: "Harbor".into(),
                unread: false,
                voice: false,
                account_id: None,
            },
            SwitchRow {
                destination: Destination::Server(ServerId::generate()),
                name: "Zulu".into(),
                context: "Mill".into(),
                unread: true,
                voice: false,
                account_id: None,
            },
        ];
        assert_eq!(sorted(rows.clone(), "")[0].name, "Zulu");
        assert_eq!(sorted(rows.clone(), "ALP har")[0].name, "Alpha");
        assert!(sorted(rows, "Alpha Mill").is_empty());
    }
}
