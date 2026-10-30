use super::Shell;
use crate::{
    assets::Icon,
    state::view::CallPanel,
    text::call_pin as words,
    theme::{Theme, u},
    ui::{Button, ButtonKind, StyleExt},
};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, div,
};

impl Shell {
    pub(crate) fn render_pin_bar(
        &self,
        panel: &CallPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pin = self.state.read(cx).pinned_call()?;
        if panel.phase != junto_media::calls::Phase::Connected {
            return None;
        }
        let person = panel
            .people
            .iter()
            .find(|person| person.account_id == pin.account)?;
        let name = super::channels::occupant_name(person);
        let mut bar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .px(u(12.0))
            .py(u(6.0))
            .debug_selector(move || format!("call-person-pinned:{}", pin.account))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .typeface(theme.types.control, theme)
                    .ink(theme.colors.bone)
                    .child(words::pinned(&name)),
            );
        if panel.stage.is_some() {
            let target = pin.clone();
            bar = bar.child(
                Button::new(SharedString::from(format!(
                    "pinned-stage:{}:{}",
                    pin.seat.identity, pin.account
                )))
                .kind(ButtonKind::Call)
                .label(if pin.show_person {
                    words::SHOW_SHARES
                } else {
                    words::SHOW_PERSON
                })
                .selector("pinned-stage-toggle")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.draws_call(cx) {
                        return;
                    }
                    this.state.update(cx, |state, cx| {
                        state.show_pinned_person(
                            target.owner,
                            &target.seat,
                            target.account,
                            !target.show_person,
                            cx,
                        );
                    });
                })),
            );
        }
        Some(
            bar.child(
                Button::new(SharedString::from(format!(
                    "unpin-person:{}:{}",
                    pin.seat.identity, pin.account
                )))
                .kind(ButtonKind::Call)
                .icon(Icon::X)
                .label(words::UNPIN)
                .selector("unpin-call-person")
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !this.draws_call(cx) {
                        return;
                    }
                    this.focus.focus(window, cx);
                    this.state.update(cx, |state, cx| {
                        state.pin_call_person(pin.owner, &pin.seat, pin.account, false, cx);
                    });
                })),
            )
            .into_any_element(),
        )
    }
}
