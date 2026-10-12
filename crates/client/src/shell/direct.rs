//! Direct calls keep the conversation available beneath their call state.
use super::Shell;
use crate::{
    assets::Icon,
    state::direct::RingOutcome,
    text,
    theme::u,
    ui::{Button, ButtonKind, Notice},
};
use gpui::{
    AnyElement, Context, IntoElement, ObjectFit, ParentElement, Styled, StyledImage, div, img,
};
use junto_client_core::protocol::{
    call::CallPlace,
    ids::{AccountId, ServerId},
};

impl Shell {
    pub(crate) fn render_direct_call(
        &self,
        server: ServerId,
        other: AccountId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let calls = state.media_calls();
        let active = calls.call.as_ref().filter(|call| call.target.server_id == server && matches!(call.target.place, CallPlace::Direct { low_account_id: low, high_account_id: high } if low == other || high == other));
        let joined = state.direct_connected(server, other);
        let latest = state
            .direct_calls()
            .rings
            .iter()
            .filter(|((id, _), ring)| *id == server && ring.other == other)
            .max_by_key(|(_, ring)| ring.invitation.created_at)
            .map(|(_, ring)| ring.outcome);
        let history = match latest {
            Some(RingOutcome::Missed) => Some(text::MISSED_CALL),
            Some(RingOutcome::Declined) => Some(text::DIRECT_CALL_DECLINED),
            Some(RingOutcome::Cancelled) => Some(text::DIRECT_CALL_CANCELLED),
            Some(RingOutcome::Ended) => Some(text::DIRECT_CALL_ENDED),
            _ => None,
        };
        let pictures = active.map(|call| {
            let CallPlace::Direct {
                low_account_id,
                high_account_id,
            } = call.target.place
            else {
                return div();
            };
            let own = if low_account_id == other {
                high_account_id
            } else {
                low_account_id
            };
            let name = self
                .view
                .as_ref()
                .and_then(|view| view.workspace())
                .and_then(|workspace| {
                    workspace
                        .directs
                        .iter()
                        .find(|direct| direct.account_id == other)
                })
                .and_then(|direct| direct.name.clone())
                .unwrap_or_else(|| text::DIRECT_CALL.to_owned());
            div().flex().flex_wrap().gap(u(8.0)).children(
                [
                    (junto_media::engine::ShareId::Own, own, text::YOU.to_owned()),
                    (junto_media::engine::ShareId::Of(other), other, name),
                ]
                .into_iter()
                .filter_map(|(id, account, label)| {
                    let picture = self.cameras.get(&id).map(|(_, image)| image.clone());
                    if state.voice().hide_people_without_video && picture.is_none() {
                        return None;
                    }
                    Some(
                        crate::ui::PersonTile::new(
                            format!("direct-camera:{id:?}"),
                            label.clone(),
                            account.as_uuid().as_u128(),
                            label,
                        )
                        .picture(picture),
                    )
                }),
            )
        });
        let share = active.and(self.pictures.main.as_ref()).map(|(_, image)| {
            img(image.clone())
                .w_full()
                .h(u(240.0))
                .object_fit(ObjectFit::Contain)
        });
        let separate = active.is_some() && state.is_call_detached() && !self.detached;
        let call = if let Some(call) = active {
            let (label, selector) = if call.phase == junto_media::calls::Phase::Reconnecting {
                (text::CALL_RECONNECTING, "direct-call-reconnecting")
            } else if joined {
                (text::DIRECT_CALL, "active-direct-call")
            } else {
                (text::CALLING, "outgoing-direct-call")
            };
            Notice::new(label)
                .selector(selector)
                .action(
                    Button::new("direct-leave")
                        .label(text::LEAVE_CALL)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.leave_call(cx))
                        })),
                )
                .into_any_element()
        } else {
            Button::new("start-direct-call")
                .label(text::DIRECT_CALL)
                .icon(Icon::Phones)
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
            .children(if separate {
                Some(
                    Notice::new(text::CALL_SEPARATE)
                        .selector("call-in-separate-window")
                        .action(self.return_call_button(cx))
                        .into_any_element(),
                )
            } else {
                None
            })
            .children(if separate { None } else { share })
            .children(if separate { None } else { pictures })
            .children(history.map(|line| Notice::new(line).selector("direct-call-history")))
            .into_any_element()
    }
}
