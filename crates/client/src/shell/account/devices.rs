//! The devices page of the account layer (ADR 0112, ADR 0781): the devices of the account
//! with a new name and the removal, and the button that links a new device with this
//! server checked (ADR 0782).

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::DeviceId;

use super::Mode;
use crate::assets::Icon;
use crate::shell::Shell;
use crate::shell::admin::{line, named, page_title, refusal, row_action, sentence};
use crate::state::account::{
    AccountPage, AccountView, AskKind, DeviceLine, DeviceList, DevicesPage,
};
use crate::state::linking::LinkRole;
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, ButtonKind, Chip, Field, Notice, StyleExt};

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
fn device_text(
    row: &DeviceLine,
    theme: &Theme,
    cycle: junto_client_core::clock::HourCycle,
) -> gpui::Div {
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
            text::account::added_on(row.added, cycle)
        )))
        .child(small(text::account::last_login(row.last_login, cycle)))
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

    fn render_device_row(
        &mut self,
        view: &AccountView,
        row: &DeviceLine,
        only: bool,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let device_id = row.id;
        let (rename, remove) = (
            AskKind::RenameDevice(device_id),
            AskKind::RemoveDevice(device_id),
        );
        let (content, actions) = crate::ui::collect_actions(|| match self.account.mode {
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
                    .child(device_text(row, theme, self.state.read(cx).hour_cycle()))
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
        });
        let focus = window
            .use_keyed_state(
                SharedString::from(format!("device-focus-{device_id}")),
                cx,
                |_, cx| cx.focus_handle().tab_stop(true),
            )
            .read(cx)
            .clone();
        let row = div()
            .id(SharedString::from(format!("device-result-{device_id}")))
            .track_focus(&focus)
            .flex()
            .flex_col()
            .flex_none()
            // Room at the right end for the menu button of the row (ADR 0731).
            .pr(u(28.0))
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
            ));
        let focused = focus.is_focused(window);
        let row = crate::ui::bind_row_action_menu(
            row,
            SharedString::from(format!("device-actions-{device_id}")),
            crate::ui::RowMenu::overlay(focused),
            actions,
            window,
            cx,
        );
        // The answer closes the form at a time that the person does not choose: the row
        // keeps the height of its form, whatever the text, the width and the scale make
        // that height (ADR 0730).
        let modes = [Mode::RenameDevice(device_id), Mode::RemoveDevice(device_id)];
        self.account
            .space
            .row(self.account.mode, &modes, row)
            .into_any_element()
    }

    fn render_device_list(
        &mut self,
        view: &AccountView,
        list: &DeviceList,
        theme: &Theme,
        window: &mut Window,
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
                    column =
                        column.child(self.render_device_row(view, row, *only, theme, window, cx));
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
                    .child(
                        crate::ui::notice_slot("devices-read-failed-slot")
                            .children(failed.as_deref().map(|reason| not_read(reason, cx))),
                    )
                    .into_any_element()
            }
        }
    }

    pub(crate) fn render_devices_page(
        &mut self,
        view: &AccountView,
        page: &DevicesPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let list = self.render_device_list(view, &page.list, &theme, window, cx);
        let server_id = view.server_id;
        // The same flow as in the settings, with only this server checked (ADR 0782).
        let link = Button::new("account-link-new-device")
            .icon(Icon::Link)
            .label(text::account::LINK_NEW_DEVICE)
            .selector("account-link-new-device")
            .h(u(30.0))
            .px(u(12.0))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_link_all(LinkRole::FirstDevice, Some(server_id), window, cx);
            }));
        div()
            .flex()
            .flex_col()
            .child(page_title(
                text::account::page_name(AccountPage::Devices),
                text::account::DEVICES_HELP,
                &theme,
            ))
            .child(list)
            .child(div().flex().flex_none().mt(u(12.0)).child(link))
            .into_any_element()
    }
}
