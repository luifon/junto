//! The page of the server itself: its logo (ADR 0101), with the file dialog that sets it
//! and the question before its removal, and the limits of the files of messages
//! (ADR 0408).

use std::path::PathBuf;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, PathPromptOptions, Role,
    StatefulInteractiveElement, Styled, Window, div,
};

use junto_client_core::model::AdminPart;
use junto_client_core::protocol::message::AttachmentLimits;

use super::{
    Form, FormField, Mode, Question, group_label, line, line_text, page_title, refusal, row_action,
    sentence,
};
use crate::shell::Shell;
use crate::state::admin::view::server::ServerSettingsPage;
use crate::state::admin::view::{AdminView, Listed, Scope, ServerPage};
use crate::state::admin::{AdminOpen, AdminPage, AskKind};
use crate::text;
use crate::text::admin::server as txt;
use crate::theme::{Theme, theme, u};
use crate::ui::{Avatar, Button};

/// The side of the logo on the page, in design pixels.
const LOGO_SIZE: f32 = 56.0;

/// One limit of the files of messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The largest file, typed in megabytes.
    FileSize,
    FilesPerMessage,
}

const BYTES_PER_MEGABYTE: u64 = 1_000_000;
/// The largest file that the server takes, in whole megabytes: the server takes at most
/// 1,073,741,824 bytes.
const MOST_MEGABYTES: u64 = 1_073;
const _: () = assert!(MOST_MEGABYTES * BYTES_PER_MEGABYTE <= 1_073_741_824);
const MOST_FILES: u32 = 100;

/// The limits after the person typed `typed` for one of them, when it is a number in its
/// range. The other limit stays as the page shows it.
fn typed_limits(
    typed: &str,
    which: Limit,
    shown: &AttachmentLimits,
) -> Result<AttachmentLimits, &'static str> {
    match which {
        Limit::FileSize => match typed.parse::<u64>() {
            Ok(megabytes) if (1..=MOST_MEGABYTES).contains(&megabytes) => Ok(AttachmentLimits {
                bytes_per_file: megabytes * BYTES_PER_MEGABYTE,
                ..shown.clone()
            }),
            _ => Err(txt::LARGEST_FILE_RANGE),
        },
        Limit::FilesPerMessage => match typed.parse::<u32>() {
            Ok(files) if (1..=MOST_FILES).contains(&files) => Ok(AttachmentLimits {
                files_per_message: files,
                ..shown.clone()
            }),
            _ => Err(txt::FILES_RANGE),
        },
    }
}

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

    /// Sends the limit that the open row of the page "Server" holds.
    pub(crate) fn submit_limit(&mut self, view: &AdminView, cx: &mut Context<Self>) {
        let Some(Mode::Limit(which)) = self.admin.mode else {
            return;
        };
        let Scope::Server {
            page: ServerPage::Server(page),
            ..
        } = &view.scope
        else {
            return;
        };
        let Listed::Loaded(shown) = &page.limits else {
            return;
        };
        match typed_limits(&self.admin_value(cx), which, shown) {
            Ok(limits) => {
                let server_id = view.server_id;
                self.admin_sent(AskKind::AttachmentLimits);
                self.state.update(cx, |state, cx| {
                    state.set_attachment_limits(server_id, limits, cx);
                });
            }
            Err(range) => self.admin.invalid = Some(range),
        }
        cx.notify();
    }

    /// One limit of the files of messages: its name, its value, and the form that changes
    /// it.
    fn render_limit(
        &mut self,
        view: &AdminView,
        which: Limit,
        limits: &AttachmentLimits,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, value, field, range, selector) = match which {
            Limit::FileSize => (
                txt::LARGEST_FILE,
                text::file_size(limits.bytes_per_file),
                txt::MEGABYTES_FIELD,
                txt::LARGEST_FILE_RANGE,
                "limit-file-size",
            ),
            Limit::FilesPerMessage => (
                txt::FILES_PER_MESSAGE,
                txt::files(limits.files_per_message),
                txt::FILES_FIELD,
                txt::FILES_RANGE,
                "limit-files",
            ),
        };
        let sending = view.sending(AskKind::AttachmentLimits);
        let open = self.admin.mode == Some(Mode::Limit(which));
        let content: AnyElement = if open {
            self.render_form(
                Form {
                    field: FormField::Value,
                    label: field,
                    field_selector: "limit-field",
                    send: text::SAVE,
                    sending,
                    held: None,
                    help: Some(range),
                    extra: Vec::new(),
                },
                theme,
                cx,
            )
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(u(8.0))
                .child(
                    line_text(title, Some(value.into()), theme)
                        .debug_selector(move || format!("{selector}-value")),
                )
                .child(row_action(
                    selector,
                    text::admin::CHANGE,
                    selector.to_owned(),
                    sending.then_some(text::admin::SENDING),
                    cx.listener(move |this, _, window, cx| {
                        this.set_admin_mode(Some(Mode::Limit(which)), "", window, cx);
                    }),
                ))
                .into_any_element()
        };
        // The refusal belongs to the row whose form asked: the form stays open with it.
        let refused = view.refused(AskKind::AttachmentLimits).filter(|_| open);
        // Each limit has an element id of its own, so the places of their refusals keep
        // their own heights (ADR 0730).
        div()
            .id(selector)
            .flex()
            .flex_col()
            .flex_none()
            .child(self.mode_row(&[Mode::Limit(which)], line(theme).child(content)))
            .children(refusal(refused, "limit-refused"))
            .into_any_element()
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
        // The question is about a logo: it is put away when the server has none, and its
        // row keeps its height, as after an answer (ADR 0730).
        if !page.has_logo && self.admin.mode == Some(Mode::RemoveLogo) {
            self.admin.mode = None;
            self.admin.space.keep(Mode::RemoveLogo, text::admin::done::NO_LOGO);
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
                self.mode_row(
                    &[Mode::RemoveLogo],
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .mt(u(10.0))
                        .child(controls),
                ),
            )
            .children(held.map(|reason| sentence(reason, &theme).mt(u(5.0))))
            .children(refusal(refused, "logo-refused"))
            .child(group_label(txt::ATTACHMENTS, &theme))
            .child(sentence(txt::ATTACHMENTS_HELP, &theme).mb(u(4.0)))
            .child(match &page.limits {
                Listed::Loading => {
                    self.render_reading(server_id, AdminPart::AttachmentLimits, None, &theme, cx)
                }
                Listed::Failed(failure) => self.render_reading(
                    server_id,
                    AdminPart::AttachmentLimits,
                    Some(*failure),
                    &theme,
                    cx,
                ),
                Listed::Loaded(limits) => div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(self.render_limit(view, Limit::FileSize, limits, &theme, cx))
                    .child(self.render_limit(view, Limit::FilesPerMessage, limits, &theme, cx))
                    .into_any_element(),
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_limit_replaces_one_limit_within_the_range_of_the_server() {
        let shown = AttachmentLimits {
            bytes_per_file: 100_000_000,
            files_per_message: 10,
        };
        assert_eq!(
            typed_limits("25", Limit::FileSize, &shown),
            Ok(AttachmentLimits {
                bytes_per_file: 25_000_000,
                files_per_message: 10,
            })
        );
        assert_eq!(
            typed_limits("1073", Limit::FileSize, &shown).map(|held| held.bytes_per_file),
            Ok(1_073_000_000)
        );
        assert_eq!(
            typed_limits("4", Limit::FilesPerMessage, &shown),
            Ok(AttachmentLimits {
                bytes_per_file: 100_000_000,
                files_per_message: 4,
            })
        );
        for typed in ["0", "1074", "-3", "2.5", ""] {
            assert_eq!(
                typed_limits(typed, Limit::FileSize, &shown),
                Err(txt::LARGEST_FILE_RANGE)
            );
        }
        for typed in ["0", "101", "ten"] {
            assert_eq!(
                typed_limits(typed, Limit::FilesPerMessage, &shown),
                Err(txt::FILES_RANGE)
            );
        }
    }
}
