//! The page of the server itself (ADR 0101): its logo, with the file dialog that sets it
//! and the question before its removal.

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, PathPromptOptions, Role,
    StatefulInteractiveElement, Styled, Window, div,
};

use super::{Mode, Question, group_label, page_title, refusal, sentence};
use crate::shell::Shell;
use crate::state::admin::view::AdminView;
use crate::state::admin::view::server::ServerSettingsPage;
use crate::state::admin::{AdminOpen, AdminPage, AskKind};
use crate::text;
use crate::text::admin::server as txt;
use crate::theme::{theme, u};
use crate::ui::{Avatar, Button};

/// The side of the logo on the page, in design pixels.
const LOGO_SIZE: f32 = 56.0;

impl Shell {
    /// Asks the person for a file, and sends it as the logo of the server of the page.
    pub(crate) fn choose_server_logo(&mut self, cx: &mut Context<Self>) {
        let Some(asked) = self.logo_owner(cx) else {
            return;
        };
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(file) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    // The dialog does not stop the window: the file is sent only while
                    // the page "Server" of the same owner is still open (ADR 0746).
                    if this.logo_owner(cx) == Some(asked) {
                        this.use_server_logo(file, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// The owner of the server whose page "Server" is open.
    fn logo_owner(&self, cx: &gpui::App) -> Option<junto_client_core::Owner> {
        let state = self.state.read(cx);
        match state.admin_open() {
            Some(AdminOpen::Server {
                server_id,
                page: AdminPage::Server,
            }) => state.owner(server_id),
            _ => None,
        }
    }

    /// Sends the file at this path as the logo of the server whose page "Server" is open,
    /// as the file dialog does. Nothing is sent when the person left that page.
    pub fn use_server_logo(&mut self, file: PathBuf, cx: &mut Context<Self>) {
        let Some(AdminOpen::Server {
            server_id,
            page: AdminPage::Server,
        }) = self.state.read(cx).admin_open()
        else {
            return;
        };
        self.state
            .update(cx, |state, cx| state.set_server_logo(server_id, file, cx));
    }

    pub(crate) fn render_server_settings_page(
        &mut self,
        view: &AdminView,
        page: &ServerSettingsPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let server_id = view.server_id;
        // The question is about a logo: it is put away when the server has none.
        if !page.has_logo && self.admin.mode == Some(Mode::RemoveLogo) {
            self.admin.mode = None;
        }
        // Only an image has a name for a screen reader and for a test. Without the image
        // the page states in a sentence what the server has.
        let logo: AnyElement = match (&page.image, page.has_logo) {
            (Some(image), _) => div()
                .id("logo-picture")
                .role(Role::Image)
                .aria_label(text::logo_of(&page.name))
                .debug_selector(|| "logo-picture".to_owned())
                .flex_none()
                .child(
                    Avatar::new(server_id.as_uuid().as_u128(), LOGO_SIZE)
                        .radius(4.0)
                        .image(Some(image.clone())),
                )
                .into_any_element(),
            (None, true) => sentence(txt::LOGO_NOT_SHOWN, &theme)
                .debug_selector(|| "logo-not-shown".to_owned())
                .into_any_element(),
            (None, false) => sentence(txt::NO_LOGO, &theme)
                .debug_selector(|| "logo-none".to_owned())
                .into_any_element(),
        };
        let sending = view.sending(AskKind::SetLogo);
        let removing = view.sending(AskKind::RemoveLogo);
        let held = (!page.may_change).then_some(txt::LOGO_NEEDS);
        let controls: AnyElement = match self.admin.mode {
            Some(Mode::RemoveLogo) => self
                .render_row_question(
                    Question {
                        sentence: txt::REMOVE_LOGO_QUESTION.into(),
                        confirm: text::REMOVE,
                        confirm_selector: "logo-remove-confirm",
                        cancel_selector: "logo-remove-cancel",
                        sending: removing,
                    },
                    &theme,
                    cx,
                    move |this, _, cx| {
                        this.admin_sent(AskKind::RemoveLogo);
                        this.state.update(cx, |state, cx| {
                            state.remove_server_logo(server_id, cx);
                        });
                    },
                )
                .into_any_element(),
            _ => {
                let busy = sending || removing || held.is_some();
                let choose = Button::new("logo-choose")
                    .label(if sending {
                        text::admin::SENDING
                    } else {
                        txt::CHOOSE_IMAGE
                    })
                    .selector("logo-choose")
                    .disabled(busy)
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, _, cx| this.choose_server_logo(cx)));
                let remove = page.has_logo.then(|| {
                    Button::new("logo-remove")
                        .label(txt::REMOVE_LOGO)
                        .selector("logo-remove")
                        .disabled(busy)
                        .h(u(30.0))
                        .px(u(12.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_admin_mode(Some(Mode::RemoveLogo), "", window, cx);
                        }))
                });
                div()
                    .flex()
                    .flex_wrap()
                    .gap(u(8.0))
                    .child(choose)
                    .children(remove)
                    .into_any_element()
            }
        };
        let refused = view
            .refused(AskKind::SetLogo)
            .or(view.refused(AskKind::RemoveLogo));
        div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::Server),
                txt::LOGO_HELP,
                &theme,
            ))
            .child(group_label(txt::LOGO, &theme))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .min_h(u(LOGO_SIZE))
                    .child(logo),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .mt(u(10.0))
                    .child(controls),
            )
            .children(held.map(|reason| sentence(reason, &theme).mt(u(5.0))))
            .children(refusal(refused, "logo-refused"))
            .into_any_element()
    }
}
