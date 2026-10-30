use super::{
    AppState,
    view::{DirectRow, ServerSnapshot},
};
use gpui::Context;
use junto_client_core::{
    Owner,
    model::PlaceId,
    protocol::ids::{ConversationId, ServerId},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Opening {
    owner: Owner,
    id: ConversationId,
    navigation: u64,
    pub(super) jump: Option<super::notices::Jump>,
}

impl Opening {
    fn same_request(self, other: Self) -> bool {
        self.owner == other.owner && self.id == other.id && self.navigation == other.navigation
    }
}

impl AppState {
    pub fn hidden_conversations(&self, server_id: ServerId) -> Vec<DirectRow> {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return Vec::new();
        };
        super::view::hidden_directs(server_id, &model, &self.known)
    }

    pub fn hide_conversation(&mut self, owner: Owner, id: ConversationId, cx: &mut Context<Self>) {
        let Some(server) = self
            .client
            .server(owner.entry.server_id)
            .filter(|server| server.owner() == Some(owner))
        else {
            return;
        };
        if server
            .live()
            .view()
            .model
            .is_none_or(|model| model.place(PlaceId::Direct(id)).is_none())
        {
            return;
        }
        let api = server.api_for(owner.account_id);
        let navigation = self.navigations;
        cx.spawn(async move |this, cx| {
            let answer = api.set_conversation_hidden(id, true).await;
            if answer.is_ok() {
                let refresh = this
                    .update(cx, |this, _| {
                        this.client
                            .server(owner.entry.server_id)
                            .filter(|server| {
                                server.owner() == Some(owner)
                                    && server.live().view().model.is_some_and(|model| {
                                        !model.hidden_conversations.contains(&id)
                                    })
                            })
                            .map(|server| server.live().refresh())
                    })
                    .ok()
                    .flatten();
                if let Some(refresh) = refresh {
                    let _ = refresh.await;
                }
            }
            if let Err(error) = answer {
                let _ = this.update(cx, |this, cx| {
                    if this.navigations == navigation
                        && this
                            .client
                            .server(owner.entry.server_id)
                            .and_then(|server| server.owner())
                            == Some(owner)
                    {
                        this.refuse(crate::text::flow_error(
                            &junto_client_core::flow::FlowError::from(&error),
                        ));
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn open_hidden_conversation(
        &mut self,
        server_id: ServerId,
        id: ConversationId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(server) = self.client.server(server_id) else {
            return false;
        };
        if !server
            .live()
            .view()
            .model
            .is_some_and(|model| model.hidden_conversations.contains(&id))
        {
            return false;
        }
        let Some(owner) = server.owner() else {
            return false;
        };
        let api = server.api_for(owner.account_id);
        self.end_jump();
        self.navigations += 1;
        let ask = Opening {
            owner,
            id,
            navigation: self.navigations,
            jump: None,
        };
        self.pending.opening_hidden = Some(ask);
        cx.spawn(async move |this, cx| {
            let answer = api.set_conversation_hidden(id, false).await;
            let mut refresh_failed = false;
            if answer.is_ok() {
                let refresh = this
                    .update(cx, |this, _| {
                        this.client
                            .server(server_id)
                            .filter(|server| {
                                this.pending
                                    .opening_hidden
                                    .is_some_and(|held| held.same_request(ask))
                                    && server.owner() == Some(owner)
                                    && server.live().view().model.is_some_and(|model| {
                                        model.hidden_conversations.contains(&id)
                                    })
                            })
                            .map(|server| server.live().refresh())
                    })
                    .ok()
                    .flatten();
                if let Some(refresh) = refresh {
                    refresh_failed = refresh.await.is_err();
                }
            }
            let _ = this.update(cx, |this, cx| {
                if !this
                    .pending
                    .opening_hidden
                    .is_some_and(|held| held.same_request(ask))
                    || this
                        .client
                        .server(server_id)
                        .and_then(|server| server.owner())
                        != Some(owner)
                {
                    return;
                }
                if refresh_failed && this.navigations == ask.navigation {
                    this.pending.opening_hidden = None;
                    this.refuse(crate::text::conversations::OPEN_FAILED.to_owned());
                }
                if let Err(error) = answer {
                    this.pending.opening_hidden = None;
                    if this.navigations == ask.navigation {
                        this.refuse(crate::text::flow_error(
                            &junto_client_core::flow::FlowError::from(&error),
                        ));
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        self.changed(cx);
        true
    }

    pub(super) fn settle_conversation_visibility(&mut self, snapshots: &[ServerSnapshot]) {
        for server in snapshots {
            if let Some(model) = &server.model
                && self
                    .selection
                    .conversations
                    .get(&server.id)
                    .is_some_and(|id| model.hidden_conversations.contains(id))
            {
                self.selection.conversations.remove(&server.id);
            }
        }
        let Some(ask) = self.pending.opening_hidden else {
            return;
        };
        if self.navigations != ask.navigation
            || self
                .client
                .server(ask.owner.entry.server_id)
                .and_then(|server| server.owner())
                != Some(ask.owner)
        {
            self.pending.opening_hidden = None;
            return;
        }
        let Some(model) = snapshots
            .iter()
            .find(|server| server.id == ask.owner.entry.server_id)
            .and_then(|server| server.model.as_ref())
        else {
            return;
        };
        if model.place(PlaceId::Direct(ask.id)).is_none() {
            self.pending.opening_hidden = None;
            return;
        }
        if !model.hidden_conversations.contains(&ask.id) {
            self.pending.opening_hidden = None;
            if let Some(jump) = ask.jump {
                self.place_jump(jump);
            } else {
                self.selection.server = Some(ask.owner.entry.server_id);
                self.selection
                    .conversations
                    .insert(ask.owner.entry.server_id, ask.id);
            }
        }
    }
}
