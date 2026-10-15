//! A change that the server did and that the state of the server does not hold yet
//! (ADR 0214). The window shows what the server took until the state holds the events of
//! the change: the stream of the server has reached the position that the answer stated,
//! or the state was replaced by a snapshot that was read after the answer.

use std::time::Duration;

use gpui::Context;
use junto_client_core::api::Receipt;
use junto_client_core::live::LoadError;
use junto_client_core::model::{Followed, ServerModel};
use junto_client_core::protocol::ids::ServerId;

use super::AppState;

/// The receipt of the answer to one change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Awaited {
    pub receipt: Receipt,
    /// A snapshot that was asked for after the answer has replaced the state.
    pub refreshed: bool,
}

impl Awaited {
    pub const fn new(receipt: Receipt) -> Self {
        Self {
            receipt,
            refreshed: false,
        }
    }

    /// True when the state holds the change.
    pub fn shown_by(&self, model: &ServerModel) -> bool {
        match model.followed(self.receipt) {
            Followed::Yes => true,
            Followed::NotYet => false,
            Followed::Unknown => self.refreshed,
        }
    }
}

/// The first wait before a snapshot that failed is asked for again, and the longest one.
const FIRST_WAIT: Duration = Duration::from_secs(2);
const LONGEST_WAIT: Duration = Duration::from_secs(64);

impl AppState {
    /// After an answer whose receipt the state cannot place: the answer stated no
    /// position, or one of another epoch. A snapshot that is asked for now holds the
    /// change (ADR 0214); `done` runs when such a snapshot replaced the state, or when the
    /// server has no event stream any more. A snapshot that failed is asked for again,
    /// with waits that double, while `waits` says that the change still waits.
    pub(crate) fn refresh_for(
        &mut self,
        server_id: ServerId,
        model: &ServerModel,
        awaited: Awaited,
        waits: impl Fn(&Self) -> bool + 'static,
        done: impl FnOnce(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) {
        if model.followed(awaited.receipt) != Followed::Unknown {
            return;
        }
        cx.spawn(async move |this, cx| {
            let mut wait = FIRST_WAIT;
            loop {
                let refreshing = this
                    .update(cx, |this, _| {
                        this.client
                            .server(server_id)
                            .filter(|_| waits(this))
                            .map(|server| server.live().refresh())
                    })
                    .ok()
                    .flatten();
                let Some(refreshing) = refreshing else {
                    return;
                };
                match refreshing.await {
                    Ok(_) | Err(LoadError::Off) => break,
                    Err(_) => {
                        cx.background_executor().timer(wait).await;
                        wait = (wait * 2).min(LONGEST_WAIT);
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                done(this);
                this.changed(cx);
            });
        })
        .detach();
    }
}
