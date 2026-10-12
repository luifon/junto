//! A person dragged from a voice channel uses the same actions as the keyboard menu.
use crate::state::person_drop::{PersonDrag, PersonDrop};
use crate::theme::{theme, u};
use crate::ui::StyleExt;
use gpui::{Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div};

pub(super) struct DragLabel(pub(super) SharedString);
impl Render for DragLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .px(u(10.0))
            .py(u(6.0))
            .rounded(u(theme.metrics.radius))
            .bg(gpui::Hsla::from(theme.colors.limestone))
            .typeface(theme.types.name, &theme)
            .ink(theme.colors.ink)
            .child(self.0.clone())
    }
}
impl super::Shell {
    pub(super) fn drop_person(
        &mut self,
        person: PersonDrag,
        destination: junto_client_core::protocol::ids::ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            match state.view().person_drop(person, destination) {
                Some(PersonDrop::Invite) => state.invite(person.account_id, cx),
                Some(PersonDrop::Move) => state.move_person(
                    (person.server_id, person.channel_id),
                    person.account_id,
                    destination,
                    cx,
                ),
                None => {}
            }
        });
    }
}
