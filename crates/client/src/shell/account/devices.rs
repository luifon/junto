//! The devices page of the account layer (ADR 0112, ADR 0781): the devices of the account
//! with a new name and the removal, and the approval of a new device by its link code.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::DeviceId;

use super::Mode;
use crate::assets::Icon;
use crate::shell::Shell;
use crate::shell::admin::{group_label, line, named, page_title, refusal, row_action, sentence};
use crate::state::account::{
    AccountPage, AccountView, AskKind, DeviceLine, DeviceList, DevicesPage, LinkView, PageView,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, ButtonKind, Chip, Field, FieldKind, Notice, StyleExt};

/// A number that tells the rows of the devices apart as elements.
fn row_number(device_id: DeviceId) -> u64 {
    device_id.as_uuid().as_u128() as u64
}

/// The name of a device in a line of its own that is cut at the width of the row: the
/// name is text of a server.
fn device_title(row: &DeviceLine, theme: &Theme) -> gpui::Div {
    let name: SharedString = if row.name.is_empty() {
        text::account::UNNAMED_DEVICE.into()
    } else {
        row.name.clone().into()
    };
    div()
        .flex()
        .items_center()
        .gap(u(8.0))
        .child(
            div()
                .min_w_0()
                .typeface(theme.types.control_strong, theme)
                .ink(theme.colors.ink)
                .truncate()
                .child(name),
        )
        .when(row.this_device, |title| {
            title.child(Chip::new(text::account::THIS_DEVICE))
        })
}

/// The name of a device, and under it how its key came to the account and when it was
/// used. Each line is cut at the width of the row.
fn device_text(row: &DeviceLine, theme: &Theme) -> gpui::Div {
    let small = |text: String| {
        div()
            .typeface(theme.types.small, theme)
            .ink(theme.colors.ink_2)
            .truncate()
            .child(text)
    };
    div()
        .flex_1()
        .min_w_0()
        .child(device_title(row, theme))
        .child(small(format!(
            "{} · {}",
            text::account::origin(row.origin),
            text::account::added_on(row.added)
        )))
        .child(small(text::account::last_login(row.last_login)))
}

/// A key as a person compares it: fixed-width type, in its groups.
fn fingerprint(
    label: &'static str,
    text: &str,
    selector: &'static str,
    theme: &Theme,
) -> gpui::Div {
    div()
        .flex_none()
        .child(
            div()
                .typeface(theme.types.label, theme)
                .ink(theme.colors.bronze)
                .child(label),
        )
        .child(
            div()
                .id(selector)
                .role(Role::Label)
                .aria_label(SharedString::from(format!("{label}: {text}")))
                .debug_selector(move || selector.to_owned())
                .mt(u(2.0))
                .typeface(theme.types.mono_code, theme)
                .ink(theme.colors.ink)
                .child(text.to_owned()),
        )
}

impl Shell {
    /// Puts text into the field for the new name of a device, as typing does.
    pub fn type_device_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.account
            .device_name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        cx.notify();
    }

    /// Sends the new name of the device whose row shows the form. A form whose request
    /// is on its way is not sent a second time, and nothing is sent while another request
    /// of the page is on its way.
    pub fn submit_device_name(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(Mode::RenameDevice(device_id)), None) = (self.account.mode, self.account.sent)
        else {
            return;
        };
        if self.state.read(cx).account_busy() {
            return;
        }
        let name = self.account.device_name.read(cx).value().to_string();
        self.account.sent = Some(AskKind::RenameDevice(device_id));
        self.state
            .update(cx, |state, cx| state.rename_device(device_id, &name, cx));
        cx.notify();
    }

    fn confirm_device_removal(&mut self, device_id: DeviceId, cx: &mut Context<Self>) {
        if self.account.sent.is_some() || self.state.read(cx).account_busy() {
            return;
        }
        self.account.sent = Some(AskKind::RemoveDevice(device_id));
        self.state
            .update(cx, |state, cx| state.remove_device(device_id, cx));
        cx.notify();
    }

    /// Puts text into the field for the link code of a new device, as typing does.
    pub fn type_link_code(&mut self, code: &str, window: &mut Window, cx: &mut Context<Self>) {
        let code = code.to_owned();
        self.account
            .code
            .update(cx, |field, cx| field.set_value(code, window, cx));
        self.link_code_changed(cx);
    }

    /// What was told about the text of the code field belongs to the text before.
    pub(super) fn link_code_changed(&mut self, cx: &mut Context<Self>) {
        self.account.invalid = None;
        cx.notify();
    }

    /// True when the field for the link code holds no text.
    pub fn link_code_is_empty(&self, cx: &gpui::App) -> bool {
        self.account.code.read(cx).value().is_empty()
    }

    /// Looks up the typed link code. An empty field is not sent, and neither is a second
    /// code while the first one is looked up. The field is emptied when the lookup
    /// starts: the flow holds the code from then on, and the field keeps no code that
    /// the server refused.
    pub fn submit_link_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let link = self
            .state
            .read(cx)
            .account_view()
            .and_then(|view| match view.page {
                PageView::Devices(page) => page.link,
                PageView::Profile(_) => None,
            });
        if !matches!(
            link,
            Some(LinkView::EnterCode {
                looking_up: false,
                ..
            })
        ) {
            return;
        }
        let code = self.account.code.read(cx).value().trim().to_owned();
        if code.is_empty() {
            self.account.invalid = Some(text::account::LINK_CODE_EMPTY);
        } else if code.chars().count() > crate::state::account::LONGEST_TYPED_CODE {
            self.account.invalid = Some(text::account::LINK_CODE_TOO_LONG);
        } else {
            self.account.invalid = None;
            self.account
                .code
                .update(cx, |field, cx| field.set_value("", window, cx));
            self.state
                .update(cx, |state, cx| state.look_up_link(&code, cx));
        }
        cx.notify();
    }

    /// Ends the link that the page shows: the flow starts again, and the code is gone.
    fn end_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.account.invalid = None;
        self.account
            .code
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.state.update(cx, |state, cx| state.reset_link(cx));
    }

    /// The code field holds a code only while the page asks for one. A lookup empties
    /// the field when it starts; a field that was filled afterwards is emptied here when
    /// the page shows the key, the result or another page.
    pub(super) fn settle_link_code(
        &mut self,
        view: &AccountView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let asks_for_code = match &view.page {
            PageView::Devices(page) => {
                matches!(page.link, None | Some(LinkView::EnterCode { .. }))
            }
            PageView::Profile(_) => false,
        };
        if !asks_for_code && !self.account.code.read(cx).value().is_empty() {
            self.account
                .code
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    fn render_device_row(
        &mut self,
        view: &AccountView,
        row: &DeviceLine,
        only: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let device_id = row.id;
        let (rename, remove) = (
            AskKind::RenameDevice(device_id),
            AskKind::RemoveDevice(device_id),
        );
        let content: AnyElement = match self.account.mode {
            Some(Mode::RenameDevice(id)) if id == device_id => div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.0))
                        .child(
                            div().flex_1().min_w_0().child(
                                Field::new(&self.account.device_name, text::account::DEVICE_NAME)
                                    .selector("device-name-field"),
                            ),
                        )
                        .child(
                            Button::new("device-form-cancel")
                                .label(text::CANCEL)
                                .selector("device-form-cancel")
                                .h(u(30.0))
                                .px(u(10.0))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_account_mode(None, "", window, cx);
                                })),
                        )
                        .child(
                            Button::new("device-form-send")
                                .kind(ButtonKind::Primary)
                                .label(text::SAVE)
                                .selector("device-form-send")
                                .disabled(view.busy())
                                .h(u(30.0))
                                .px(u(12.0))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_device_name(window, cx);
                                })),
                        ),
                )
                .child(sentence(text::account::DEVICE_NAME_HELP, theme).mt(u(5.0)))
                .into_any_element(),
            Some(Mode::RemoveDevice(id)) if id == device_id => div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(u(8.0))
                // The question is fixed text. The name of the device stays on its line
                // above it: text of a server is no part of a question.
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(device_title(row, theme))
                        .child(
                            div()
                                .typeface(theme.types.small, theme)
                                .ink(theme.colors.ink)
                                .child(text::account::remove_device_question(row.this_device)),
                        ),
                )
                .child(
                    Button::new("device-remove-cancel")
                        .label(text::CANCEL)
                        .selector("device-remove-cancel")
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_account_mode(None, "", window, cx);
                        })),
                )
                .child(
                    Button::new("device-remove-confirm")
                        .kind(ButtonKind::Primary)
                        .label(text::account::REMOVE)
                        .selector("device-remove-confirm")
                        .disabled(view.busy())
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirm_device_removal(device_id, cx);
                        })),
                )
                .into_any_element(),
            _ => {
                let name = row.name.clone();
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child(device_text(row, theme))
                    .child(row_action(
                        ("device-rename", row_number(device_id)),
                        text::account::RENAME,
                        format!("device-rename-{device_id}"),
                        None,
                        cx.listener(move |this, _, window, cx| {
                            this.set_account_mode(
                                Some(Mode::RenameDevice(device_id)),
                                &name,
                                window,
                                cx,
                            );
                        }),
                    ))
                    .child(row_action(
                        ("device-remove", row_number(device_id)),
                        text::account::REMOVE,
                        format!("device-remove-{device_id}"),
                        only.then_some(text::account::LAST_DEVICE),
                        cx.listener(move |this, _, window, cx| {
                            this.set_account_mode(
                                Some(Mode::RemoveDevice(device_id)),
                                "",
                                window,
                                cx,
                            );
                        }),
                    ))
                    .into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                named(
                    format!("device-row-{device_id}"),
                    line(theme).child(content),
                )
                .flex_1(),
            )
            .children(refusal(
                view.refused(rename).or(view.refused(remove)),
                "device-refused",
            ))
            .into_any_element()
    }

    fn render_device_list(
        &mut self,
        view: &AccountView,
        list: &DeviceList,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let read_again = |cx: &mut Context<Self>| {
            Button::new("devices-read-again")
                .label(text::TRY_AGAIN)
                .selector("devices-read-again")
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |state, cx| state.reload_devices(cx));
                }))
        };
        let not_read = |reason: &str, cx: &mut Context<Self>| {
            div().flex_none().py(u(8.0)).child(
                Notice::error(format!("{} {reason}", text::account::DEVICES_NOT_READ))
                    .selector("devices-read-failed")
                    .action(read_again(cx)),
            )
        };
        match list {
            DeviceList::Reading => sentence(text::account::READING, theme)
                .py(u(10.0))
                .debug_selector(|| "devices-reading".to_owned())
                .into_any_element(),
            DeviceList::Failed(reason) => not_read(reason, cx).into_any_element(),
            DeviceList::Listed {
                rows,
                only,
                hidden,
                failed,
            } => {
                let mut column = div().flex().flex_col().flex_none();
                for row in rows {
                    column = column.child(self.render_device_row(view, row, *only, theme, cx));
                }
                column
                    // A disabled control states its reason without a pointer.
                    .when(*only, |column| {
                        column.child(sentence(text::account::LAST_DEVICE, theme).mt(u(6.0)))
                    })
                    .when(*hidden > 0, |column| {
                        column
                            .child(sentence(text::account::more_devices(*hidden), theme).mt(u(6.0)))
                    })
                    .when_some(failed.as_deref(), |column, reason| {
                        column.child(not_read(reason, cx))
                    })
                    .into_any_element()
            }
        }
    }

    /// The approval of a new device: the code, then the key to compare, then the result.
    fn render_link(
        &mut self,
        link: &LinkView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let failure = |error: &Option<String>| {
            error.clone().map(|error| {
                div()
                    .flex_none()
                    .mt(u(6.0))
                    .child(Notice::error(error).selector("link-error"))
            })
        };
        let cancel = |cx: &mut Context<Self>| {
            Button::new("link-cancel")
                .label(text::CANCEL)
                .selector("link-cancel")
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(|this, _, window, cx| this.end_link(window, cx)))
        };
        match link {
            LinkView::EnterCode { looking_up, error } => div()
                .flex()
                .flex_col()
                .flex_none()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.0))
                        .child(
                            div().flex_none().w(u(220.0)).child(
                                Field::new(&self.account.code, text::account::LINK_CODE_FIELD)
                                    .kind(FieldKind::Code)
                                    .selector("link-code-field"),
                            ),
                        )
                        .child(
                            Button::new("link-look-up")
                                .label(if *looking_up {
                                    text::account::LOOKING_UP
                                } else {
                                    text::account::LOOK_UP
                                })
                                .selector("link-look-up")
                                .disabled(*looking_up)
                                .h(u(32.0))
                                .px(u(12.0))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_link_code(window, cx);
                                })),
                        ),
                )
                .child(sentence(text::account::LINK_NEW_DEVICE_HELP, theme).mt(u(5.0)))
                .when_some(self.account.invalid, |column, invalid| {
                    column.child(
                        div()
                            .flex_none()
                            .mt(u(6.0))
                            .child(Notice::error(invalid).selector("link-code-invalid")),
                    )
                })
                .children(failure(error))
                .into_any_element(),
            LinkView::Confirm {
                device_name,
                fingerprint: key,
                expires_at,
                approving,
                error,
            } => div()
                .flex()
                .flex_col()
                .flex_none()
                .debug_selector(|| "link-confirm".to_owned())
                // The name comes from the new device: it has a line of its own that is
                // cut at the width of the page.
                .child(
                    sentence(text::account::new_device_named(device_name), theme)
                        .truncate()
                        .debug_selector(|| "link-device-name".to_owned()),
                )
                .child(
                    fingerprint(
                        text::account::KEY_OF_NEW_DEVICE,
                        key,
                        "link-fingerprint",
                        theme,
                    )
                    .mt(u(8.0)),
                )
                .child(sentence(text::account::COMPARE_KEY, theme).mt(u(6.0)))
                .child(sentence(text::account::code_expires(*expires_at), theme).mt(u(2.0)))
                .children(failure(error))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(u(8.0))
                        .mt(u(10.0))
                        .child(cancel(cx))
                        .child(
                            Button::new("link-approve")
                                .kind(ButtonKind::Primary)
                                .icon(Icon::Check)
                                .label(if *approving {
                                    text::account::APPROVING
                                } else {
                                    text::account::APPROVE
                                })
                                .selector("link-approve")
                                .disabled(*approving)
                                .h(u(30.0))
                                .px(u(12.0))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |state, cx| state.approve_link(cx));
                                })),
                        ),
                )
                .into_any_element(),
            LinkView::OutcomeUnknown {
                device_name,
                fingerprint: key,
                checking,
                error,
            } => div()
                .flex()
                .flex_col()
                .flex_none()
                .child(
                    Notice::new(text::account::OUTCOME_UNKNOWN)
                        .icon(Icon::Warning)
                        .selector("link-unknown"),
                )
                .child(
                    sentence(text::account::new_device_named(device_name), theme)
                        .truncate()
                        .mt(u(8.0)),
                )
                .child(
                    fingerprint(
                        text::account::KEY_OF_NEW_DEVICE,
                        key,
                        "link-fingerprint",
                        theme,
                    )
                    .mt(u(8.0)),
                )
                .children(failure(error))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(u(8.0))
                        .mt(u(10.0))
                        .child(cancel(cx))
                        .child(
                            Button::new("link-check")
                                .kind(ButtonKind::Primary)
                                .icon(Icon::Retry)
                                .label(if *checking {
                                    text::account::CHECKING
                                } else {
                                    text::CHECK_AGAIN
                                })
                                .selector("link-check")
                                .disabled(*checking)
                                .h(u(30.0))
                                .px(u(12.0))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |state, cx| state.check_link(cx));
                                })),
                        ),
                )
                .into_any_element(),
            LinkView::Approved { device_name } => div()
                .flex_none()
                .child(
                    Notice::new(format!(
                        "{}. {}",
                        text::account::DEVICE_LINKED,
                        text::account::device_linked(device_name)
                    ))
                    .icon(Icon::Check)
                    .selector("link-approved")
                    .action(
                        Button::new("link-done")
                            .kind(ButtonKind::Primary)
                            .label(text::account::DONE)
                            .selector("link-done")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.end_link(window, cx);
                            })),
                    ),
                )
                .into_any_element(),
        }
    }

    pub(crate) fn render_devices_page(
        &mut self,
        view: &AccountView,
        page: &DevicesPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let list = self.render_device_list(view, &page.list, &theme, cx);
        let link = page
            .link
            .as_ref()
            .map(|link| self.render_link(link, &theme, cx));
        div()
            .flex()
            .flex_col()
            .child(page_title(
                text::account::page_name(AccountPage::Devices),
                text::account::DEVICES_HELP,
                &theme,
            ))
            .child(list)
            .child(group_label(text::account::LINK_NEW_DEVICE, &theme))
            .children(link)
            .into_any_element()
    }
}
