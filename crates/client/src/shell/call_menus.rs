use gpui::{
    AnyElement, App, AppContext as _, Context, IntoElement, ParentElement, Styled, Window, div,
};
use gpui_base::slider::SliderEvent;

use super::{
    Layer, Shell,
    account_menus::{AccountSource, OpenAccountMenu},
    call::VolumeControl,
};
use crate::{
    text,
    theme::{theme, u},
    ui::{StyleExt, VolumeSlider},
};

impl Shell {
    pub(super) fn call_menu_current(&self, open: &OpenAccountMenu, cx: &App) -> bool {
        matches!(&self.layer, Layer::AccountMenu(current) if current.as_ref() == open)
            && open
                .people
                .first()
                .is_some_and(|person| self.account_menu_current(open.owner, person, cx))
    }

    fn call_volume_value(&self, open: &OpenAccountMenu) -> Option<u16> {
        let Some(AccountSource::CallOccupant { account_id, .. }) = &open.source else {
            return None;
        };
        if *account_id == open.owner.account_id {
            return None;
        }
        self.view
            .as_ref()?
            .lists()?
            .call
            .as_ref()?
            .people
            .iter()
            .find(|person| person.account_id == *account_id)
            .map(|person| person.volume)
    }

    pub(super) fn start_call_menu_volume(
        &mut self,
        open: &OpenAccountMenu,
        cx: &mut Context<Self>,
    ) {
        let Some(percent) = self.call_volume_value(open) else {
            return;
        };
        let context = open.clone();
        let state = cx.new(|_| VolumeSlider::state(percent));
        let changes = cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
            if !this.call_menu_current(&context, cx) {
                return;
            }
            let (value, release) = match event {
                SliderEvent::Change(value) => (value.end(), false),
                SliderEvent::Release(value) => (value.end(), true),
            };
            let percent = value.round().clamp(0.0, 200.0) as u16;
            let account = context.people[0].account_id;
            this.state.update(cx, |state, cx| {
                state.set_volume(context.owner.entry.server_id, account, percent, cx);
                if release {
                    state.store_volume(context.owner.entry.server_id, account, cx);
                }
            });
        });
        self.account_volume = Some(VolumeControl {
            state,
            focus: cx.focus_handle(),
            _changes: changes,
        });
    }

    pub(super) fn call_volume_row(
        &mut self,
        open: &OpenAccountMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let percent = self.call_volume_value(open)?;
        let control = self.account_volume.as_ref()?;
        if control.state.read(cx).value().end().round() as u16 != percent {
            control.state.update(cx, |state, cx| {
                state.set_value(f32::from(percent), window, cx)
            });
        }
        let theme = theme(cx);
        let name = &open.people[0].name;
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .min_w_0()
                .px(u(12.0))
                .py(u(8.0))
                .gap(u(5.0))
                .typeface(theme.types.small, &theme)
                .child(div().truncate().child(text::your_volume_for(name)))
                .child(
                    div().flex().w_full().child(
                        VolumeSlider::new(
                            "call-menu-volume",
                            &control.state,
                            &control.focus,
                            text::volume_for(name),
                        )
                        .selector("call-menu-volume"),
                    ),
                )
                .into_any_element(),
        )
    }
}
