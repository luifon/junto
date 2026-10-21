//! The moves of channels that the person sent and that the state of the server does not
//! hold yet (ADR 0730, ADR 0761).

use junto_client_core::api::Receipt;
use junto_client_core::model::ServerModel;
use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::ChannelMove;

use crate::state::receipts::Awaited;

/// The page of the channels shows a move from its send: the order changes at the action of
/// the person, a press or a drop, and not when the answer or the events arrive, at a time
/// that the person does not choose. A move waits until the state holds the events of its
/// answer. A move that the server refused stays where it is drawn, with the refusal, until
/// the person acts again on the layer; then the page shows the state.
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
    /// The channel that the person moved, at whose row a refusal is stated.
    channel_id: ChannelId,
    /// The moves that change a channel. A later move of the same channel that the state
    /// holds takes the move of that channel out of an earlier request.
    moves: Vec<ChannelMove>,
    /// The receipt of the answer. Absent while the request is on its way.
    answered: Option<Awaited>,
    /// Why the server refused the request.
    refused: Option<String>,
}

impl ChannelMoves {
    /// The moves of the channels of one workspace that are drawn, the oldest first.
    pub fn of(&self, server_id: ServerId, workspace_id: WorkspaceId) -> Vec<ChannelMove> {
        self.waiting
            .iter()
            .filter(|held| held.server_id == server_id && held.workspace_id == workspace_id)
            .flat_map(|held| held.moves.iter().copied())
            .collect()
    }

    /// The newest refused move of a workspace that is drawn: the moved channel, and why
    /// the server refused it.
    pub fn refusal(
        &self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
    ) -> Option<(ChannelId, String)> {
        self.waiting
            .iter()
            .rev()
            .filter(|held| held.server_id == server_id && held.workspace_id == workspace_id)
            .find_map(|held| Some((held.channel_id, held.refused.clone()?)))
    }

    /// Notes the moves of one request, and returns its number. A move that only checks the
    /// place of a channel changes nothing and is not drawn.
    pub(crate) fn begin(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        moves: &[ChannelMove],
    ) -> u64 {
        self.begun += 1;
        self.waiting.push(Waiting {
            number: self.begun,
            server_id,
            workspace_id,
            channel_id,
            moves: moves
                .iter()
                .filter(|held| held.to != held.from)
                .copied()
                .collect(),
            answered: None,
            refused: None,
        });
        self.begun
    }

    /// The server refused the request: its moves stay drawn with the reason until the
    /// person acts again.
    pub(crate) fn refused(&mut self, number: u64, reason: String) {
        for waiting in self.waiting.iter_mut().filter(|held| held.number == number) {
            waiting.refused = Some(reason.clone());
        }
    }

    /// The request was dropped without an answer, with its server: nothing is drawn of it.
    pub(crate) fn dropped(&mut self, number: u64) {
        self.waiting.retain(|held| held.number != number);
    }

    /// The person acted on the layer: the refused moves are no longer drawn.
    pub(crate) fn release(&mut self) {
        self.waiting.retain(|held| held.refused.is_none());
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
    /// nothing. When the state holds a request, the moves of the same channels in the
    /// earlier requests are taken out: the state holds a later place of those channels,
    /// which an earlier answer that comes late must not draw over.
    pub(crate) fn settle<'a>(
        &mut self,
        state: impl Fn(ServerId, WorkspaceId) -> Option<&'a ServerModel>,
    ) {
        let mut held: Vec<(u64, ServerId, WorkspaceId, Vec<ChannelId>)> = Vec::new();
        self.waiting.retain(|waiting| {
            let keeps = match state(waiting.server_id, waiting.workspace_id) {
                None => false,
                Some(model) => waiting
                    .answered
                    .is_none_or(|answered| !answered.shown_by(model)),
            };
            if !keeps {
                held.push((
                    waiting.number,
                    waiting.server_id,
                    waiting.workspace_id,
                    waiting.moves.iter().map(|moved| moved.channel_id).collect(),
                ));
            }
            keeps
        });
        for (number, server_id, workspace_id, channels) in held {
            for earlier in self.waiting.iter_mut().filter(|earlier| {
                earlier.number < number
                    && earlier.server_id == server_id
                    && earlier.workspace_id == workspace_id
            }) {
                earlier
                    .moves
                    .retain(|moved| !channels.contains(&moved.channel_id));
            }
        }
    }

    /// Another account uses the server, or none: no move of the account before waits.
    pub(crate) fn forget(&mut self) {
        self.waiting.clear();
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::structure::ChannelPlace;

    use super::*;

    fn place(position: i32) -> ChannelPlace {
        ChannelPlace {
            category_id: None,
            position,
        }
    }

    fn moved(channel_id: ChannelId, from: i32, to: i32) -> ChannelMove {
        ChannelMove {
            channel_id,
            from: place(from),
            to: place(to),
        }
    }

    #[test]
    fn a_later_move_that_the_state_holds_takes_the_same_channel_out_of_earlier_moves() {
        let model = crate::state::fixtures::model_at(40);
        let (server_id, workspace_id) = (ServerId::generate(), WorkspaceId::generate());
        let (x, y) = (ChannelId::generate(), ChannelId::generate());
        let at = |position| {
            Receipt::At(Cursor {
                epoch: model.cursor.epoch,
                position,
            })
        };
        let state = |_: ServerId, _: WorkspaceId| Some(&model);
        let mut moves = ChannelMoves::default();
        // The first request moves "x" and "y"; the second moves "x" again from where the
        // first put it. A check of a neighbour is not drawn.
        let first = moves.begin(
            server_id,
            workspace_id,
            x,
            &[moved(x, 0, 5), moved(y, 5, 4)],
        );
        let second = moves.begin(
            server_id,
            workspace_id,
            x,
            &[moved(x, 5, 9), moved(y, 4, 4)],
        );
        assert_eq!(
            moves.of(server_id, workspace_id),
            [moved(x, 0, 5), moved(y, 5, 4), moved(x, 5, 9)]
        );

        // The second answer arrives first, and the state holds it; the first answer is
        // late. The first move of "x" is not drawn over the later place of "x"; its move of
        // "y" waits for its own answer.
        moves.answered(second, at(40));
        moves.settle(state);
        assert_eq!(moves.of(server_id, workspace_id), [moved(y, 5, 4)]);
        assert!(moves.waits_for(first) && !moves.waits_for(second));
        moves.answered(first, at(39));
        moves.settle(state);
        assert!(moves.of(server_id, workspace_id).is_empty());
    }

    #[test]
    fn a_refused_move_stays_drawn_with_its_reason_until_the_person_acts() {
        let model = crate::state::fixtures::model_at(40);
        let (server_id, workspace_id) = (ServerId::generate(), WorkspaceId::generate());
        let x = ChannelId::generate();
        let mut moves = ChannelMoves::default();
        let number = moves.begin(server_id, workspace_id, x, &[moved(x, 0, 5)]);
        moves.refused(number, "No longer there.".to_owned());
        moves.settle(|_, _| Some(&model));
        assert_eq!(moves.of(server_id, workspace_id), [moved(x, 0, 5)]);
        assert_eq!(
            moves.refusal(server_id, workspace_id),
            Some((x, "No longer there.".to_owned()))
        );
        moves.release();
        assert!(moves.of(server_id, workspace_id).is_empty());
        assert_eq!(moves.refusal(server_id, workspace_id), None);
    }
}
