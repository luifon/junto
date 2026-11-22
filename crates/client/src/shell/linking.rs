//! Device linking in the window (ADR 0782): what a server shows while this device waits
//! for its link, and the layer that links a new device with one code: the device that
//! approves checks its servers and types the code, the new device asks one server for the
//! code and then links on the servers that the other device names (ADR 0104).

use gpui::{
    AnyElement, Context, Entity, InteractiveElement, IntoElement, ParentElement, Role,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder,
};
use gpui_base::input::InputState;
use junto_client_core::protocol::ids::ServerId;

use super::account::Surface;
use super::admin::{group_label, line, named, sentence};
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::linking::{
    ApprovalLine, ApprovalShown, ChoiceLine, FirstDeviceView, FirstStep, LinkAllView, LinkLine,
    LinkRole, LinkShown, LinkWait, NewDeviceView, NewStep,
};
use crate::state::view::{SelectedServer, ServerStatus};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Field, FieldEvent, Notice, StatePanel, StyleExt, Toggle, icon,
};

/// The longest text that the field for a link code keeps: a code has 8 symbols, in two
/// groups.
const LONGEST_CODE_TEXT: usize = 16;

/// What the layer of a link holds of the window itself.
pub(crate) struct LinkingUi {
    /// The address of a server of the account, on the new device.
    pub(crate) address: Entity<InputState>,
    /// The name of this device, on the new device.
    pub(crate) device_name: Entity<InputState>,
    /// The code of the new device, on the device that approves.
    pub(crate) code: Entity<InputState>,
    /// Why the text of a field was not sent.
    pub(crate) invalid: Option<&'static str>,
    pub(crate) scroll: ScrollHandle,
    /// What the layer drew last. A test reads the layer from it.
    #[cfg(feature = "test-support")]
    pub(crate) drawn: Option<LinkAllView>,
}

impl LinkingUi {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let address = Field::state(text::account::SERVER_ADDRESS_PLACEHOLDER, window, cx);
        let device_name = Field::state(text::account::THIS_DEVICE_NAME, window, cx);
        let code = Field::state(text::account::LINK_CODE_PLACEHOLDER, window, cx);
        let subscriptions = vec![
            Field::observe(
                &address,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => cx.notify(),
                    FieldEvent::Submitted => this.submit_link_address(window, cx),
                },
            ),
            Field::observe(
                &device_name,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => cx.notify(),
                    FieldEvent::Submitted => this.submit_link_address(window, cx),
                },
            ),
            Field::observe(
                &code,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => this.link_code_changed(window, cx),
                    FieldEvent::Submitted => this.submit_link_code(window, cx),
                },
            ),
        ];
        let ui = Self {
            address,
            device_name,
            code,
            invalid: None,
            scroll: ScrollHandle::new(),
            #[cfg(feature = "test-support")]
            drawn: None,
        };
        (ui, subscriptions)
    }
}

/// The label of a value that a person compares or types, above the value.
fn value_label(label: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .typeface(theme.types.label, theme)
        .ink(theme.colors.bronze)
        .child(label)
}

/// A value in fixed-width type. `label` names it in the accessibility tree; a code is no
/// label, because a label is kept by the accessibility tree.
fn mono_value(
    value: String,
    label: &'static str,
    selector: &'static str,
    large: bool,
    theme: &Theme,
) -> impl IntoElement + use<> {
    div()
        .id(selector)
        .role(Role::Label)
        .aria_label(label)
        .debug_selector(move || selector.to_owned())
        .flex_none()
        .typeface(theme.types.mono_code, theme)
        // A key has eleven groups: at the smaller size it fits one line of a dialog.
        .map(|value| {
            if large {
                value.text_size(u(26.0)).line_height(u(34.0))
            } else {
                value.text_size(u(13.0)).line_height(u(19.0))
            }
        })
        .ink(theme.colors.ink)
        .child(value)
}

/// A value with the button that copies it (ADR 0731).
fn with_copy(value: impl IntoElement, copy: AnyElement) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(6.0))
        .child(value)
        .child(copy)
}

/// The copy buttons of the code and of the key that a device that waits for its link shows.
pub(crate) struct WaitCopies {
    code: Option<AnyElement>,
    fingerprint: AnyElement,
}

impl Shell {
    pub(crate) fn link_wait_copies(&self, wait: &LinkWait, cx: &mut Context<Self>) -> WaitCopies {
        let copy =
            |selector: &'static str, label: &'static str, value: String, cx: &mut Context<Self>| {
                div()
                    .flex_none()
                    .debug_selector(move || selector.to_owned())
                    .child(Self::value_copy_button(selector, label, value, cx))
                    .into_any_element()
            };
        WaitCopies {
            code: wait.code.as_ref().map(|code| {
                copy(
                    "link-wait-code-copy",
                    text::account::COPY_LINK_CODE,
                    code.expose().to_owned(),
                    cx,
                )
            }),
            fingerprint: copy(
                "link-wait-fingerprint-copy",
                text::account::COPY_DEVICE_CODE,
                wait.fingerprint.clone(),
                cx,
            ),
        }
    }
}

/// What a device that waits for its link shows: the code, the key of this device, the
/// expiry, and what the person does with them.
pub(crate) fn link_wait_block(
    wait: &LinkWait,
    copies: WaitCopies,
    centered: bool,
    theme: &Theme,
    cycle: junto_client_core::clock::HourCycle,
) -> gpui::Div {
    let WaitCopies {
        code: code_copy,
        fingerprint: fingerprint_copy,
    } = copies;
    let code: AnyElement = match (&wait.code, code_copy) {
        (Some(code), Some(copy)) => with_copy(
            mono_value(
                code.expose().to_owned(),
                text::account::LINK_CODE,
                "link-wait-code",
                true,
                theme,
            ),
            copy,
        )
        .into_any_element(),
        _ => Notice::error(text::account::LINK_CODE_NOT_SHOWN)
            .selector("link-wait-no-code")
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .flex_none()
        .when(centered, |block| block.items_center())
        .debug_selector(|| "link-wait".to_owned())
        .child(value_label(text::account::LINK_CODE, theme))
        .child(code)
        .child(value_label(text::account::KEY_OF_THIS_DEVICE, theme).mt(u(10.0)))
        .child(with_copy(
            mono_value(
                wait.fingerprint.clone(),
                text::account::KEY_OF_THIS_DEVICE,
                "link-wait-fingerprint",
                false,
                theme,
            ),
            fingerprint_copy,
        ))
        .child(
            sentence(text::account::code_expires(wait.expires_at, cycle), theme)
                .mt(u(8.0))
                .debug_selector(|| "link-wait-expiry".to_owned()),
        )
        .child(
            sentence(text::account::LINK_WAIT_HELP, theme)
                .mt(u(6.0))
                .when(centered, |help| help.text_center()),
        )
        .child(
            sentence(text::account::LINK_WAIT_RECOVERY, theme)
                .mt(u(6.0))
                .when(centered, |help| help.text_center())
                .debug_selector(|| "link-wait-recovery".to_owned()),
        )
}

fn failure(reason: Option<&str>, selector: &'static str) -> Option<AnyElement> {
    Some(
        crate::ui::notice_slot(selector)
            .children(reason.map(|reason| Notice::error(reason.to_owned()).selector(selector)))
            .into_any_element(),
    )
}

/// The name of a server and under it a second line, in a line of the layer.
fn server_text(name: &str, detail: Option<String>, theme: &Theme) -> gpui::Div {
    let name: SharedString = if name.is_empty() {
        text::account::UNSAVED_SERVER.into()
    } else {
        name.to_owned().into()
    };
    div()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .typeface(theme.types.control_strong, theme)
                .ink(theme.colors.ink)
                .truncate()
                .child(name),
        )
        .when_some(detail, |column, detail| {
            column.child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_2)
                    .truncate()
                    .child(detail),
            )
        })
}

/// The state of one server at the end of its line.
fn progress(text: impl Into<SharedString>, mark: Option<Icon>, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(5.0))
        .max_w(u(330.0))
        .typeface(theme.types.small_strong, theme)
        .ink(theme.colors.ink_2)
        .when_some(mark, |state, mark| {
            state.child(icon(mark, 13.0, theme.colors.ink_2))
        })
        .child(div().min_w_0().child(text.into()))
}

impl Shell {
    /// The device-link scroll position used by the window tests.
    #[cfg(feature = "test-support")]
    pub fn linking_scroll_handle(&self) -> ScrollHandle {
        self.linking.scroll.clone()
    }

    /// Opens a link in one role. With `only`, the device that approves starts with only
    /// that server checked.
    pub fn open_link_all(
        &mut self,
        role: LinkRole,
        only: Option<ServerId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reset_linking_ui(window, cx);
        self.state
            .update(cx, |state, cx| state.open_link_all(role, only, cx));
        if role == LinkRole::NewDevice {
            self.linking.device_name.update(cx, |field, cx| {
                field.set_value(text::device_name(), window, cx);
            });
        }
        match self.layer {
            Layer::LinkAll => {}
            Layer::None => self.open_layer(Layer::LinkAll, window, cx),
            // The layer takes the place of the layer that opened it.
            _ => {
                self.layer = Layer::LinkAll;
                self.layer_focus().clone().focus(window, cx);
            }
        }
        cx.notify();
    }

    /// The fields of the layer are emptied when the layer opens and closes.
    pub(crate) fn reset_linking_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.linking.invalid = None;
        #[cfg(feature = "test-support")]
        {
            self.linking.drawn = None;
        }
        for field in [
            self.linking.address.clone(),
            self.linking.device_name.clone(),
            self.linking.code.clone(),
        ] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    /// A field for a link code keeps a short text only.
    fn link_code_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (too_long, empty) = {
            let value = self.linking.code.read(cx).value();
            (value.chars().count() > LONGEST_CODE_TEXT, value.is_empty())
        };
        if too_long {
            self.linking
                .code
                .update(cx, |field, cx| field.set_value("", window, cx));
            self.linking.invalid = Some(text::account::LINK_CODE_TOO_LONG);
        } else if !empty {
            self.linking.invalid = None;
        }
        cx.notify();
    }

    /// Puts text into the field for the code of the new device, as typing does.
    pub fn type_link_code(&mut self, code: &str, window: &mut Window, cx: &mut Context<Self>) {
        let code = code.to_owned();
        self.linking
            .code
            .update(cx, |field, cx| field.set_value(code, window, cx));
        self.link_code_changed(window, cx);
    }

    /// Puts text into the field for the address of a server, as typing does.
    pub fn type_link_address(
        &mut self,
        address: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let address = address.to_owned();
        self.linking
            .address
            .update(cx, |field, cx| field.set_value(address, window, cx));
        cx.notify();
    }

    /// Why the text of a field of the layer was not sent.
    pub fn link_text_refusal(&self) -> Option<&'static str> {
        self.linking.invalid
    }

    /// The new device asks the server at the typed address for a link code.
    pub fn submit_link_address(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let enters = matches!(
            self.state.read(cx).link_all_view(),
            Some(LinkAllView::New(NewDeviceView {
                step: NewStep::EnterAddress,
                busy: false,
                ..
            }))
        );
        if !enters {
            return;
        }
        let address = self.linking.address.read(cx).value().trim().to_owned();
        if address.is_empty() {
            self.linking.invalid = Some(text::account::ADDRESS_EMPTY);
            cx.notify();
            return;
        }
        let device_name = self.linking.device_name.read(cx).value().to_string();
        self.linking.invalid = None;
        self.state.update(cx, |state, cx| {
            state.start_linking(address, device_name, cx)
        });
    }

    /// The device that approves looks the typed code up on its checked servers. Nothing
    /// is approved.
    pub fn submit_link_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enters = matches!(
            self.state.read(cx).link_all_view(),
            Some(LinkAllView::First(FirstDeviceView {
                step: FirstStep::EnterCode { .. },
                busy: false,
                ..
            }))
        );
        if !enters {
            return;
        }
        let code = self.linking.code.read(cx).value().trim().to_owned();
        if code.is_empty() {
            self.linking.invalid = Some(text::account::LINK_CODE_EMPTY);
            cx.notify();
            return;
        }
        self.linking.invalid = None;
        self.linking
            .code
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.state
            .update(cx, |state, cx| state.look_up_link_code(code, cx));
    }

    /// What the layer of a link drew last.
    #[cfg(feature = "test-support")]
    pub fn link_all_drawn(&self) -> Option<&LinkAllView> {
        self.linking.drawn.as_ref()
    }

    fn start_over(&mut self, cx: &mut Context<Self>) -> Button {
        Button::new("link-all-reset")
            .label(text::account::START_OVER)
            .selector("link-all-reset")
            .h(u(30.0))
            .px(u(12.0))
            .on_click(cx.listener(|this, _, window, cx| {
                let role = this.state.read(cx).link_all_role();
                if let Some(role) = role {
                    this.open_link_all(role, None, window, cx);
                }
            }))
    }

    fn close_button(&mut self, cx: &mut Context<Self>) -> Button {
        Button::new("link-all-done")
            .kind(ButtonKind::Primary)
            .label(text::account::DONE)
            .selector("link-all-done")
            .h(u(30.0))
            .px(u(12.0))
            .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
    }

    /// One server of this device with its switch, in the choice of a link.
    fn render_choice(
        &mut self,
        choice: &ChoiceLine,
        busy: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = choice.server_id;
        let detail: Option<SharedString> = if choice.locked {
            Some(text::account::LINK_STARTS_HERE.into())
        } else {
            choice.unavailable.clone().map(Into::into)
        };
        let held: Option<SharedString> = if choice.locked {
            Some(text::account::LINK_STARTS_HERE.into())
        } else if busy {
            Some(text::account::CHECKING.into())
        } else {
            choice.unavailable.clone().map(Into::into)
        };
        let toggle = Toggle::new(
            ("link-choice", server_id.as_uuid().as_u128() as u64),
            choice.server.clone(),
            choice.checked,
            (text::ON, text::OFF),
        )
        .held(held)
        .on_click(cx.listener(move |this, _, _, cx| {
            let checked = !this.link_choice_checked(server_id, cx);
            this.state.update(cx, |state, cx| {
                state.choose_link_server(server_id, checked, cx)
            });
        }));
        named(
            format!("link-choice-{server_id}"),
            line(theme)
                .child(super::admin::line_text(
                    choice.server.clone(),
                    detail,
                    theme,
                ))
                .child(toggle),
        )
        .flex_1()
        .into_any_element()
    }

    fn link_choice_checked(&self, server_id: ServerId, cx: &gpui::App) -> bool {
        let Some(LinkAllView::First(view)) = self.state.read(cx).link_all_view() else {
            return false;
        };
        let (FirstStep::EnterCode { choices } | FirstStep::Confirm { choices, .. }) = view.step
        else {
            return false;
        };
        choices
            .iter()
            .any(|choice| choice.server_id == server_id && choice.checked)
    }

    fn render_approval_line(row: &ApprovalLine, theme: &Theme) -> AnyElement {
        let server_id = row.server_id;
        let state = match &row.progress {
            ApprovalShown::Waiting => progress(
                text::account::PROGRESS_WAITING_FOR_DEVICE,
                Some(Icon::Clock),
                theme,
            ),
            ApprovalShown::Working => progress(text::account::PROGRESS_WORKING, None, theme),
            ApprovalShown::Approved => {
                progress(text::account::PROGRESS_APPROVED, Some(Icon::Check), theme)
            }
            ApprovalShown::AlreadyLinked => progress(
                text::account::PROGRESS_ALREADY_LINKED,
                Some(Icon::Check),
                theme,
            ),
            ApprovalShown::Failed(reason) => progress(
                text::account::progress_failed(reason),
                Some(Icon::Warning),
                theme,
            ),
        };
        named(
            format!("approval-line-{server_id}"),
            line(theme)
                .child(super::admin::line_text(
                    row.server.clone(),
                    Some(text::account::account_line(&row.account).into()),
                    theme,
                ))
                .child(
                    crate::ui::notice_slot((
                        "approval-progress",
                        server_id.as_uuid().as_u128() as u64,
                    ))
                    .w(u(330.0))
                    .child(state),
                ),
        )
        .flex_1()
        .into_any_element()
    }

    fn render_first_device(
        &mut self,
        view: &FirstDeviceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut column = div().flex().flex_col();
        match &view.step {
            FirstStep::EnterCode { choices } => {
                column = column
                    .child(sentence(text::account::LINK_NEW_DEVICE_HELP, theme))
                    .child(group_label(text::account::SERVERS, theme));
                for choice in choices {
                    column = column.child(self.render_choice(choice, view.busy, theme, cx));
                }
                let none = !choices.iter().any(|choice| choice.checked);
                column = column
                    .child(value_label(text::account::LINK_CODE_FIELD, theme).mt(u(12.0)))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(u(8.0))
                            .mt(u(3.0))
                            .child(
                                div().flex_1().min_w_0().child(
                                    Field::new(&self.linking.code, text::account::LINK_CODE_FIELD)
                                        .selector("link-code-field"),
                                ),
                            )
                            .child(
                                Button::new("link-code-look-up")
                                    .kind(ButtonKind::Primary)
                                    .label(if view.busy {
                                        text::account::LOOKING_UP
                                    } else {
                                        text::account::LOOK_UP
                                    })
                                    .selector("link-code-look-up")
                                    .disabled(view.busy || none)
                                    .h(u(32.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_link_code(window, cx);
                                    })),
                            ),
                    )
                    .children(failure(self.linking.invalid, "link-text-invalid"))
                    .children(failure(view.error.as_deref(), "link-all-error"));
            }
            FirstStep::Confirm {
                device_name,
                device_code,
                choices,
            } => {
                column = column
                    .child(sentence(text::account::COMPARE_KEY, theme))
                    .child(value_label(text::account::KEY_OF_NEW_DEVICE, theme).mt(u(8.0)))
                    .child(with_copy(
                        mono_value(
                            device_code.clone(),
                            text::account::KEY_OF_NEW_DEVICE,
                            "link-all-device-code",
                            false,
                            theme,
                        ),
                        div()
                            .flex_none()
                            .debug_selector(|| "link-all-device-code-copy".to_owned())
                            .child(Self::value_copy_button(
                                "link-all-device-code-copy",
                                text::account::COPY_DEVICE_CODE,
                                device_code.clone(),
                                cx,
                            ))
                            .into_any_element(),
                    ))
                    // The name comes from the new device: it has a line of its own that is
                    // cut at the width of the page.
                    .child(
                        sentence(text::account::new_device_named(device_name), theme)
                            .truncate()
                            .mt(u(4.0)),
                    )
                    .child(group_label(text::account::SERVERS, theme));
                for choice in choices {
                    column = column.child(self.render_choice(choice, view.busy, theme, cx));
                }
                column = column
                    .children(failure(view.error.as_deref(), "link-all-error"))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap(u(8.0))
                            .mt(u(12.0))
                            .child(self.start_over(cx))
                            .child(
                                Button::new("link-all-approve")
                                    .kind(ButtonKind::Primary)
                                    .icon(Icon::Check)
                                    .label(if view.busy {
                                        text::account::APPROVING
                                    } else {
                                        text::account::APPROVE
                                    })
                                    .selector("link-all-approve")
                                    .disabled(view.busy)
                                    .h(u(30.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state
                                            .update(cx, |state, cx| state.approve_new_device(cx));
                                    })),
                            ),
                    );
            }
            FirstStep::Linking {
                device_name,
                lines,
                done,
            } => {
                column = column
                    .child(sentence(text::account::device_linked(device_name), theme).truncate());
                column = column.child(group_label(text::account::SERVERS, theme));
                for row in lines {
                    column = column.child(Self::render_approval_line(row, theme));
                }
                let buttons = div().flex().flex_none().gap(u(8.0)).mt(u(12.0));
                let buttons = if *done {
                    buttons.child(self.close_button(cx))
                } else {
                    buttons.child(
                        Button::new("link-all-check")
                            .icon(Icon::Retry)
                            .label(if view.busy {
                                text::account::CHECKING
                            } else {
                                text::TRY_AGAIN
                            })
                            .selector("link-all-check")
                            .disabled(view.busy)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.follow_new_device(cx));
                            })),
                    )
                };
                column = column
                    .children(failure(view.error.as_deref(), "link-all-error"))
                    .child(buttons);
            }
        }
        column.into_any_element()
    }

    fn render_link_line(row: &LinkLine, theme: &Theme) -> AnyElement {
        let state = match &row.progress {
            LinkShown::Working => progress(text::account::PROGRESS_WORKING, None, theme),
            LinkShown::Waiting => {
                progress(text::account::PROGRESS_WAITING, Some(Icon::Clock), theme)
            }
            LinkShown::Linked => progress(text::account::PROGRESS_LINKED, Some(Icon::Check), theme),
            LinkShown::Failed(reason) => progress(
                text::account::progress_failed(reason),
                Some(Icon::Warning),
                theme,
            ),
        };
        named(
            format!("link-line-{}", row.server_id),
            line(theme)
                .child(server_text(&row.server, None, theme))
                .child(
                    crate::ui::notice_slot((
                        "link-progress",
                        row.server_id.as_uuid().as_u128() as u64,
                    ))
                    .w(u(330.0))
                    .child(state),
                ),
        )
        .flex_1()
        .into_any_element()
    }

    fn render_new_device(
        &mut self,
        view: &NewDeviceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let column = div().flex().flex_col();
        match &view.step {
            NewStep::EnterAddress => column
                .child(sentence(text::account::LINK_THIS_DEVICE_HELP, theme))
                .child(value_label(text::account::SERVER_ADDRESS, theme).mt(u(10.0)))
                .child(
                    div().flex_none().mt(u(3.0)).child(
                        Field::new(&self.linking.address, text::account::SERVER_ADDRESS)
                            .selector("link-address-field"),
                    ),
                )
                .child(value_label(text::account::THIS_DEVICE_NAME, theme).mt(u(10.0)))
                .child(
                    div().flex_none().mt(u(3.0)).child(
                        Field::new(&self.linking.device_name, text::account::THIS_DEVICE_NAME)
                            .selector("link-device-name-field"),
                    ),
                )
                .children(failure(self.linking.invalid, "link-text-invalid"))
                .children(failure(view.error.as_deref(), "link-all-error"))
                .child(
                    div().flex().flex_none().mt(u(12.0)).child(
                        Button::new("link-start")
                            .kind(ButtonKind::Primary)
                            .label(if view.busy {
                                text::account::REQUESTING_LINK
                            } else {
                                text::account::GET_A_CODE
                            })
                            .selector("link-start")
                            .disabled(view.busy)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_link_address(window, cx);
                            })),
                    ),
                )
                .into_any_element(),
            NewStep::Waiting { server, wait } => {
                let cycle = self.state.read(cx).hour_cycle();
                column
                    .child(
                        sentence(text::account::code_from(server), theme)
                            .truncate()
                            .mb(u(8.0)),
                    )
                    .children(wait.as_ref().map(|wait| {
                        let copies = self.link_wait_copies(wait, cx);
                        link_wait_block(wait, copies, false, theme, cycle)
                    }))
                    .children(failure(view.error.as_deref(), "link-all-error"))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap(u(8.0))
                            .mt(u(12.0))
                            .child(self.start_over(cx))
                            .child(
                                Button::new("link-all-new-code")
                                    .icon(Icon::Retry)
                                    .label(text::account::NEW_CODE)
                                    .selector("link-all-new-code")
                                    .disabled(view.busy)
                                    .h(u(30.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |state, cx| state.new_link_code(cx));
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            NewStep::Linked {
                server,
                lines,
                offer_read,
                done,
            } => {
                let mut column = column.child(
                    Notice::new(text::account::linked_on(server))
                        .icon(Icon::Check)
                        .selector("link-all-first-linked"),
                );
                if !lines.is_empty() {
                    column = column.child(group_label(text::account::SERVERS, theme));
                }
                for row in lines {
                    column = column.child(Self::render_link_line(row, theme));
                }
                if !offer_read {
                    column = column.child(
                        sentence(text::account::READING_OTHER_SERVERS, theme)
                            .mt(u(8.0))
                            .debug_selector(|| "link-reading-offer".to_owned()),
                    );
                }
                let buttons = div().flex().flex_none().gap(u(8.0)).mt(u(12.0));
                let buttons = if *done {
                    buttons.child(self.close_button(cx))
                } else {
                    buttons.child(
                        Button::new("link-check-all")
                            .icon(Icon::Retry)
                            .label(if view.busy {
                                text::account::CHECKING
                            } else {
                                text::TRY_AGAIN
                            })
                            .selector("link-check-all")
                            .disabled(view.busy)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.check_this_device(cx));
                            })),
                    )
                };
                column
                    .children(failure(view.error.as_deref(), "link-all-error"))
                    .child(buttons)
                    .into_any_element()
            }
        }
    }

    pub(crate) fn render_link_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.state.read(cx).link_all_view() else {
            return div().into_any_element();
        };
        #[cfg(feature = "test-support")]
        {
            self.linking.drawn = Some(view.clone());
        }
        let theme = theme(cx);
        let (role, page) = match &view {
            LinkAllView::First(view) => (
                LinkRole::FirstDevice,
                self.render_first_device(view, &theme, cx),
            ),
            LinkAllView::New(view) => (
                LinkRole::NewDevice,
                self.render_new_device(view, &theme, cx),
            ),
        };
        let surface = Surface {
            id: "link-all",
            title: text::account::role_title(role),
            icon: Icon::Link,
            note: SharedString::default(),
            close_selector: "link-all-close",
            nav: None,
            page,
            page_scroll: self.linking.scroll.clone(),
        };
        self.render_surface(surface, window, cx)
    }

    /// The section "Linked devices" of the settings: the two ways into a link.
    pub(crate) fn render_devices_settings(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let open = |role: LinkRole, selector: &'static str, cx: &mut Context<Self>| {
            Button::new(selector)
                .icon(Icon::Link)
                .label(text::account::role_title(role))
                .selector(selector)
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_link_all(role, None, window, cx);
                }))
        };
        vec![
            div()
                .mb(u(5.0))
                .typeface(theme.types.label, theme)
                .ink(theme.colors.bronze)
                .child(text::account::DEVICES_SECTION)
                .into_any_element(),
            div()
                .flex()
                .flex_wrap()
                .gap(u(8.0))
                .child(open(LinkRole::FirstDevice, "settings-link-first", cx))
                .child(open(LinkRole::NewDevice, "settings-link-new", cx))
                .into_any_element(),
            div()
                .mt(u(5.0))
                .typeface(theme.types.small, theme)
                .ink(theme.colors.ink_2)
                .child(text::account::DEVICES_SECTION_HELP)
                .into_any_element(),
        ]
    }

    /// The third way of the join dialog: a device of an account that exists starts the
    /// link of this device on this server.
    pub(crate) fn render_link_way(
        &mut self,
        busy: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(12.0))
            .mt(u(10.0))
            .pt(u(10.0))
            .px(u(12.0))
            .pb(u(12.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(gpui::Hsla::from(colors.rule))
            .bg(gpui::Hsla::from(colors.travertine_hi))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.heading, theme)
                            .ink(colors.ink)
                            .child(text::account::LINK_THIS_DEVICE),
                    )
                    .child(
                        div()
                            .mt(u(2.0))
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(text::account::LINK_WAY_HELP),
                    ),
            )
            .child(
                Button::new("request-link")
                    .icon(Icon::Link)
                    .label(text::account::REQUEST_LINK)
                    .disabled(busy)
                    .selector("request-link")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.request_link_in_dialog(window, cx);
                    })),
            )
            .into_any_element()
    }

    /// The two actions of a device that waits for its link: a new code, and the end of
    /// the wait. The key of the device belongs to no account yet, so forgetting it
    /// loses nothing and asks no question.
    pub(crate) fn link_wait_actions(
        &mut self,
        server_id: ServerId,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> [Button; 2] {
        [
            Button::new("link-wait-cancel")
                .label(text::account::CANCEL_LINK)
                .selector("link-wait-cancel")
                .disabled(busy)
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.forget_account(server_id, cx));
                })),
            Button::new("link-wait-new-code")
                .icon(Icon::Retry)
                .label(text::account::NEW_CODE)
                .selector("link-wait-new-code")
                .disabled(busy)
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.request_link(server_id, cx));
                })),
        ]
    }

    /// The state of a selected server that waits for a device link, in the main area.
    /// Absent for every other state.
    pub(crate) fn render_link_wait_panel(
        &mut self,
        server: &SelectedServer,
        cx: &mut Context<Self>,
    ) -> Option<StatePanel> {
        if !matches!(server.status, ServerStatus::WaitingForLink { .. }) {
            return None;
        }
        let wait = self.state.read(cx).link_wait(server.id)?;
        let theme = theme(cx);
        let error = server.error.as_ref().map(text::account::link_error);
        let [cancel, new_code] = self.link_wait_actions(server.id, server.busy, cx);
        Some(
            StatePanel::with_detail(
                text::WAITING_FOR_LINK_TITLE,
                link_wait_block(
                    &wait,
                    self.link_wait_copies(&wait, cx),
                    true,
                    &theme,
                    self.state.read(cx).hour_cycle(),
                )
                .mt(u(12.0)),
            )
            .icon(Icon::Clock)
            .selector("waiting-for-link")
            .error(error.map(SharedString::from))
            .action(new_code)
            .action(cancel),
        )
    }
}
