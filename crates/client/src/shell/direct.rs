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
    pub(crate) fn render_incoming_direct(
        &self,
        invited: &crate::state::view::Invited,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui::InteractiveElement as _;
        let accepted = invited.clone();
        let declined = invited.clone();
        div()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(u(12.0))
            .p(u(24.0))
            .debug_selector(|| "incoming-direct-call".to_owned())
            .child(super::people::avatar(
                &self.faces(invited.target.server_id),
                invited.caller,
                48.0,
            ))
            .child(format!(
                "{} is calling",
                invited.by.as_deref().unwrap_or(text::UNKNOWN_PERSON)
            ))
            .child(
                div()
                    .flex()
                    .gap(u(8.0))
                    .child(
                        Button::new(("direct-call-accept", index))
                            .label("Accept")
                            .icon(Icon::Phones)
                            .kind(ButtonKind::Primary)
                            .selector("direct-call-accept")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.accept_invitation(&accepted, cx))
                            })),
                    )
                    .child(
                        Button::new(("direct-call-decline", index))
                            .label(text::DECLINE)
                            .selector("direct-call-decline")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.decline_invitation(&declined, cx))
                            })),
                    ),
            )
            .into_any_element()
    }
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
            .map(|(_, ring)| ring);
        let history = history_label(
            state.direct_calls().finished.get(&(server, other)).copied(),
            latest.map(|ring| (ring.outcome, ring.incoming)),
            active.is_some(),
        );
        let incoming = self.view.as_ref().is_some_and(|view| view.invitations.iter().any(|invited| invited.target.server_id == server && matches!(invited.target.place, CallPlace::Direct { low_account_id, high_account_id } if low_account_id == other || high_account_id == other)));
        let faces = self.faces(server);
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
                    if state.voice().hide_people_without_video
                        && !self.cameras.is_empty()
                        && picture.is_none()
                    {
                        return None;
                    }
                    Some(
                        crate::ui::PersonTile::new(
                            format!("direct-camera:{id:?}"),
                            label.clone(),
                            account.as_uuid().as_u128(),
                            label,
                        )
                        .camera_view(active.and_then(|call| {
                            let running = if id == junto_media::engine::ShareId::Own {
                                matches!(
                                    call.camera,
                                    junto_media::camera::CameraState::Starting
                                        | junto_media::camera::CameraState::Running
                                )
                            } else {
                                call.cameras.contains(&account)
                            };
                            running.then(|| (state.calls().clone(), id))
                        }))
                        .picture(faces.image(account))
                        .camera_picture(picture),
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
            } else if call.phase == junto_media::calls::Phase::Entering {
                (text::CALL_ENTERING, "direct-call-entering")
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
        } else if incoming {
            div().into_any_element()
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

fn history_label(
    finished: Option<RingOutcome>,
    latest: Option<(RingOutcome, bool)>,
    active: bool,
) -> Option<&'static str> {
    if active {
        return None;
    }
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
            history_label(
                Some(RingOutcome::Missed),
                Some((RingOutcome::Missed, true)),
                false
            ),
            Some("No answer")
        );
        assert_eq!(
            history_label(None, Some((RingOutcome::Cancelled, true)), false),
            Some(text::MISSED_CALL)
        );
        assert_eq!(
            history_label(None, Some((RingOutcome::Cancelled, false)), false),
            Some(text::DIRECT_CALL_CANCELLED)
        );
        assert_eq!(
            history_label(None, Some((RingOutcome::Missed, true)), true),
            None
        );
    }
}
