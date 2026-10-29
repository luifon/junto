use gpui::Context;
use junto_client_core::Owner;
use junto_client_core::ServerHandle;
use junto_client_core::address::ServerAddress;
use junto_client_core::links::{self, AppLink};
use junto_client_core::model::{AdminPart, PlaceId};
use junto_client_core::protocol::ids::{AccountId, AttachmentId, ChannelId, ConversationId};
use junto_client_core::protocol::message::Message;
use junto_client_core::protocol::structure::Channel;

use super::AppState;

#[derive(Clone)]
pub struct OpenFileLink {
    pub owner: Owner,
    pub message: Message,
    pub attachment_id: AttachmentId,
}

impl gpui::EventEmitter<OpenFileLink> for AppState {}

#[derive(Clone, Copy)]
enum Target {
    File(AttachmentId),
    Channel(ChannelId),
    Conversation(ConversationId),
}

enum Destination {
    File(Message, AttachmentId),
    Channel(Channel),
    Conversation(ConversationId),
}

struct Readers {
    server: ServerHandle,
    parts: Vec<AdminPart>,
}

impl Drop for Readers {
    fn drop(&mut self) {
        for part in &self.parts {
            self.server.live().release_admin(*part);
        }
    }
}

impl Readers {
    async fn hold(&mut self, part: AdminPart) -> Option<()> {
        self.parts.push(part);
        self.server.live().open_admin(part).await.ok()
    }

    async fn channel(&mut self, account: AccountId, id: ChannelId) -> Option<Channel> {
        let channel = self.server.api_for(account).channel(id).await.ok()?;
        if channel.id != id {
            return None;
        }
        self.server.live().refresh().await.ok()?;
        if self.server.live().view().model?.channel(id).is_some() {
            return Some(channel);
        }
        self.hold(AdminPart::Archived(channel.workspace_id)).await?;
        self.hold(AdminPart::ChannelPermissions(id)).await?;
        self.server
            .live()
            .view()
            .model?
            .admin
            .archived_channel(id)?;
        Some(channel)
    }

    async fn conversation(
        &mut self,
        account: AccountId,
        id: ConversationId,
    ) -> Option<ConversationId> {
        let listed = self.server.api_for(account).conversations().await.ok()?;
        if !listed.iter().any(|conversation| conversation.id == id) {
            return None;
        }
        self.server.live().refresh().await.ok()?;
        self.server
            .live()
            .view()
            .model?
            .conversations
            .iter()
            .find(|conversation| conversation.id == id)?;
        Some(id)
    }

    async fn resolve(&mut self, account: AccountId, target: Target) -> Option<Destination> {
        match target {
            Target::Channel(id) => self.channel(account, id).await.map(Destination::Channel),
            Target::Conversation(id) => self
                .conversation(account, id)
                .await
                .map(Destination::Conversation),
            Target::File(id) => {
                let message = self
                    .server
                    .api_for(account)
                    .attachment_message(id)
                    .await
                    .ok()?;
                if message.deleted || !message.attachments.iter().any(|file| file.id == id) {
                    return None;
                }
                match PlaceId::of(&message.place) {
                    PlaceId::Channel(channel) => {
                        self.channel(account, channel).await?;
                    }
                    PlaceId::Direct(conversation) => {
                        self.conversation(account, conversation).await?;
                    }
                    PlaceId::Thread { .. } => return None,
                }
                Some(Destination::File(message, id))
            }
        }
    }
}

impl AppState {
    pub fn file_link(
        &self,
        server_id: junto_client_core::protocol::ids::ServerId,
        id: AttachmentId,
    ) -> Option<String> {
        let server = self.client.server(server_id)?;
        Some(links::format(&AppLink::File {
            server: server.entry().address,
            id,
        }))
    }

    /// False asks the URL router to open the add-server form with this origin.
    pub fn open_file_link(
        &mut self,
        address: &ServerAddress,
        id: AttachmentId,
        cx: &mut Context<Self>,
    ) -> bool {
        self.open_target_link(address, Target::File(id), cx)
    }

    /// False asks the URL router to open the add-server form with this origin.
    pub fn open_channel_link(
        &mut self,
        address: &ServerAddress,
        id: ChannelId,
        cx: &mut Context<Self>,
    ) -> bool {
        self.open_target_link(address, Target::Channel(id), cx)
    }

    /// False asks the URL router to open the add-server form with this origin.
    pub fn open_conversation_link(
        &mut self,
        address: &ServerAddress,
        id: ConversationId,
        cx: &mut Context<Self>,
    ) -> bool {
        self.open_target_link(address, Target::Conversation(id), cx)
    }

    fn open_target_link(
        &mut self,
        address: &ServerAddress,
        target: Target,
        cx: &mut Context<Self>,
    ) -> bool {
        self.message_link_asks = self.message_link_asks.wrapping_add(1);
        let number = self.message_link_asks;
        self.message_link_failed = None;
        let Some(server) = self
            .client
            .servers()
            .into_iter()
            .find(|server| server.entry().address == *address)
        else {
            self.changed(cx);
            return false;
        };
        let Some(owner) = server.owner() else {
            self.message_link_failed = Some(self.navigations);
            self.changed(cx);
            return true;
        };
        let server_id = server.id();
        let navigation = self.navigations;
        cx.spawn(async move |this, cx| {
            let mut readers = Readers {
                server,
                parts: Vec::new(),
            };
            let destination = readers.resolve(owner.account_id, target).await;
            let _ = this.update(cx, |this, cx| {
                if this.message_link_asks != number
                    || this.navigations != navigation
                    || this
                        .client
                        .server(server_id)
                        .and_then(|server| server.owner())
                        != Some(owner)
                {
                    return;
                }
                match destination {
                    Some(Destination::File(message, attachment_id)) => {
                        this.go_to(super::notices::Jump::to(server_id, &message), cx);
                        cx.emit(OpenFileLink {
                            owner,
                            message,
                            attachment_id,
                        });
                    }
                    Some(Destination::Channel(channel)) => {
                        this.selection.thread = None;
                        this.selection.pins = None;
                        this.selection.transcript = None;
                        this.known.fork = None;
                        this.show_channel(server_id, channel.workspace_id, channel.id, cx);
                    }
                    Some(Destination::Conversation(id)) => {
                        this.select_conversation(server_id, id, cx)
                    }
                    None => {
                        this.message_link_failed = Some(this.navigations);
                        this.changed(cx);
                    }
                }
            });
            // changed() acquires the window's readers before these temporary readers end.
            drop(readers);
        })
        .detach();
        self.changed(cx);
        true
    }
}
