use std::sync::Arc;

use futures_channel::oneshot;
use gpui::Context;
use junto_client_core::model::{Named, PlaceId};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{MessageId, ServerId, WorkspaceId};
use junto_client_core::protocol::message::{Block, Document, Message, MessagePlace};
use junto_client_core::protocol::origin::Origin;
use junto_client_core::{Owner, ServerHandle};

use super::{
    AppState,
    feed::{Names, Quoted},
};
use crate::text;

#[derive(Clone, Debug)]
pub(crate) struct Destination {
    pub server_id: ServerId,
    pub owner: Owner,
    pub workspace_id: Option<WorkspaceId>,
    pub place: PlaceId,
    pub label: String,
}

impl Destination {
    pub fn selector(&self) -> String {
        let kind = match self.place {
            PlaceId::Channel(id) => format!("channel:{id}"),
            PlaceId::Direct(id) => format!("direct:{id}"),
            PlaceId::Thread { root_id, .. } => format!("thread:{root_id}"),
        };
        format!("forward-destination:{}:{kind}", self.server_id)
    }
}

pub(crate) struct Forwarded {
    pub message: Arc<Message>,
    pub server_id: ServerId,
    pub names: Names,
    pub place: String,
}

pub(crate) enum Preview {
    Reading,
    Unavailable,
    Read(Forwarded),
}

impl AppState {
    pub(crate) fn forward_destinations(&self) -> Vec<Destination> {
        let mut rows = Vec::new();
        for server in self.client.servers() {
            let Some(owner) = server.owner() else {
                continue;
            };
            let Some(model) = server
                .live()
                .view()
                .model
                .filter(|model| model.account.id == owner.account_id)
            else {
                continue;
            };
            let server_id = server.id();
            let name = server.entry().name;
            for workspace in &model.workspaces {
                for channel in &workspace.channels {
                    let channel = &channel.channel;
                    if model.holds_in(channel.id, WorkspacePermission::SendMessages) {
                        rows.push(Destination {
                            server_id,
                            owner,
                            workspace_id: Some(workspace.workspace.id),
                            place: PlaceId::Channel(channel.id),
                            label: format!(
                                "{name} · {} · #{}",
                                workspace.workspace.name, channel.name
                            ),
                        });
                    }
                }
            }
            for conversation in &model.conversations {
                let other = if conversation.low_account_id == model.account.id {
                    conversation.high_account_id
                } else {
                    conversation.low_account_id
                };
                if self.known.people.departures(server_id).contains_key(&other) {
                    continue;
                }
                let other = self
                    .known
                    .people
                    .name(server_id, other)
                    .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned());
                rows.push(Destination {
                    server_id,
                    owner,
                    workspace_id: None,
                    place: PlaceId::Direct(conversation.id),
                    label: format!("{name} · {other}"),
                });
            }
            let roots = model
                .timelines
                .iter()
                .filter_map(|timeline| match timeline.place {
                    PlaceId::Thread { root_id, .. } => Some(root_id),
                    _ => None,
                })
                .chain(model.threads.iter().map(|thread| thread.root_id));
            for root_id in roots {
                let root = model
                    .timelines
                    .iter()
                    .flat_map(|timeline| timeline.messages.iter().chain(timeline.root.iter()))
                    .find(|message| message.id == root_id)
                    .or_else(|| match model.named(root_id) {
                        Some(Named::Read(message)) => Some(message),
                        _ => None,
                    });
                let Some(root) = root else {
                    continue;
                };
                if let MessagePlace::Channel {
                    workspace_id,
                    channel_id,
                } = root.place
                    && model.holds_in(channel_id, WorkspacePermission::SendMessagesInThreads)
                {
                    let place = PlaceId::Thread {
                        channel_id,
                        root_id,
                    };
                    if rows
                        .iter()
                        .any(|row| row.server_id == server_id && row.place == place)
                    {
                        continue;
                    }
                    let channel = model
                        .channel(channel_id)
                        .map(|channel| channel.channel.name.as_str())
                        .unwrap_or(text::MESSAGES);
                    let title = root
                        .document
                        .as_ref()
                        .map(|document| super::feed::plain_text(document, &Names::default(), 72))
                        .unwrap_or_else(|| text::FORWARDED_MESSAGE.to_owned());
                    rows.push(Destination {
                        server_id,
                        owner,
                        workspace_id: Some(workspace_id),
                        place,
                        label: format!("{name} · #{channel} · {title}"),
                    });
                }
            }
        }
        rows.sort_by_key(|row| row.label.to_lowercase());
        rows
    }

    pub(crate) fn forward_preview(&self, origin: &Origin, id: MessageId) -> Preview {
        let Some(server) = self
            .client
            .servers()
            .into_iter()
            .find(|server| server.entry().address.origin() == origin)
        else {
            return Preview::Unavailable;
        };
        let Some(owner) = server.owner() else {
            return Preview::Unavailable;
        };
        let Some(model) = server
            .live()
            .view()
            .model
            .filter(|model| model.account.id == owner.account_id)
        else {
            return Preview::Reading;
        };
        let message = match self.known.quoted.get(&(server.id(), id)) {
            Some(Quoted::Read(message)) if message.id == id && !message.deleted => message.clone(),
            None => return Preview::Reading,
            _ => return Preview::Unavailable,
        };
        let (workspace, place) = match message.place {
            MessagePlace::Channel {
                workspace_id,
                channel_id,
            } => {
                if model.permissions_in(channel_id).is_some_and(|permissions| {
                    !permissions.contains(&WorkspacePermission::ReadMessageHistory)
                }) {
                    return Preview::Unavailable;
                }
                let name = model
                    .channel(channel_id)
                    .map(|channel| channel.channel.name.as_str())
                    .or_else(|| {
                        model
                            .admin
                            .archived_channel(channel_id)
                            .map(|channel| channel.channel.name.as_str())
                    })
                    .unwrap_or(text::MESSAGES);
                (model.workspace(workspace_id), format!("#{name}"))
            }
            MessagePlace::Direct {
                conversation_id, ..
            } => {
                if !model
                    .conversations
                    .iter()
                    .any(|conversation| conversation.id == conversation_id)
                {
                    return Preview::Unavailable;
                }
                (None, text::DIRECT_MESSAGES.to_owned())
            }
        };
        let place = format!("{} · {place}", server.entry().name);
        Preview::Read(Forwarded {
            names: super::view::names(server.id(), workspace, &model, &self.known),
            server_id: server.id(),
            message,
            place,
        })
    }

    pub(crate) fn forward_message(
        &mut self,
        origin: Origin,
        id: MessageId,
        destination: Destination,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<(), String>> {
        let (answer, receive) = oneshot::channel();
        let handles: Vec<(ServerHandle, Owner)> = self
            .client
            .servers()
            .into_iter()
            .filter_map(|server| Some((server.clone(), server.owner()?)))
            .collect();
        let target = handles
            .iter()
            .find(|(server, _)| server.id() == destination.server_id)
            .cloned();
        cx.spawn(async move |this, cx| {
            let prepared = prepare(origin, id, &handles).await;
            let _ = this.update(cx, move |this, cx| {
                if answer.is_canceled() {
                    return;
                }
                let result = (|| {
                    let (origin, id, owners) = prepared?;
                    let (target, owner) =
                        target.ok_or_else(|| text::FORWARD_UNAVAILABLE.to_owned())?;
                    if this.owner(target.id()) != Some(owner)
                        || owners
                            .iter()
                            .any(|(server, owner)| this.owner(*server) != Some(*owner))
                        || !this.forward_destination_allowed(&destination)
                    {
                        return Err(text::FORWARD_UNAVAILABLE.to_owned());
                    }
                    let document = Document {
                        blocks: vec![Block::Forward {
                            server: origin,
                            message_id: id,
                        }],
                    };
                    if target
                        .live()
                        .send_message(destination.place, document, None)
                        .is_none()
                    {
                        return Err(text::FORWARD_UNAVAILABLE.to_owned());
                    }
                    this.select_forward_destination(&destination);
                    this.changed(cx);
                    Ok(())
                })();
                let _ = answer.send(result);
            });
        })
        .detach();
        receive
    }

    pub(crate) fn forward_destination_label(&self, destination: &Destination) -> String {
        if let PlaceId::Thread { channel_id, .. } = destination.place
            && self
                .client
                .server(destination.server_id)
                .and_then(|server| server.live().view().model)
                .is_none_or(|model| {
                    !model.holds_in(channel_id, WorkspacePermission::ReadMessageHistory)
                })
        {
            return text::REPLY_IN_THREAD.to_owned();
        }
        destination.label.clone()
    }

    pub(crate) fn forward_destination_allowed(&self, destination: &Destination) -> bool {
        if self.owner(destination.server_id) != Some(destination.owner) {
            return false;
        }
        let Some(model) = self
            .client
            .server(destination.server_id)
            .and_then(|server| server.live().view().model)
        else {
            return false;
        };
        match destination.place {
            PlaceId::Channel(channel) => model.holds_in(channel, WorkspacePermission::SendMessages),
            PlaceId::Thread { channel_id, .. } => {
                model.holds_in(channel_id, WorkspacePermission::SendMessagesInThreads)
            }
            PlaceId::Direct(id) => model
                .conversations
                .iter()
                .find(|conversation| conversation.id == id)
                .is_some_and(|conversation| {
                    let other = if conversation.low_account_id == model.account.id {
                        conversation.high_account_id
                    } else {
                        conversation.low_account_id
                    };
                    !self
                        .known
                        .people
                        .departures(destination.server_id)
                        .contains_key(&other)
                }),
        }
    }

    pub(crate) fn resolve_forward_thread(
        &mut self,
        address: junto_client_core::address::ServerAddress,
        id: MessageId,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Result<Destination, String>> {
        let (answer, receive) = oneshot::channel();
        let server = self
            .client
            .servers()
            .into_iter()
            .find(|server| server.entry().address == address);
        cx.spawn(async move |this, cx| {
            let Some(server) = server else {
                let _ = answer.send(Err(text::FORWARD_UNAVAILABLE.to_owned()));
                return;
            };
            let Some(owner) = server.owner() else {
                let _ = answer.send(Err(text::FORWARD_UNAVAILABLE.to_owned()));
                return;
            };
            let read = server.api_for(owner.account_id).message(id).await;
            let _ = this.update(cx, move |this, _| {
                if answer.is_canceled() {
                    return;
                }
                let result = (|| {
                    let message = read.map_err(|_| text::FORWARD_UNAVAILABLE.to_owned())?;
                    if this.owner(server.id()) != Some(owner) || message.id != id || message.deleted
                    {
                        return Err(text::FORWARD_UNAVAILABLE.to_owned());
                    }
                    let MessagePlace::Channel {
                        workspace_id,
                        channel_id,
                    } = message.place
                    else {
                        return Err(text::FORWARD_UNAVAILABLE.to_owned());
                    };
                    let destination = Destination {
                        server_id: server.id(),
                        owner,
                        workspace_id: Some(workspace_id),
                        place: PlaceId::Thread {
                            channel_id,
                            root_id: message.thread_root.unwrap_or(message.id),
                        },
                        label: format!("{} · {}", server.entry().name, text::REPLY_IN_THREAD),
                    };
                    if !this.forward_destination_allowed(&destination) {
                        return Err(text::FORWARD_UNAVAILABLE.to_owned());
                    }
                    Ok(destination)
                })();
                let _ = answer.send(result);
            });
        })
        .detach();
        receive
    }

    fn select_forward_destination(&mut self, destination: &Destination) {
        self.end_jump();
        self.navigations += 1;
        self.selection.server = Some(destination.server_id);
        self.selection.thread = None;
        self.selection.pins = None;
        self.selection.notices = None;
        self.selection.transcript = None;
        match destination.place {
            PlaceId::Direct(id) => {
                self.selection
                    .conversations
                    .insert(destination.server_id, id);
            }
            PlaceId::Channel(channel_id) | PlaceId::Thread { channel_id, .. } => {
                if let Some(workspace) = destination.workspace_id {
                    self.selection
                        .workspaces
                        .insert(destination.server_id, workspace);
                    self.selection
                        .channels
                        .insert((destination.server_id, workspace), channel_id);
                }
                self.selection.conversations.remove(&destination.server_id);
                if let PlaceId::Thread { root_id, .. } = destination.place {
                    self.selection.thread = Some((destination.server_id, channel_id, root_id));
                }
            }
        }
    }
}

async fn prepare(
    mut origin: Origin,
    mut id: MessageId,
    handles: &[(ServerHandle, Owner)],
) -> Result<(Origin, MessageId, Vec<(ServerId, Owner)>), String> {
    let mut owners = Vec::new();
    for _ in 0..8 {
        let (server, owner) = handles
            .iter()
            .find(|(server, _)| server.entry().address.origin() == &origin)
            .ok_or_else(|| text::FORWARD_UNAVAILABLE.to_owned())?;
        owners.push((server.id(), *owner));
        let message = server
            .api_for(owner.account_id)
            .message(id)
            .await
            .map_err(|_| text::FORWARD_UNAVAILABLE.to_owned())?;
        if message.id != id || message.deleted || message.document.is_none() {
            return Err(text::FORWARD_UNAVAILABLE.to_owned());
        }
        if let Some([Block::Forward { server, message_id }]) = message
            .document
            .as_ref()
            .map(|document| document.blocks.as_slice())
        {
            origin = server.clone();
            id = *message_id;
        } else {
            return Ok((origin, id, owners));
        }
    }
    Err(text::FORWARD_UNAVAILABLE.to_owned())
}
