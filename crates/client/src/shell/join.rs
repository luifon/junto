//! Joining a server (ADR 0105, ADR 0732). The address is typed in the server book; this
//! dialog shows the server that answered and the two ways in: a code, or a request that an
//! administrator approves. The dialog draws the states of the flows of the client core and
//! holds no rule of joining.

use gpui::{
    AnyElement, Context, Hsla, IntoElement, ParentElement, SharedString, Styled, Window, div,
    prelude::FluentBuilder,
};
use junto_client_core::book::ConnectionMode;
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::AccessAction;
use junto_client_core::flow::add_server::{AddServerAction, AddServerFlow, AddServerState};
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::protocol::secret::{InviteCode, OwnerCode};
use junto_client_core::version::Incompatibility;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::view::{ServerStatus, ShellView, shown_address};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Chip, Dialog, Field, FieldKind, Mosaic, MosaicKind, Notice, StyleExt,
    WaitBox, dialog_footer,
};

/// The number of symbols of an owner code. An invite code has 16 (ADR 0110).
const OWNER_CODE_SYMBOLS: usize = 20;

/// The server that the dialog is about.
#[derive(Clone)]
pub(crate) enum Target {
    /// The address answered and nothing is saved yet.
    Preview(AddServerFlow),
    /// A server of the server book on which this device has no account.
    Saved(ServerId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Running {
    Joining,
    Requesting,
    /// A link code for this device is asked for.
    Linking,
    Saving,
    Discarding,
}

pub(crate) struct Join {
    /// Tells this dialog from every other one that the window opened. An action that ends
    /// after its dialog was closed changes no later dialog.
    pub(crate) opened: u64,
    pub(crate) target: Target,
    pub(crate) running: Option<Running>,
    pub(crate) error: Option<String>,
    /// The text of the "Add server" field that the preview was asked for with.
    typed: Option<String>,
}

impl Join {
    pub(crate) fn preview(opened: u64, flow: AddServerFlow, typed: String) -> Self {
        Self {
            opened,
            target: Target::Preview(flow),
            running: None,
            error: None,
            typed: Some(typed),
        }
    }

    pub(crate) fn saved(opened: u64, server_id: ServerId) -> Self {
        Self {
            opened,
            target: Target::Saved(server_id),
            running: None,
            error: None,
            typed: None,
        }
    }

    /// True when the dialog has nothing to show any more: its server left the server book,
    /// the preview was given up, or this device has its account now.
    pub(crate) fn is_stale(&self, view: &ShellView) -> bool {
        match &self.target {
            Target::Preview(flow) => matches!(flow.view().state, AddServerState::EnterAddress),
            Target::Saved(server_id) => {
                match view.servers.iter().find(|row| row.id == *server_id) {
                    None => true,
                    Some(row) => matches!(
                        row.status,
                        ServerStatus::Ready
                            | ServerStatus::Standby
                            | ServerStatus::Connecting
                            | ServerStatus::Reconnecting
                            | ServerStatus::LoggedOut
                            | ServerStatus::LoggedOutByPerson
                    ),
                }
            }
        }
    }
}

/// What the dialog shows of its server.
struct Card {
    seed: u128,
    name: SharedString,
    address: SharedString,
}

/// The body of the dialog.
enum Body {
    /// The two ways in.
    Paths {
        found: bool,
    },
    /// The server and this build share no API version. It can still be saved.
    Incompatible(Incompatibility),
    KeyOfOtherAddress(String),
    Waiting {
        name: String,
    },
    /// This device waits for its link: the code and the key of this device.
    LinkWait(ServerId, crate::state::linking::LinkWait),
    /// Another state of a saved server, as a sentence.
    Statement(&'static str, String),
}

impl Shell {
    pub fn open_join_for(
        &mut self,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.code
            .update(cx, |code, cx| code.set_value("", window, cx));
        let opened = self.next_join();
        self.open_layer(Layer::Join(Join::saved(opened, server_id)), window, cx);
        self.focus_field(&self.code.clone(), window, cx);
    }

    /// The mark of the next join dialog.
    pub(crate) fn next_join(&mut self) -> u64 {
        self.joins_opened += 1;
        self.joins_opened
    }

    /// The join dialog that an action was started from, while it is still the open one.
    fn join_opened(&mut self, opened: u64) -> Option<&mut Join> {
        match &mut self.layer {
            Layer::Join(join) if join.opened == opened => Some(join),
            _ => None,
        }
    }

    pub(crate) fn code_changed(&mut self, cx: &mut Context<Self>) {
        if let Layer::Join(join) = &mut self.layer {
            join.error = None;
        }
        cx.notify();
    }

    pub(crate) fn join_with_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.code.read(cx).value().to_string();
        let symbols = code.chars().filter(char::is_ascii_alphanumeric).count();
        let name = self.state.read(cx).preset_name().to_owned();
        let Layer::Join(join) = &mut self.layer else {
            return;
        };
        if join.running.is_some() {
            return;
        }
        if symbols == 0 {
            join.error = Some(text::CODE_MISSING.to_owned());
            cx.notify();
            return;
        }
        let device_name = text::device_name();
        // The two codes differ in their length, so one field takes both.
        let action = if symbols == OWNER_CODE_SYMBOLS {
            AccessAction::ClaimOwner {
                code: OwnerCode::new(code),
                name,
                device_name,
            }
        } else {
            AccessAction::Join {
                code: InviteCode::new(code),
                name,
                device_name,
            }
        };
        self.run_access(Running::Joining, action, window, cx);
    }

    pub(crate) fn request_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.state.read(cx).preset_name().to_owned();
        self.run_access(
            Running::Requesting,
            AccessAction::RequestAccess { name },
            window,
            cx,
        );
    }

    /// The third way in: this device asks for a link code that a logged-in device of the
    /// account approves (ADR 0112). A second request replaces the code of the first.
    pub(crate) fn request_link_in_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run_access(
            Running::Linking,
            AccessAction::RequestLink {
                device_name: text::device_name(),
            },
            window,
            cx,
        );
    }

    /// Saves the previewed server when it is not saved yet, then runs one action of its
    /// access flow. The server is in the server book from the moment the person chooses a
    /// way in.
    fn run_access(
        &mut self,
        running: Running,
        action: AccessAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let client = self.state.read(cx).client().clone();
        let Layer::Join(join) = &mut self.layer else {
            return;
        };
        if join.running.is_some() {
            return;
        }
        join.running = Some(running);
        join.error = None;
        let (opened, target, typed) = (join.opened, join.target.clone(), join.typed.clone());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let saved = match target {
                Target::Saved(server_id) => Ok(server_id),
                Target::Preview(flow) => save(&flow).await,
            };
            let result = match saved {
                Err(error) => Err(error),
                Ok(server_id) => {
                    let shown = this.update_in(cx, |this, window, cx| {
                        if let Some(join) = this.join_opened(opened) {
                            join.target = Target::Saved(server_id);
                        }
                        this.server_added(typed.as_deref(), window, cx);
                        this.state
                            .update(cx, |state, cx| state.select_server(server_id, cx));
                    });
                    match (shown, client.server(server_id)) {
                        (Ok(()), Some(server)) => server.dispatch_when_free(action).await,
                        _ => Err(FlowError::UnknownServer),
                    }
                }
            };
            let _ = this.update_in(cx, |this, _, cx| {
                this.join_action_ended(opened, result, cx);
            });
        })
        .detach();
    }

    /// An action of a join dialog has ended. Its result is shown in that dialog only: when
    /// the dialog was closed meanwhile, the state of the server shows the result.
    fn join_action_ended(
        &mut self,
        opened: u64,
        result: Result<(), FlowError>,
        cx: &mut Context<Self>,
    ) {
        self.join_results += 1;
        if let Some(join) = self.join_opened(opened) {
            join.running = None;
            join.error = result.err().map(|error| text::flow_error(&error));
        }
        cx.notify();
    }

    /// Runs an action of the flow that adds the server.
    fn run_add(
        &mut self,
        running: Running,
        action: AddServerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Layer::Join(join) = &mut self.layer else {
            return;
        };
        let (Target::Preview(flow), None) = (join.target.clone(), join.running) else {
            return;
        };
        join.running = Some(running);
        join.error = None;
        let (opened, typed) = (join.opened, join.typed.clone());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = flow.dispatch(action).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let AddServerState::Saved { server_id } = flow.view().state {
                    this.server_added(typed.as_deref(), window, cx);
                    this.state
                        .update(cx, |state, cx| state.select_server(server_id, cx));
                    if this.join_opened(opened).is_some() {
                        this.close_layer(window, cx);
                    }
                    return;
                }
                this.join_action_ended(opened, result, cx);
            });
        })
        .detach();
    }

    /// The previewed server is in the server book now. The "Add server" field is emptied
    /// when it still holds the text that the preview was asked for with; a text that the
    /// person typed since then stays.
    fn server_added(&mut self, typed: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(typed) = typed else {
            return;
        };
        if self.address.read(cx).value() == typed {
            self.address
                .update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    /// Gives up the request that the dialog shows. The state of the application carries
    /// the work and its result; the dialog closes when the server has left the book.
    fn cancel_request_in_dialog(&mut self, cx: &mut Context<Self>) {
        let Layer::Join(join) = &mut self.layer else {
            return;
        };
        let (Target::Saved(server_id), None) = (join.target.clone(), join.running) else {
            return;
        };
        join.error = None;
        self.state
            .update(cx, |state, cx| state.cancel_request(server_id, cx));
        cx.notify();
    }

    pub(crate) fn render_join(
        &mut self,
        view: &ShellView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let Layer::Join(join) = &self.layer else {
            return div().into_any_element();
        };
        let running = join.running;
        let mut error = join.error.clone();
        let pending = self.state.read(cx).pending().clone();
        let mut cancelling = false;
        let (card, body) = match &join.target {
            Target::Preview(flow) => match flow.view().state {
                AddServerState::Preview(preview) => (
                    Card {
                        seed: preview.server_id.as_uuid().as_u128(),
                        name: preview.name.clone().into(),
                        address: shown_address(preview.address.as_str()).into(),
                    },
                    match preview.version {
                        Ok(_) => Body::Paths { found: true },
                        Err(reason) => Body::Incompatible(reason),
                    },
                ),
                AddServerState::KeyOfOtherAddress {
                    preview,
                    key_address,
                } => (
                    Card {
                        seed: preview.server_id.as_uuid().as_u128(),
                        name: preview.name.clone().into(),
                        address: shown_address(preview.address.as_str()).into(),
                    },
                    Body::KeyOfOtherAddress(shown_address(key_address.as_str())),
                ),
                AddServerState::EnterAddress | AddServerState::Saved { .. } => {
                    return div().into_any_element();
                }
            },
            Target::Saved(server_id) => {
                let Some(row) = view.servers.iter().find(|row| row.id == *server_id) else {
                    return div().into_any_element();
                };
                let flow_error = view
                    .selected
                    .as_ref()
                    .filter(|selected| selected.id == *server_id)
                    .and_then(|selected| selected.error.as_ref())
                    .map(text::flow_error);
                cancelling = pending.cancelling.contains(server_id);
                if error.is_none() && running.is_none() && !cancelling {
                    error = pending.remove_failed.get(server_id).cloned().or(flow_error);
                }
                let body = match &row.status {
                    ServerStatus::WaitingForApproval { name } => {
                        Body::Waiting { name: name.clone() }
                    }
                    ServerStatus::WaitingForLink { .. } => {
                        match self.state.read(cx).link_wait(*server_id) {
                            Some(wait) => Body::LinkWait(*server_id, wait),
                            None => Body::Paths { found: false },
                        }
                    }
                    ServerStatus::NoAccount(_) => Body::Paths { found: false },
                    ServerStatus::Banned => {
                        Body::Statement(text::BANNED_TITLE, text::BANNED.to_owned())
                    }
                    ServerStatus::Unreachable => {
                        Body::Statement(text::UNREACHABLE_TITLE, text::UNREACHABLE.to_owned())
                    }
                    ServerStatus::Incompatible(reason) => {
                        let (title, detail) = text::incompatibility(*reason);
                        Body::Statement(title, detail)
                    }
                    ServerStatus::OtherServer => {
                        Body::Statement(text::OTHER_SERVER_TITLE, text::OTHER_SERVER.to_owned())
                    }
                    ServerStatus::Ready
                    | ServerStatus::Standby
                    | ServerStatus::Connecting
                    | ServerStatus::Reconnecting
                    | ServerStatus::LoggedOut
                    | ServerStatus::LoggedOutByPerson => return div().into_any_element(),
                };
                (
                    Card {
                        seed: row.id.as_uuid().as_u128(),
                        name: row.name.clone().into(),
                        address: row.address.clone().into(),
                    },
                    body,
                )
            }
        };

        let server_name = card.name.clone();
        let found = matches!(body, Body::Paths { found: true });
        let mut dialog = Dialog::new(text::JOIN_A_SERVER, self.layer_focus())
            .selector("join-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            .child(server_card(card, found, &theme))
            .child(
                div()
                    .mt(u(10.0))
                    .child(Notice::new(text::OPERATOR_NOTICE).selector("operator-notice")),
            );
        dialog = dialog.child(
            crate::ui::notice_slot("join-error-slot")
                .children(error.map(|error| Notice::error(error).selector("join-error"))),
        );
        let busy = running.is_some() || cancelling;
        match body {
            Body::Paths { .. } => {
                dialog = dialog.child(
                    div()
                        .flex()
                        .gap(u(10.0))
                        .mt(u(12.0))
                        .child(path(
                            text::HAVE_A_CODE,
                            text::HAVE_A_CODE_HELP.into(),
                            vec![
                                Field::new(&self.code, text::CODE_FIELD)
                                    .kind(FieldKind::Code)
                                    .selector("code-field")
                                    .into_any_element(),
                                Button::new("join-with-code")
                                    .kind(ButtonKind::Primary)
                                    .label(if running == Some(Running::Joining) {
                                        text::JOINING
                                    } else {
                                        text::JOIN_WITH_CODE
                                    })
                                    .disabled(busy)
                                    .selector("join-with-code")
                                    .w_full()
                                    .h(u(30.0))
                                    .mt(u(8.0))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.join_with_code(window, cx);
                                    }))
                                    .into_any_element(),
                            ],
                            &theme,
                        ))
                        .child(path(
                            text::REQUEST_ACCESS,
                            text::request_access_help(&server_name).into(),
                            vec![
                                Button::new("request-access")
                                    .label(if running == Some(Running::Requesting) {
                                        text::REQUESTING
                                    } else {
                                        text::REQUEST_ACCESS
                                    })
                                    .disabled(busy)
                                    .selector("request-access")
                                    .w_full()
                                    .h(u(30.0))
                                    .mt(u(8.0))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.request_access(window, cx);
                                    }))
                                    .into_any_element(),
                            ],
                            &theme,
                        )),
                );
                dialog = dialog.child(self.render_link_way(
                    busy,
                    running == Some(Running::Linking),
                    &theme,
                    cx,
                ));
            }
            Body::LinkWait(server_id, wait) => {
                let [cancel, new_code] = self.link_wait_actions(server_id, busy, cx);
                dialog = dialog
                    .child(
                        div()
                            .mt(u(12.0))
                            .child(crate::shell::linking::link_wait_block(&wait, false, &theme)),
                    )
                    .child(dialog_footer([
                        cancel.into_any_element(),
                        new_code.into_any_element(),
                        Button::new("close-join")
                            .kind(ButtonKind::Primary)
                            .label(text::CLOSE)
                            .selector("close-join")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_layer(window, cx);
                            }))
                            .into_any_element(),
                    ]));
            }
            Body::Incompatible(reason) => {
                let (title, detail) = text::incompatibility(reason);
                dialog = dialog
                    .child(
                        div().mt(u(10.0)).child(
                            Notice::new(format!("{title}. {detail}"))
                                .icon(Icon::Warning)
                                .selector("join-incompatible"),
                        ),
                    )
                    .child(dialog_footer([
                        Button::new("cancel-join")
                            .label(text::CANCEL)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_layer(window, cx);
                            }))
                            .into_any_element(),
                        Button::new("add-to-server-book")
                            .kind(ButtonKind::Primary)
                            .label(if running == Some(Running::Saving) {
                                text::SAVING
                            } else {
                                text::ADD_TO_SERVER_BOOK
                            })
                            .disabled(busy)
                            .selector("add-to-server-book")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run_add(
                                    Running::Saving,
                                    AddServerAction::Save {
                                        connection_mode: ConnectionMode::Always,
                                    },
                                    window,
                                    cx,
                                );
                            }))
                            .into_any_element(),
                    ]));
            }
            Body::KeyOfOtherAddress(stored) => {
                dialog = dialog
                    .child(
                        div().mt(u(10.0)).child(
                            Notice::new(format!(
                                "{}. {}",
                                text::KEY_OF_OTHER_ADDRESS_TITLE,
                                text::key_of_other_address(&stored)
                            ))
                            .icon(Icon::Warning)
                            .selector("join-key-of-other-address"),
                        ),
                    )
                    .child(dialog_footer([
                        Button::new("cancel-join")
                            .label(text::CANCEL)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_layer(window, cx);
                            }))
                            .into_any_element(),
                        Button::new("discard-key")
                            .kind(ButtonKind::Primary)
                            .label(text::DISCARD_KEY)
                            .disabled(busy)
                            .selector("discard-key")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.run_add(
                                    Running::Discarding,
                                    AddServerAction::DiscardKey,
                                    window,
                                    cx,
                                );
                            }))
                            .into_any_element(),
                    ]));
            }
            Body::Waiting { name } => {
                dialog = dialog
                    .child(div().mt(u(12.0)).child(WaitBox::new(
                        text::REQUEST_SENT_TITLE,
                        text::request_sent(&server_name, &name),
                    )))
                    .child(dialog_footer([
                        Button::new("cancel-request")
                            .label(text::CANCEL_REQUEST)
                            .disabled(busy)
                            .selector("cancel-request")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_request_in_dialog(cx);
                            }))
                            .into_any_element(),
                        Button::new("close-join")
                            .kind(ButtonKind::Primary)
                            .label(text::CLOSE)
                            .selector("close-join")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_layer(window, cx);
                            }))
                            .into_any_element(),
                    ]));
            }
            Body::Statement(title, detail) => {
                dialog = dialog
                    .child(
                        div().mt(u(10.0)).child(
                            Notice::new(format!("{title}. {detail}"))
                                .icon(Icon::Warning)
                                .selector("join-statement"),
                        ),
                    )
                    .child(dialog_footer([Button::new("close-join")
                        .kind(ButtonKind::Primary)
                        .label(text::CLOSE)
                        .selector("close-join")
                        .h(u(30.0))
                        .px(u(12.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_layer(window, cx);
                        }))
                        .into_any_element()]));
            }
        }
        dialog.into_any_element()
    }
}

/// Saves the previewed server with the connection mode "always".
async fn save(flow: &AddServerFlow) -> Result<ServerId, FlowError> {
    flow.dispatch(AddServerAction::Save {
        connection_mode: ConnectionMode::Always,
    })
    .await?;
    match flow.view().state {
        AddServerState::Saved { server_id } => Ok(server_id),
        _ => Err(FlowError::NotAvailable),
    }
}

/// The card of the server that the dialog is about.
fn server_card(card: Card, found: bool, theme: &Theme) -> impl IntoElement + use<> {
    let colors = theme.colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(u(12.0))
        .py(u(10.0))
        .px(u(12.0))
        .rounded(u(theme.metrics.radius))
        .border_1()
        .border_color(Hsla::from(colors.rule))
        .bg(Hsla::from(colors.travertine_hi))
        .child(Mosaic::new(MosaicKind::Large, card.seed, 44.0).radius(4.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .typeface(theme.types.heading_large, theme)
                        .ink(colors.ink)
                        .truncate()
                        .child(card.name),
                )
                .child(
                    div()
                        .typeface(theme.types.mono_small, theme)
                        .text_size(u(12.5))
                        .line_height(u(17.0))
                        .ink(colors.ink_2)
                        .truncate()
                        .child(card.address),
                ),
        )
        .when(found, |card| {
            card.child(Chip::new(text::SERVER_FOUND).icon(Icon::Check))
        })
}

/// One of the two ways into a server: a heading, a sentence and its controls.
fn path(
    title: &'static str,
    help: SharedString,
    controls: Vec<AnyElement>,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let colors = theme.colors;
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .pt(u(10.0))
        .px(u(12.0))
        .pb(u(12.0))
        .rounded(u(theme.metrics.radius))
        .border_1()
        .border_color(Hsla::from(colors.rule))
        .bg(Hsla::from(colors.travertine_hi))
        .child(
            div()
                .typeface(theme.types.heading, theme)
                .ink(colors.ink)
                .child(title),
        )
        .child(
            div()
                .flex_1()
                .mt(u(2.0))
                .mb(u(9.0))
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .child(help),
        )
        .children(controls)
}
