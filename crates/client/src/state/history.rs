//! The places that the window showed, for the keys Back and Forward (ADR 0777): a server with
//! its workspace and its channel, or a direct conversation.

use gpui::Context;
use junto_client_core::protocol::ids::{ChannelId, ConversationId, ServerId, WorkspaceId};

use super::AppState;
use super::view::Shown;

/// The most places that each direction keeps.
const MOST_PLACES: usize = 50;

/// A place that the window showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Visited {
    server: ServerId,
    workspace: Option<WorkspaceId>,
    channel: Option<ChannelId>,
    conversation: Option<ConversationId>,
}

impl Visited {
    fn of(shown: &Shown) -> Option<Self> {
        Some(Self {
            server: shown.server?,
            workspace: shown.workspace,
            channel: shown.channel,
            conversation: shown.conversation,
        })
    }
}

#[derive(Debug, Default)]
pub(super) struct History {
    back: Vec<Visited>,
    forward: Vec<Visited>,
    /// A step of Back or Forward is shown now: the change of place that follows is not a
    /// new visit.
    stepping: bool,
}

impl AppState {
    /// The place that the window shows changed from `before` to `after`: `before` is a
    /// visit that Back returns to, and Forward forgets what it held.
    pub(super) fn follow_history(&mut self, before: &Shown, after: &Shown) {
        let (Some(before), Some(after)) = (Visited::of(before), Visited::of(after)) else {
            return;
        };
        if before == after {
            return;
        }
        if std::mem::take(&mut self.history.stepping) {
            return;
        }
        self.history.back.push(before);
        if self.history.back.len() > MOST_PLACES {
            self.history.back.remove(0);
        }
        self.history.forward.clear();
    }

    /// Shows the place before the current one. False when there is none.
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        self.step_history(true, cx)
    }

    /// Shows the place that Back left. False when there is none.
    pub fn go_forward(&mut self, cx: &mut Context<Self>) -> bool {
        self.step_history(false, cx)
    }

    fn step_history(&mut self, back: bool, cx: &mut Context<Self>) -> bool {
        let (from, to) = if back {
            (&mut self.history.back, &mut self.history.forward)
        } else {
            (&mut self.history.forward, &mut self.history.back)
        };
        let Some(place) = from.pop() else {
            return false;
        };
        if let Some(current) = Visited::of(&self.shown) {
            to.push(current);
        }
        self.history.stepping = true;
        self.end_jump();
        self.navigations += 1;
        self.selection.server = Some(place.server);
        if let Some(workspace) = place.workspace {
            self.selection.workspaces.insert(place.server, workspace);
            if let Some(channel) = place.channel {
                self.selection
                    .channels
                    .insert((place.server, workspace), channel);
            }
        }
        match place.conversation {
            Some(conversation) => {
                self.selection
                    .conversations
                    .insert(place.server, conversation);
            }
            None => {
                self.selection.conversations.remove(&place.server);
            }
        }
        self.changed(cx);
        true
    }
}
