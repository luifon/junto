//! The profile page of the account layer (ADR 0119, ADR 0781): the picture and the name
//! of the account on one server.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
};
use junto_client_core::profile;

use crate::shell::Shell;
use crate::shell::admin::{group_label, page_title, refusal, sentence};
use crate::shell::people::avatar;
use crate::state::account::{AccountPage, AccountView, AskKind, ProfilePage};
use crate::text;
use crate::theme::{theme, u};
use crate::ui::{Button, Field};

/// The side of the picture on the profile page, in design pixels.
const PICTURE_SIZE: f32 = 72.0;

impl Shell {
    /// The picture of the account of the layer: its image when the window holds it, and
    /// the generated avatar of the account otherwise (ADR 0783).
    fn own_picture(&self, view: &AccountView) -> impl IntoElement + use<> {
        avatar(&self.faces(view.server_id), view.account_id, PICTURE_SIZE).radius(4.0)
    }

    pub(crate) fn render_profile_page(
        &mut self,
        view: &AccountView,
        page: &ProfilePage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        // The page sends one request at a time: while one is on its way, no control of
        // the page sends another.
        let busy = view.busy();
        let held = match &page.picture {
            Some(picture) => {
                text::account::picture_held(picture.width, picture.height, picture.bytes)
            }
            None => text::account::NO_PICTURE.to_owned(),
        };
        let picture = div()
            .flex()
            .flex_none()
            .items_start()
            .gap(u(14.0))
            .child(
                div()
                    .flex_none()
                    .debug_selector(|| "account-picture".to_owned())
                    .child(self.own_picture(view)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        sentence(held, &theme).debug_selector(|| "account-picture-held".to_owned()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .flex_wrap()
                            .gap(u(8.0))
                            .mt(u(8.0))
                            .child(
                                Button::new("account-picture-choose")
                                    .icon(crate::assets::Icon::Picture)
                                    .label(if view.sending(AskKind::SetPicture) {
                                        text::account::PICTURE_SENDING
                                    } else {
                                        text::account::CHOOSE_PICTURE
                                    })
                                    .selector("account-picture-choose")
                                    .disabled(busy)
                                    .h(u(30.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.choose_account_picture(cx);
                                    })),
                            )
                            .child(
                                Button::new("account-picture-remove")
                                    .label(text::account::REMOVE_PICTURE)
                                    .selector("account-picture-remove")
                                    .disabled(busy || page.picture.is_none())
                                    .h(u(30.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.account.picture_refused = None;
                                        this.state.update(cx, |state, cx| {
                                            state.remove_account_picture(cx);
                                        });
                                    })),
                            ),
                    )
                    .child(sentence(text::account::PICTURE_BOUNDS, &theme).mt(u(6.0))),
            );
        let picture_refused = self
            .account
            .picture_refused
            .or(view.refused(AskKind::SetPicture))
            .or(view.refused(AskKind::RemovePicture));

        // The button says "Saved" while the field holds the name that the server holds.
        let typed = profile::account_name(&self.account.name.read(cx).value());
        let saved = typed.is_ok_and(|typed| typed == page.name);
        let sending_name = view.sending(AskKind::SaveName);
        let name = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .child(
                div().flex_1().min_w_0().child(
                    Field::new(&self.account.name, text::account::NAME_FIELD)
                        .selector("account-name-field"),
                ),
            )
            .child(
                Button::new("account-name-save")
                    .label(if sending_name {
                        text::account::SENDING
                    } else if saved {
                        text::SAVED
                    } else {
                        text::SAVE
                    })
                    .selector("account-name-save")
                    .disabled(saved || busy)
                    .h(u(32.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.submit_account_name(window, cx);
                    })),
            );

        div()
            .flex()
            .flex_col()
            .child(page_title(
                text::account::page_name(AccountPage::Profile),
                text::account::PROFILE_HELP,
                &theme,
            ))
            .child(group_label(text::account::PICTURE, &theme))
            .child(picture)
            .children(refusal(picture_refused, "account-picture-refused"))
            .child(group_label(text::account::NAME, &theme))
            .child(name)
            .child(sentence(text::account::NAME_HELP, &theme).mt(u(5.0)))
            .children(refusal(
                view.refused(AskKind::SaveName),
                "account-name-refused",
            ))
            .into_any_element()
    }
}
