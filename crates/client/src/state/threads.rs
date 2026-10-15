//! What the application does for threads, pins and direct conversations (ADR 0741): which
//! thread and which pin list the side panel shows, the fork of a thread, a pin, and the
//! direct conversation that a server shows in place of a channel.

use gpui::Context;
use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{AccountId, ChannelId, ConversationId, MessageId, ServerId};
use junto_client_core::protocol::structure::ChannelKind;

use super::AppState;
use super::view::{ForkAsked, ForkState};
use crate::text;

/// Why the server did not fork a thread, as the dialog states it (ADR 0409).
fn fork_refusal(error: &ApiError) -> String {
    match error.category() {
        Some(ErrorCategory::NotAllowed) => text::FORK_NOT_ALLOWED.to_owned(),
        Some(ErrorCategory::Conflict) => text::FORK_EXISTS.to_owned(),
        Some(ErrorCategory::NotFound) => text::FORK_THREAD_GONE.to_owned(),
        _ if error.code() == Some(ErrorCode::InvalidRequest) => text::FORK_NAME_REFUSED.to_owned(),
        _ => text::flow_error(&FlowError::from(error)),
    }
}

impl AppState {
    /// Shows the thread under a message of a channel in the side panel.
    pub fn open_thread(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        root_id: MessageId,
        cx: &mut Context<Self>,
    ) {
        self.selection.thread = Some((server_id, channel_id, root_id));
        self.selection.pins = None;
        self.known.fork = None;
        self.changed(cx);
    }

    pub fn close_thread(&mut self, cx: &mut Context<Self>) {
        self.selection.thread = None;
        self.known.fork = None;
        self.changed(cx);
    }

    /// Shows the pinned messages of a channel in the side panel, in place of a thread.
    pub fn open_pins(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.selection.pins = Some((server_id, channel_id));
        self.selection.thread = None;
        self.changed(cx);
    }

    pub fn close_pins(&mut self, cx: &mut Context<Self>) {
        self.selection.pins = None;
        self.changed(cx);
    }

    /// Reads the pin list of the side panel again after a read that failed.
    pub fn reload_pins(&mut self, cx: &mut Context<Self>) {
        if let Some(((server_id, channel_id), _)) = self.open_pins.take()
            && let Some(server) = self.client.server(server_id)
        {
            server.live().release_pins(channel_id);
        }
        self.changed(cx);
    }

    /// Pins a message of a channel, or takes its pin away. A refusal is told in
    /// [`super::Pending::message_refused`]; the change arrives with the state.
    pub fn set_pinned(
        &mut self,
        server_id: ServerId,
        message_id: MessageId,
        pinned: bool,
        cx: &mut Context<Self>,
    ) {
        let refusal = |error: &ApiError| match error.code() {
            Some(ErrorCode::LimitReached) => text::PIN_LIMIT.to_owned(),
            _ if error.category() == Some(ErrorCategory::NotAllowed) => {
                text::PIN_NOT_ALLOWED.to_owned()
            }
            _ => text::flow_error(&FlowError::from(error)),
        };
        self.ask_about_messages(
            server_id,
            cx,
            move |api| async move {
                if pinned {
                    api.pin_message(message_id).await
                } else {
                    api.unpin_message(message_id).await
                }
            },
            refusal,
        );
    }

    /// Shows a message of an open place: the messages around its position replace what is
    /// loaded.
    pub fn show_position(&mut self, server_id: ServerId, place: PlaceId, position: u64) {
        if let Some(server) = self.client.server(server_id) {
            self.anchors.insert((server_id, place), Some(position));
            server.live().read_messages_around(place, position);
        }
    }

    /// Asks the server to copy a thread into a new channel. The state of the request is
    /// in the view of the thread; the new channel arrives with the state of the server.
    pub fn fork_thread(
        &mut self,
        server_id: ServerId,
        root_id: MessageId,
        name: String,
        kind: ChannelKind,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let api = server.api();
        self.fork_asks += 1;
        let number = self.fork_asks;
        self.known.fork = Some(ForkAsked {
            server_id,
            root_id,
            state: ForkState::Sending,
        });
        cx.spawn(async move |this, cx| {
            let asked = api.fork_thread(root_id, &name, kind).await;
            let _ = this.update(cx, |this, cx| {
                let current =
                    this.fork_asks == number
                        && this.known.fork.as_ref().is_some_and(|fork| {
                            fork.server_id == server_id && fork.root_id == root_id
                        });
                if !current {
                    return;
                }
                match asked {
                    // The thread shows its fork when the page of the thread was read again.
                    Ok(_) => this.known.fork = None,
                    Err(error) => {
                        this.known.fork = Some(ForkAsked {
                            server_id,
                            root_id,
                            state: ForkState::Refused(fork_refusal(&error)),
                        });
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        self.changed(cx);
    }

    /// Puts away what was told about the last fork.
    pub fn dismiss_fork(&mut self, cx: &mut Context<Self>) {
        self.known.fork = None;
        self.changed(cx);
    }

    /// Shows a direct conversation of a server in place of a channel.
    pub fn select_conversation(
        &mut self,
        server_id: ServerId,
        conversation_id: ConversationId,
        cx: &mut Context<Self>,
    ) {
        self.end_jump();
        self.navigations += 1;
        self.selection.server = Some(server_id);
        self.selection
            .conversations
            .insert(server_id, conversation_id);
        self.known.conversation_refused = None;
        self.changed(cx);
    }

    /// Opens the direct conversation with an account and shows it. The server creates the
    /// conversation when there is none, and refuses it when the two accounts may not
    /// write to each other (ADR 0307).
    pub fn open_conversation(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        // A conversation that the state holds needs no request.
        let held = server
            .live()
            .view()
            .model
            .and_then(|model| model.conversation_with(account_id).map(|held| held.id));
        if let Some(conversation_id) = held {
            self.select_conversation(server_id, conversation_id, cx);
            return;
        }
        let api = server.api();
        self.conversation_asks += 1;
        let number = self.conversation_asks;
        self.known.conversation_refused = None;
        self.known.conversation_asked = Some((server_id, account_id));
        // The answer does not take the person away from a place that they selected
        // after this request.
        let navigations = self.navigations;
        cx.spawn(async move |this, cx| {
            let asked = api.open_conversation(account_id).await;
            let _ = this.update(cx, |this, cx| {
                if this.conversation_asks != number {
                    return;
                }
                this.known.conversation_asked = None;
                let stayed = this.navigations == navigations;
                match asked {
                    // The conversation is shown when the state of the server holds it,
                    // which the event of the server brings.
                    Ok(_) if !stayed => {}
                    Ok(conversation) => {
                        this.navigations += 1;
                        this.selection.server = Some(server_id);
                        this.selection
                            .conversations
                            .insert(server_id, conversation.id);
                    }
                    Err(error) => {
                        let reason = match error.category() {
                            Some(ErrorCategory::NotFound) => text::NO_CONTACT.to_owned(),
                            _ => text::flow_error(&FlowError::from(&error)),
                        };
                        this.known.conversation_refused = Some((server_id, account_id, reason));
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub fn dismiss_conversation_refusal(&mut self, cx: &mut Context<Self>) {
        self.known.conversation_refused = None;
        self.changed(cx);
    }

    /// Draws this state of a fork and this refusal of a direct conversation in place of
    /// what the servers told. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_threads_fixture(
        &mut self,
        fork: Option<ForkAsked>,
        conversation_refused: Option<(ServerId, AccountId, String)>,
        cx: &mut Context<Self>,
    ) {
        self.known.fork = fork;
        self.known.conversation_refused = conversation_refused;
        self.changed(cx);
    }
}
