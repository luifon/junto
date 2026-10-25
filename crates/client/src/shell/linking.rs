//! Device linking in the window (ADR 0782): what a server shows while this device waits
//! for its link, the third way of the join dialog, and the layer that links a device on
//! all servers with two texts that the person carries (ADR 0716).

use gpui::{
    AnyElement, ClipboardItem, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Role, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder,
};
use gpui_base::input::InputState;
use junto_client_core::protocol::ids::ServerId;

use super::account::Surface;
use super::admin::{group_label, line, named, sentence};
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::linking::{
    ApprovalLine, ApprovalShown, FirstDeviceView, FirstStep, LinkAllView, LinkLine, LinkRole,
    LinkShown, LinkWait, NewDeviceView, NewStep, fits,
};
use crate::state::view::{SelectedServer, ServerStatus};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, ButtonKind, Field, FieldEvent, Notice, StatePanel, StyleExt, icon};

/// The most characters of a link text that its box draws. "Copy" takes the whole text.
const SHOWN_CHARACTERS: usize = 480;

/// A field of the layer of the link on all servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkField {
    Offer,
    Request,
}

/// What the layer of the link on all servers holds of the window itself.
pub(crate) struct LinkingUi {
    /// The offer of the first device, on the new device.
    pub(crate) offer: Entity<InputState>,
    /// The name of this device, on the new device.
    pub(crate) device_name: Entity<InputState>,
    /// The request of the new device, on the first device.
    pub(crate) request: Entity<InputState>,
    /// Why the text of a field was not sent.
    pub(crate) invalid: Option<&'static str>,
    /// The text of the layer was given to the clipboard.
    pub(crate) copied: bool,
    pub(crate) scroll: ScrollHandle,
    /// What the layer drew last. A test reads the layer from it.
    #[cfg(feature = "test-support")]
    pub(crate) drawn: Option<LinkAllView>,
}

impl LinkingUi {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let offer = Field::state("", window, cx);
        let device_name = Field::state(text::account::THIS_DEVICE_NAME, window, cx);
        let request = Field::state("", window, cx);
        let subscriptions = vec![
            Field::observe(
                &offer,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => this.link_text_changed(LinkField::Offer, window, cx),
                    FieldEvent::Submitted => this.submit_link_offer(window, cx),
                },
            ),
            Field::observe(
                &device_name,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => cx.notify(),
                    FieldEvent::Submitted => this.submit_link_offer(window, cx),
                },
            ),
            Field::observe(
                &request,
                window,
                cx,
                |this: &mut Shell, event, window, cx| match event {
                    FieldEvent::Changed => this.link_text_changed(LinkField::Request, window, cx),
                    FieldEvent::Submitted => this.submit_link_request(window, cx),
                },
            ),
        ];
        let ui = Self {
            offer,
            device_name,
            request,
            invalid: None,
            copied: false,
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

/// What a device that waits for its link shows: the code, the key of this device, the
/// expiry, and what the person does with them.
pub(crate) fn link_wait_block(wait: &LinkWait, centered: bool, theme: &Theme) -> gpui::Div {
    let code: AnyElement = match &wait.code {
        Some(code) => mono_value(
            code.expose().to_owned(),
            text::account::LINK_CODE,
            "link-wait-code",
            true,
            theme,
        )
        .into_any_element(),
        None => Notice::error(text::account::LINK_CODE_NOT_SHOWN)
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
        .child(mono_value(
            wait.fingerprint.clone(),
            text::account::KEY_OF_THIS_DEVICE,
            "link-wait-fingerprint",
            false,
            theme,
        ))
        .child(
            sentence(text::account::code_expires(wait.expires_at), theme)
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

/// A long text that the person carries to the other device: its start in a frame, and
/// its length.
fn text_box(
    text: &str,
    label: &'static str,
    selector: &'static str,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let characters = text.chars().count();
    let shown: String = if characters > SHOWN_CHARACTERS {
        text.chars().take(SHOWN_CHARACTERS).chain(['…']).collect()
    } else {
        text.to_owned()
    };
    div()
        .flex()
        .flex_col()
        .flex_none()
        .child(
            div()
                .id(selector)
                .role(Role::Label)
                .aria_label(label)
                .debug_selector(move || selector.to_owned())
                .w_full()
                .min_w_0()
                .py(u(8.0))
                .px(u(10.0))
                .rounded(u(theme.metrics.radius))
                .border_1()
                .border_color(Hsla::from(theme.colors.rule))
                .bg(Hsla::from(theme.colors.travertine_hi))
                .typeface(theme.types.mono_small, theme)
                .ink(theme.colors.ink)
                .child(shown),
        )
        .child(
            sentence(text::account::text_length(characters), theme)
                .mt(u(4.0))
                .debug_selector(move || format!("{selector}-length")),
        )
}

/// The heading of a step of the layer.
fn step_title(title: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .mt(u(14.0))
        .mb(u(3.0))
        .typeface(theme.types.heading, theme)
        .ink(theme.colors.ink)
        .child(title)
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
    progress_named(text, None, mark, theme)
}

/// The state of one server with a name that a server gave. The name has a line of its
/// own under the state, cut at the width of the state: it is no part of the sentence.
fn progress_named(
    text: impl Into<SharedString>,
    named: Option<String>,
    mark: Option<Icon>,
    theme: &Theme,
) -> gpui::Div {
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
        .child(
            div()
                .min_w_0()
                .child(text.into())
                .when_some(named, |state, named| {
                    state.child(div().truncate().child(named))
                }),
        )
}

impl Shell {
    /// The device-link scroll position used by the window tests.
    #[cfg(feature = "test-support")]
    pub fn linking_scroll_handle(&self) -> ScrollHandle {
        self.linking.scroll.clone()
    }

    /// Opens the link on all servers in one role.
    pub fn open_link_all(&mut self, role: LinkRole, window: &mut Window, cx: &mut Context<Self>) {
        self.reset_linking_ui(window, cx);
        self.state
            .update(cx, |state, cx| state.open_link_all(role, cx));
        if role == LinkRole::NewDevice {
            self.linking.device_name.update(cx, |field, cx| {
                field.set_value(text::device_name(), window, cx);
            });
        }
        match self.layer {
            Layer::LinkAll => {}
            Layer::None => self.open_layer(Layer::LinkAll, window, cx),
            // The layer takes the place of the settings that opened it.
            _ => {
                self.layer = Layer::LinkAll;
                self.layer_focus().clone().focus(window, cx);
            }
        }
        cx.notify();
    }

    /// The fields of the layer are emptied: a text with link codes is not kept in a
    /// field that nobody sees.
    pub(crate) fn reset_linking_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.linking.invalid = None;
        self.linking.copied = false;
        #[cfg(feature = "test-support")]
        {
            self.linking.drawn = None;
        }
        for field in [
            self.linking.offer.clone(),
            self.linking.device_name.clone(),
            self.linking.request.clone(),
        ] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    /// A field for a link text takes at most 32 KiB (ADR 0716): a longer text is not
    /// kept in the field.
    fn link_text_changed(&mut self, which: LinkField, window: &mut Window, cx: &mut Context<Self>) {
        let field = match which {
            LinkField::Offer => self.linking.offer.clone(),
            LinkField::Request => self.linking.request.clone(),
        };
        let (too_long, empty) = {
            let value = field.read(cx).value();
            (!fits(&value), value.is_empty())
        };
        if too_long {
            field.update(cx, |field, cx| field.set_value("", window, cx));
            self.linking.invalid = Some(text::account::TEXT_TOO_LONG);
        } else if !empty {
            self.linking.invalid = None;
        }
        cx.notify();
    }

    /// Puts text into the field for the offer of the first device, as pasting does.
    pub fn type_link_offer(&mut self, offer: &str, window: &mut Window, cx: &mut Context<Self>) {
        let offer = offer.to_owned();
        self.linking
            .offer
            .update(cx, |field, cx| field.set_value(offer, window, cx));
        self.link_text_changed(LinkField::Offer, window, cx);
    }

    /// Puts text into the field for the request of the new device, as pasting does.
    pub fn type_link_request(
        &mut self,
        request: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = request.to_owned();
        self.linking
            .request
            .update(cx, |field, cx| field.set_value(request, window, cx));
        self.link_text_changed(LinkField::Request, window, cx);
    }

    /// True when the two fields for link texts hold no text.
    pub fn link_fields_are_empty(&self, cx: &gpui::App) -> bool {
        self.linking.offer.read(cx).value().is_empty()
            && self.linking.request.read(cx).value().is_empty()
    }

    /// Why the text of a field of the layer was not sent.
    pub fn link_text_refusal(&self) -> Option<&'static str> {
        self.linking.invalid
    }

    /// The new device takes the typed offer and asks each server for a link. A second
    /// start while the first one runs is not sent.
    pub fn submit_link_offer(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let enters = matches!(
            self.state.read(cx).link_all_view(),
            Some(LinkAllView::New(NewDeviceView {
                step: NewStep::EnterOffer,
                busy: false,
                ..
            }))
        );
        if !enters {
            return;
        }
        let offer = self.linking.offer.read(cx).value().trim().to_owned();
        if offer.is_empty() {
            self.linking.invalid = Some(text::account::OFFER_EMPTY);
            cx.notify();
            return;
        }
        let device_name = self.linking.device_name.read(cx).value().to_string();
        self.linking.invalid = None;
        self.linking.copied = false;
        self.state
            .update(cx, |state, cx| state.start_linking(offer, device_name, cx));
    }

    /// The first device takes the typed request and looks each link up. Nothing is
    /// approved.
    pub fn submit_link_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let enters = matches!(
            self.state.read(cx).link_all_view(),
            Some(LinkAllView::First(FirstDeviceView {
                step: FirstStep::EnterRequest,
                busy: false,
                ..
            }))
        );
        if !enters {
            return;
        }
        let request = self.linking.request.read(cx).value().trim().to_owned();
        if request.is_empty() {
            self.linking.invalid = Some(text::account::REQUEST_EMPTY);
            cx.notify();
            return;
        }
        self.linking.invalid = None;
        // The flow holds the request from here on; the field does not keep its codes.
        self.linking
            .request
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.state
            .update(cx, |state, cx| state.look_up_request(request, cx));
    }

    /// What the layer of the link on all servers drew last.
    #[cfg(feature = "test-support")]
    pub fn link_all_drawn(&self) -> Option<&LinkAllView> {
        self.linking.drawn.as_ref()
    }

    /// The button that gives a text to the clipboard. The clipboard is written by the
    /// click and at no other moment.
    fn copy_button(
        &mut self,
        text: String,
        selector: &'static str,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(selector)
            .label(if self.linking.copied {
                text::account::COPIED
            } else {
                text::account::COPY
            })
            .selector(selector)
            .h(u(28.0))
            .px(u(10.0))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                this.linking.copied = true;
                cx.notify();
            }))
    }

    fn start_over(&mut self, cx: &mut Context<Self>) -> Button {
        Button::new("link-all-reset")
            .label(text::account::START_OVER)
            .selector("link-all-reset")
            .h(u(30.0))
            .px(u(12.0))
            .on_click(cx.listener(|this, _, window, cx| {
                this.reset_linking_ui(window, cx);
                let role = this.state.read(cx).link_all_role();
                if let Some(role) = role {
                    this.open_link_all(role, window, cx);
                }
            }))
    }

    fn render_approval_line(
        &mut self,
        row: &ApprovalLine,
        busy: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id: ServerId = row.server_id;
        let account = match &row.account {
            Some(account) => text::account::account_line(account),
            None => text::account::NO_ACCOUNT_SHOWN.to_owned(),
        };
        let state = match &row.progress {
            ApprovalShown::Working => progress(text::account::PROGRESS_WORKING, None, theme),
            ApprovalShown::Ready => progress(text::account::PROGRESS_READY, None, theme),
            ApprovalShown::Approved => {
                progress(text::account::PROGRESS_APPROVED, Some(Icon::Check), theme)
            }
            ApprovalShown::Failed(reason) => progress(
                text::account::progress_failed(reason),
                Some(Icon::Warning),
                theme,
            ),
            ApprovalShown::Unknown => {
                progress(text::account::PROGRESS_UNKNOWN, Some(Icon::Warning), theme)
            }
            ApprovalShown::AccountChanged(now) => progress_named(
                text::account::ACCOUNT_CHANGED,
                Some(text::account::account_now(now)),
                Some(Icon::Warning),
                theme,
            ),
        };
        let content = line(theme)
            .child(server_text(&row.server, Some(account), theme))
            .child(
                crate::ui::notice_slot((
                    "approval-progress",
                    row.server_id.as_uuid().as_u128() as u64,
                ))
                .w(u(330.0))
                .child(state),
            )
            .when(
                matches!(row.progress, ApprovalShown::AccountChanged(_)),
                |content| {
                    content.child(named(
                        format!("use-account-{server_id}"),
                        Button::new(("use-account", server_id.as_uuid().as_u128() as u64))
                            .label(text::account::USE_THIS_ACCOUNT)
                            .disabled(busy)
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.use_account(server_id, cx));
                            })),
                    ))
                },
            );
        named(format!("approval-line-{server_id}"), content)
            .flex_1()
            .into_any_element()
    }

    fn render_first_device(
        &mut self,
        view: &FirstDeviceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut column = div()
            .flex()
            .flex_col()
            .child(step_title(text::account::OFFER_TITLE, theme))
            .child(sentence(text::account::OFFER_HELP, theme));
        column = match &view.offer {
            Some(offer) => column
                .child(div().flex_none().mt(u(8.0)).child(text_box(
                    offer,
                    text::account::OFFER_LABEL,
                    "link-offer-text",
                    theme,
                )))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(10.0))
                        .mt(u(6.0))
                        .child(self.copy_button(offer.clone(), "link-offer-copy", cx))
                        .child(
                            sentence(text::account::offer_names(&view.offered), theme)
                                .flex_1()
                                .min_w_0()
                                .truncate(),
                        ),
                ),
            None => column.child(
                div()
                    .flex_none()
                    .mt(u(8.0))
                    .child(Notice::new(text::account::NO_OFFER).selector("link-no-offer")),
            ),
        };
        match &view.step {
            FirstStep::EnterRequest => {
                column = column
                    .child(step_title(text::account::REQUEST_TITLE, theme))
                    .child(sentence(text::account::REQUEST_HELP, theme))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(u(8.0))
                            .mt(u(8.0))
                            .child(
                                div().flex_1().min_w_0().child(
                                    Field::new(&self.linking.request, text::account::REQUEST_FIELD)
                                        .selector("link-request-field"),
                                ),
                            )
                            .child(
                                Button::new("link-request-look-up")
                                    .kind(ButtonKind::Primary)
                                    .label(if view.busy {
                                        text::account::LOOKING_UP
                                    } else {
                                        text::account::LOOK_UP
                                    })
                                    .selector("link-request-look-up")
                                    .disabled(view.busy || view.offer.is_none())
                                    .h(u(32.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_link_request(window, cx);
                                    })),
                            ),
                    )
                    .children(failure(self.linking.invalid, "link-text-invalid"))
                    .children(failure(view.error.as_deref(), "link-all-error"));
            }
            FirstStep::Confirm {
                fingerprint,
                device_name,
                lines,
                can_approve,
                all_approved,
            } => {
                column = column
                    .child(step_title(text::account::CONFIRM_TITLE, theme))
                    .child(sentence(text::account::CONFIRM_HELP, theme))
                    .child(value_label(text::account::FINGERPRINT, theme).mt(u(8.0)))
                    .child(mono_value(
                        fingerprint.clone(),
                        text::account::FINGERPRINT,
                        "link-all-fingerprint",
                        true,
                        theme,
                    ))
                    // The name comes from the request: it has a line of its own that is
                    // cut at the width of the page.
                    .child(
                        sentence(text::account::new_device_named(device_name), theme)
                            .truncate()
                            .mt(u(4.0)),
                    )
                    .child(group_label(text::account::SERVERS, theme));
                for row in lines {
                    column = column.child(self.render_approval_line(row, view.busy, theme, cx));
                }
                column = column
                    .children(failure(view.error.as_deref(), "link-all-error"))
                    .child(crate::ui::notice_slot("link-all-approved-slot").children(
                        all_approved.then(|| {
                            Notice::new(text::account::ALL_APPROVED)
                                .icon(Icon::Check)
                                .selector("link-all-approved")
                        }),
                    ))
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
                                    .disabled(!*can_approve)
                                    .h(u(30.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state
                                            .update(cx, |state, cx| state.approve_devices(cx));
                                    })),
                            ),
                    );
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
            NewStep::EnterOffer => column
                .child(step_title(text::account::OFFER_FIELD_TITLE, theme))
                .child(sentence(text::account::OFFER_FIELD_HELP, theme))
                .child(
                    div().flex_none().mt(u(8.0)).child(
                        Field::new(&self.linking.offer, text::account::OFFER_FIELD)
                            .selector("link-offer-field"),
                    ),
                )
                .child(value_label(text::account::THIS_DEVICE_NAME, theme).mt(u(10.0)))
                .child(
                    div().flex_none().mt(u(3.0)).child(
                        Field::new(&self.linking.device_name, text::account::THIS_DEVICE_NAME)
                            .selector("link-device-name-field"),
                    ),
                )
                .child(sentence(text::account::DEVICE_NAME_HELP, theme).mt(u(5.0)))
                .children(failure(self.linking.invalid, "link-text-invalid"))
                .children(failure(view.error.as_deref(), "link-all-error"))
                .child(
                    div().flex().flex_none().mt(u(12.0)).child(
                        Button::new("link-start")
                            .kind(ButtonKind::Primary)
                            .label(if view.busy {
                                text::account::REQUESTING_LINKS
                            } else {
                                text::account::REQUEST_LINKS
                            })
                            .selector("link-start")
                            .disabled(view.busy)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_link_offer(window, cx);
                            })),
                    ),
                )
                .into_any_element(),
            NewStep::Requested {
                device_name: _,
                request,
                fingerprint,
                lines,
                all_linked,
            } => {
                let mut column = column
                    .child(step_title(text::account::REQUEST_TEXT_TITLE, theme))
                    .child(sentence(text::account::REQUEST_TEXT_HELP, theme));
                let result = crate::ui::notice_slot("link-request-result").h(u(256.0));
                let result = match (request, fingerprint) {
                    (Some(request), Some(fingerprint)) => result
                        .child(div().flex_none().mt(u(8.0)).child(text_box(
                            request.expose(),
                            text::account::REQUEST_LABEL,
                            "link-request-text",
                            theme,
                        )))
                        .child(div().flex().flex_none().mt(u(6.0)).child(self.copy_button(
                            request.expose().to_owned(),
                            "link-request-copy",
                            cx,
                        )))
                        .child(value_label(text::account::FINGERPRINT, theme).mt(u(8.0)))
                        .child(mono_value(
                            fingerprint.clone(),
                            text::account::FINGERPRINT,
                            "link-all-fingerprint",
                            true,
                            theme,
                        )),
                    _ if *all_linked => result.child(
                        div().flex_none().mt(u(8.0)).child(
                            Notice::new(text::account::ALL_LINKED)
                                .icon(Icon::Check)
                                .selector("link-all-linked"),
                        ),
                    ),
                    _ => result.child(
                        sentence(text::account::NO_REQUEST, theme)
                            .mt(u(8.0))
                            .debug_selector(|| "link-no-request".to_owned()),
                    ),
                };
                column = column.child(result);
                column = column.child(group_label(text::account::SERVERS, theme));
                for row in lines {
                    column = column.child(Self::render_link_line(row, theme));
                }
                column
                    .children(failure(view.error.as_deref(), "link-all-error"))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .gap(u(8.0))
                            .mt(u(12.0))
                            .child(self.start_over(cx))
                            .child(
                                Button::new("link-retry")
                                    .icon(Icon::Retry)
                                    .label(text::TRY_AGAIN)
                                    .selector("link-retry")
                                    .disabled(view.busy || *all_linked)
                                    .h(u(30.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.linking.copied = false;
                                        this.state.update(cx, |state, cx| state.retry_links(cx));
                                    })),
                            )
                            .child(
                                Button::new("link-check-all")
                                    .kind(ButtonKind::Primary)
                                    .label(if view.busy {
                                        text::account::CHECKING
                                    } else {
                                        text::account::CHECK
                                    })
                                    .selector("link-check-all")
                                    .disabled(view.busy || *all_linked)
                                    .h(u(30.0))
                                    .px(u(12.0))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.update(cx, |state, cx| state.check_links(cx));
                                    })),
                            ),
                    )
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

    /// The section "Devices" of the settings: the two ways into the link on all servers.
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
                    this.open_link_all(role, window, cx);
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

    /// The third way of the join dialog: a device of an account that exists asks for a
    /// link code.
    pub(crate) fn render_link_way(
        &mut self,
        busy: bool,
        requesting: bool,
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
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
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
                            .child(text::account::LINK_THIS_DEVICE_HELP),
                    ),
            )
            .child(
                Button::new("request-link")
                    .icon(Icon::Link)
                    .label(if requesting {
                        text::account::REQUESTING_LINK
                    } else {
                        text::account::REQUEST_LINK
                    })
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
                link_wait_block(&wait, true, &theme).mt(u(12.0)),
            )
            .icon(Icon::Clock)
            .selector("waiting-for-link")
            .error(error.map(SharedString::from))
            .action(new_code)
            .action(cancel),
        )
    }
}
