use super::{Layer, Shell};
use crate::{
    assets::Icon,
    state::profile::ProfileRead,
    text,
    theme::{theme, u},
    ui::{Avatar, Button, ButtonKind, Chip, Dialog, Notice, OpenProfile, StyleExt, dialog_footer},
};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, WeakFocusHandle,
    Window, div,
};

#[derive(Default)]
pub(super) struct ProfileUi {
    pub(super) previous: Option<Layer>,
    pub(super) focus: Option<gpui::FocusHandle>,
    return_focus: Option<WeakFocusHandle>,
    roles: usize,
}
impl Shell {
    pub(crate) fn show_profile(
        &mut self,
        target: OpenProfile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let own = cx.entity_id();
        self.profile.roles = 100;
        if !self.state.update(cx, |state, cx| {
            state.open_profile(own, target.server, target.account, cx)
        }) {
            return;
        }
        self.profile_layer(window, cx);
    }
    #[cfg(feature = "test-support")]
    pub fn show_profile_fixture(
        &mut self,
        view: crate::state::profile::ProfileView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let own = cx.entity_id();
        self.state
            .update(cx, |state, cx| state.show_profile_fixture(own, view, cx));
        self.profile.roles = 100;
        self.profile_layer(window, cx);
    }
    fn profile_layer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.layer, Layer::Profile) {
            self.profile.previous = Some(std::mem::replace(&mut self.layer, Layer::Profile));
            self.profile.return_focus = window.focused(cx).map(|focus| focus.downgrade());
        }
        self.close_hover_menu(cx);
        self.profile
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .focus(window, cx);
        cx.notify();
    }
    pub(crate) fn close_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.layer = self.profile.previous.take().unwrap_or(Layer::None);
        let own = cx.entity_id();
        self.state
            .update(cx, |state, cx| state.close_profile(own, cx));
        if let Some(focus) = self
            .profile
            .return_focus
            .take()
            .and_then(|focus| focus.upgrade())
        {
            focus.focus(window, cx);
        } else if matches!(self.layer, Layer::QuickSwitcher) {
            self.open_switcher(window, cx);
        } else if matches!(self.layer, Layer::NewDirect) {
            self.contact_part
                .update(cx, |input, cx| input.focus(window, cx));
        } else if matches!(self.layer, Layer::None) {
            self.focus.focus(window, cx);
        } else {
            self.layer_focus.focus(window, cx);
        }
        cx.notify();
    }
    fn profile_contact(&mut self, call: bool, window: &mut Window, cx: &mut Context<Self>) {
        let own = cx.entity_id();
        if !self
            .state
            .update(cx, |state, cx| state.profile_message(own, call, cx))
        {
            return;
        }
        self.close_profile(window, cx);
        if matches!(self.layer, Layer::QuickSwitcher) {
            self.close_switcher(window, cx);
        }
        if !matches!(self.layer, Layer::None) {
            self.close_layer(window, cx);
        }
        if self.detached && !call {
            crate::notify::bring_to_front(None, cx);
        }
    }
    pub(crate) fn render_profile(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme(cx);
        let own = cx.entity_id();
        let Some(view) = self.state.read(cx).profile_view(own) else {
            return div().into_any_element();
        };
        let server = view.owner.entry.server_id;
        let faces = self.faces(server);
        let mut content = div().flex().flex_col().gap(u(10.));
        if let Some(name) = self
            .view
            .as_ref()
            .and_then(|view| view.servers.iter().find(|row| row.id == server))
            .map(|row| row.name.clone())
        {
            content = content.child(
                div()
                    .typeface(theme.types.small, &theme)
                    .ink(theme.colors.ink_2)
                    .child(name),
            );
        }
        match &view.read {
            ProfileRead::Reading => {
                content =
                    content.child(Notice::new(text::profile::READING).selector("profile-reading"));
            }
            ProfileRead::Unavailable => {
                content = content
                    .child(Notice::new(text::profile::UNAVAILABLE).selector("profile-unavailable"));
            }
            ProfileRead::Failed(reason) => {
                content = content.child(
                    Notice::error(format!("{} {reason}", text::profile::NOT_READ))
                        .selector("profile-failed")
                        .action(
                            Button::new("profile-retry")
                                .label(text::TRY_AGAIN)
                                .selector("profile-retry")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, cx| state.retry_profile(own, cx))
                                })),
                        ),
                );
            }
            ProfileRead::Shown(card) => {
                content = content.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(14.))
                        .child(
                            Avatar::new(view.account.as_uuid().as_u128(), 64.)
                                .image(faces.image(view.account))
                                .presence(faces.presence(view.account)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(
                                    div()
                                        .typeface(theme.types.title, &theme)
                                        .child(text::label(&card.profile.name)),
                                )
                                .child(
                                    div()
                                        .typeface(theme.types.small, &theme)
                                        .ink(theme.colors.ink_2)
                                        .child(text::presence::label(faces.presence(view.account))),
                                ),
                        ),
                );
                let total = card.server_roles.len()
                    + card
                        .workspaces
                        .iter()
                        .map(|workspace| workspace.roles.len())
                        .sum::<usize>();
                let mut remaining = self.profile.roles.max(100);
                if !card.server_roles.is_empty() {
                    content = content
                        .child(
                            div()
                                .typeface(theme.types.label, &theme)
                                .child(text::profile::SERVER_ROLES),
                        )
                        .child(div().flex().flex_wrap().gap(u(6.)).children(
                            card.server_roles.iter().take(remaining).map(|role| {
                                div()
                                    .debug_selector(move || format!("profile-role:{}", role.id))
                                    .child(Chip::new(text::label(&role.name)))
                            }),
                        ));
                    remaining = remaining.saturating_sub(card.server_roles.len());
                }
                for workspace in &card.workspaces {
                    if remaining == 0 {
                        break;
                    }
                    content = content
                        .child(
                            div()
                                .typeface(theme.types.label, &theme)
                                .child(text::label(&workspace.name)),
                        )
                        .child(div().flex().flex_wrap().gap(u(6.)).children(
                            workspace.roles.iter().take(remaining).map(|role| {
                                div()
                                    .debug_selector(move || format!("profile-role:{}", role.id))
                                    .child(Chip::new(text::label(&role.name)))
                            }),
                        ));
                    remaining = remaining.saturating_sub(workspace.roles.len());
                }
                if self.profile.roles.max(100) < total {
                    content = content.child(
                        Button::new("profile-more-roles")
                            .label(text::profile::MORE_ROLES)
                            .selector("profile-more-roles")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.profile.roles =
                                    this.profile.roles.max(100).saturating_add(100);
                                cx.notify();
                            })),
                    );
                }
                if card.server_roles.is_empty() && card.workspaces.is_empty() {
                    content = content.child(Notice::new(text::profile::NO_ROLES));
                }
                if view.account == view.owner.account_id {
                    content = content.child(Notice::new(text::profile::YOUR_ACCOUNT));
                }
            }
        }
        let dialog = Dialog::new(
            text::profile::TITLE,
            self.profile.focus.as_ref().unwrap_or(&self.layer_focus),
        )
        .selector("profile-card")
        .on_close(Self::handler(cx, |this, window, cx| {
            this.close_profile(window, cx)
        }))
        .child(content)
        .child(dialog_footer([
            Button::new("profile-message")
                .label(text::profile::MESSAGE)
                .icon(Icon::Chat)
                .kind(ButtonKind::Primary)
                .selector("profile-message")
                .disabled(!view.can_contact())
                .on_click(
                    cx.listener(|this, _, window, cx| this.profile_contact(false, window, cx)),
                )
                .into_any_element(),
            Button::new("profile-call")
                .label(text::profile::CALL)
                .icon(Icon::Phone)
                .selector("profile-call")
                .disabled(!view.can_contact())
                .on_click(cx.listener(|this, _, window, cx| this.profile_contact(true, window, cx)))
                .into_any_element(),
        ]));
        div()
            .id("profile-overlay")
            .absolute()
            .inset_0()
            .child(dialog)
            .into_any_element()
    }
}
