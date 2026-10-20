//! The moves of channels that the person sent and that the state of the server does not
//! hold yet (ADR 0730, ADR 0761).

use junto_client_core::api::Receipt;
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};
use junto_client_core::protocol::structure::ChannelMove;

use crate::state::receipts::Awaited;

/// The page of the channels shows a move from its send: the order changes at the action of
/// the person, a press or a drop, and not when the answer or the events arrive, at a time
/// that the person does not choose. A move waits until the state holds the events of its
/// answer. A move that the server refused waits no more: the page shows the state again.
#[derive(Debug, Default)]
pub struct ChannelMoves {
    waiting: Vec<Waiting>,
    /// The number of moves that were sent. A move is named by its number, so the answer to
    /// an earlier request does not settle a later one.
    begun: u64,
}

#[derive(Debug)]
struct Waiting {
    number: u64,
    server_id: ServerId,
    workspace_id: WorkspaceId,
    moves: Vec<ChannelMove>,
    /// The receipt of the answer. Absent while the request is on its way.
    answered: Option<Awaited>,
}

impl ChannelMoves {
    /// The moves of the channels of one workspace that wait, the oldest first.
    pub fn of(&self, server_id: ServerId, workspace_id: WorkspaceId) -> Vec<ChannelMove> {
        self.waiting
            .iter()
            .filter(|held| held.server_id == server_id && held.workspace_id == workspace_id)
            .flat_map(|held| held.moves.iter().copied())
            .collect()
    }

    /// Notes the moves of one request, and returns its number.
    pub(crate) fn begin(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        moves: Vec<ChannelMove>,
    ) -> u64 {
        self.begun += 1;
        self.waiting.push(Waiting {
            number: self.begun,
            server_id,
            workspace_id,
            moves,
            answered: None,
        });
        self.begun
    }

    /// The request failed: its moves wait no more.
    pub(crate) fn failed(&mut self, number: u64) {
        self.waiting.retain(|held| held.number != number);
    }

    /// The server did the moves of a request, with this receipt.
    pub(crate) fn answered(&mut self, number: u64, receipt: Receipt) {
        for waiting in self.waiting.iter_mut().filter(|held| held.number == number) {
            waiting.answered = Some(Awaited::new(receipt));
        }
    }

    /// True while the moves with this number wait for the state.
    pub(crate) fn waits_for(&self, number: u64) -> bool {
        self.waiting.iter().any(|held| held.number == number)
    }

    /// A snapshot that was read after the answer replaced the state.
    pub(crate) fn refreshed(&mut self, number: u64) {
        for waiting in self.waiting.iter_mut().filter(|held| held.number == number) {
            if let Some(answered) = &mut waiting.answered {
                answered.refreshed = true;
            }
        }
    }

    /// Drops the moves that the state holds. `state` gives the state of a server for the
    /// account that asked, when it holds the workspace. A move waits while the server has
    /// not answered; a workspace or a server that the state does not hold waits for
    /// nothing.
    pub(crate) fn settle<'a>(
        &mut self,
        state: impl Fn(ServerId, WorkspaceId) -> Option<&'a ServerModel>,
    ) {
        self.waiting.retain(
            |waiting| match state(waiting.server_id, waiting.workspace_id) {
                None => false,
                Some(model) => waiting
                    .answered
                    .is_none_or(|answered| !answered.shown_by(model)),
            },
        );
    }

    /// Another account uses the server, or none: no move of the account before waits.
    pub(crate) fn forget(&mut self) {
        self.waiting.clear();
    }
}
