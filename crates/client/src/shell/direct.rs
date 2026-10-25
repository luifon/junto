//! A direct conversation outside its call: the button that starts a direct-message call, and
//! the outcome of the last call.
use super::Shell;
use crate::{
    assets::Icon,
    state::direct::RingOutcome,
    text,
    theme::u,
    ui::{Button, ButtonKind, Notice},
};
use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, div};
use junto_client_core::protocol::{
    call::CallPlace,
    ids::{AccountId, ServerId},
};

impl Shell {
    /// What a direct conversation shows above its messages while the main window does not
    /// draw its call: the button that starts a direct-message call, or the statement that
    /// the call window draws the call of the conversation with Return, and the outcome of
    /// the last call with the person. The call of the conversation is drawn as the call of
    /// a voice channel is (ADR 0735).
    pub(crate) fn render_direct_call(
        &self,
        server: ServerId,
        other: AccountId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let latest = state
            .direct_calls()
            .rings
            .iter()
            .filter(|((id, _), ring)| *id == server && ring.other == other)
            .max_by_key(|(_, ring)| ring.invitation.created_at)
            .map(|(_, ring)| ring);
        let history = history_label(
            state.direct_calls().finished.get(&(server, other)).copied(),
            latest.map(|ring| (ring.outcome, ring.incoming)),
        );
        let incoming = self.view.as_ref().is_some_and(|view| view.invitations.iter().any(|invited| invited.target.server_id == server && matches!(invited.target.place, CallPlace::Direct { low_account_id, high_account_id } if low_account_id == other || high_account_id == other)));
        let separate = state.is_call_detached()
            && state.media_calls().call.is_some_and(|call| {
                call.target.server_id == server
                    && matches!(call.target.place, CallPlace::Direct { low_account_id, high_account_id } if low_account_id == other || high_account_id == other)
            });
        let call = if separate {
            Notice::new(text::CALL_SEPARATE)
                .selector("call-in-separate-window")
                .action(self.return_call_button(cx))
                .into_any_element()
        } else if incoming {
            div().into_any_element()
        } else {
            Button::new("start-direct-call")
                .label(text::DIRECT_CALL)
                .icon(Icon::Phone)
                .kind(ButtonKind::Primary)
                .selector("start-direct-call")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.start_direct_call(server, other, cx))
                }))
                .into_any_element()
        };
        div()
            .flex_none()
            .p(u(12.0))
            .child(call)
            .child(
                crate::ui::notice_slot("direct-notice-area").children(
                    history.map(|line| Notice::new(line).selector("direct-call-history")),
                ),
            )
            .into_any_element()
    }
}

fn history_label(
    finished: Option<RingOutcome>,
    latest: Option<(RingOutcome, bool)>,
) -> Option<&'static str> {
    match finished.or_else(|| latest.map(|(outcome, _)| outcome)) {
        Some(RingOutcome::Missed)
            if finished.is_some() || latest.is_none_or(|(_, incoming)| !incoming) =>
        {
            Some("No answer")
        }
        Some(RingOutcome::Missed) => Some(text::MISSED_CALL),
        Some(RingOutcome::Declined) => Some(text::DIRECT_CALL_DECLINED),
        Some(RingOutcome::Cancelled) if latest.is_some_and(|(_, incoming)| incoming) => {
            Some(text::MISSED_CALL)
        }
        Some(RingOutcome::Cancelled) => Some(text::DIRECT_CALL_CANCELLED),
        Some(RingOutcome::Ended | RingOutcome::EndedElsewhere) => Some(text::DIRECT_CALL_ENDED),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_answer_uses_the_current_call_direction_even_after_an_older_incoming_call() {
        assert_eq!(
            history_label(Some(RingOutcome::Missed), Some((RingOutcome::Missed, true))),
            Some("No answer")
        );
        assert_eq!(
            history_label(None, Some((RingOutcome::Cancelled, true))),
            Some(text::MISSED_CALL)
        );
        assert_eq!(
            history_label(None, Some((RingOutcome::Cancelled, false))),
            Some(text::DIRECT_CALL_CANCELLED)
        );
    }
}
