//! The window "Add server or workspace" (ADR 0732): one place to add a server by its address
//! or an invite link, and to join further workspaces of a server where this device is signed
//! in with an invite link or code (ADR 0115).

use gpui::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div,
};
use junto_client_core::links::AppLink;
use junto_client_core::protocol::ids::ServerId;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::text::add as txt;
use crate::theme::{theme, u};
use crate::ui::{
    Button, ButtonKind, ChannelRow, Dialog, Field, Notice, SelectBox, StyleExt, Tab, TabRow,
    dialog_footer,
};

/// The tab of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddTab {
    Server,
    Workspace,
}

/// What the open window holds of itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddWindow {
    pub(crate) tab: AddTab,
    /// The server of the tab "Workspace".
    pub(crate) server: Option<ServerId>,
    /// The choices of the server are open under its box.
    pub(crate) choosing: bool,
    /// Why the link or the code of the tab "Workspace" was not used.
    pub(crate) error: Option<&'static str>,
}

impl Shell {
    /// Opens the window at a tab. The tab "Workspace" starts on `server`, or on the selected
    /// server when it is signed in.
    pub fn open_add_window(
        &mut self,
        tab: AddTab,
        server: Option<ServerId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let signed = self.signed_in_servers(cx);
        let selected = self
            .state
            .read(cx)
            .view()
            .selected
            .as_ref()
            .map(|server| server.id);
        let server = server
            .or(selected)
            .filter(|id| signed.iter().any(|(held, _)| held == id))
            .or_else(|| signed.first().map(|(id, _)| *id));
        self.add_code
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.address_error = None;
        self.close_hover_menu(cx);
        self.open_layer(
            Layer::AddServer(AddWindow {
                tab,
                server,
                choosing: false,
                error: None,
            }),
            window,
            cx,
        );
        match tab {
            AddTab::Server => self.focus_field(&self.address.clone(), window, cx),
            AddTab::Workspace => self.focus_field(&self.add_code.clone(), window, cx),
        }
    }

    /// The servers of the server book whose account is signed in on this device, in the order
    /// of the server book: only such an account can redeem a code (ADR 0115).
    pub(crate) fn signed_in_servers(&self, cx: &App) -> Vec<(ServerId, String)> {
        let state = self.state.read(cx);
        state
            .view()
            .servers
            .iter()
            .filter(|row| {
                state
                    .client()
                    .server(row.id)
                    .is_some_and(|server| server.owner().is_some())
            })
            .map(|row| (row.id, row.name.clone()))
            .collect()
    }

    fn add_window(&mut self) -> Option<&mut AddWindow> {
        match &mut self.layer {
            Layer::AddServer(add) => Some(add),
            _ => None,
        }
    }

    fn choose_add_tab(&mut self, tab: AddTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(add) = self.add_window() {
            add.tab = tab;
            add.choosing = false;
            add.error = None;
        }
        match tab {
            AddTab::Server => self.focus_field(&self.address.clone(), window, cx),
            AddTab::Workspace => self.focus_field(&self.add_code.clone(), window, cx),
        }
        cx.notify();
    }

    /// "Join" of the tab "Workspace". A link of a server where this device is signed in
    /// chooses that server; a link of another server continues on the tab "Server" with the
    /// link in its field; a code joins the workspaces of the chosen server. The dialog of the
    /// server then shows what the invite added (ADR 0115).
    pub fn submit_add_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(add) = self.add_window().cloned() else {
            return;
        };
        let typed = self.add_code.read(cx).value().trim().to_owned();
        if typed.is_empty() {
            if let Some(add) = self.add_window() {
                add.error = Some(txt::CODE_EMPTY);
            }
            cx.notify();
            return;
        }
        let signed = self.signed_in_servers(cx);
        let (server_id, code) = match junto_client_core::links::parse_pasted(&typed) {
            Some(AppLink::Invite { server, code }) => {
                let known = signed.iter().map(|(id, _)| *id).find(|id| {
                    self.state
                        .read(cx)
                        .client()
                        .server(*id)
                        .is_some_and(|handle| handle.entry().address == server)
                });
                match known {
                    Some(server_id) => (server_id, code.expose().to_owned()),
                    // The server of the link is not in the server book, or this device is not
                    // signed in there: the tab "Server" adds it with the link.
                    None => {
                        self.add_code
                            .update(cx, |field, cx| field.set_value("", window, cx));
                        self.type_address(&typed, window, cx);
                        self.choose_add_tab(AddTab::Server, window, cx);
                        self.submit_address(window, cx);
                        return;
                    }
                }
            }
            Some(_) => {
                if let Some(add) = self.add_window() {
                    add.error = Some(txt::NOT_AN_INVITE);
                }
                cx.notify();
                return;
            }
            None => match add.server {
                Some(server_id) => (server_id, typed),
                None => {
                    if let Some(add) = self.add_window() {
                        add.error = Some(txt::NO_SERVER);
                    }
                    cx.notify();
                    return;
                }
            },
        };
        self.add_code
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.state
            .update(cx, |state, cx| state.select_server(server_id, cx));
        self.open_join_for(server_id, window, cx);
        self.type_code(&code, window, cx);
        self.join_with_code(window, cx);
    }

    pub(crate) fn render_add_window(
        &mut self,
        add: &AddWindow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let close = Self::handler(cx, |this, window, cx| this.close_layer(window, cx));
        let tabs: Vec<Tab> = [
            (
                AddTab::Server,
                Icon::Plug,
                txt::TAB_SERVER,
                "add-tab-server",
            ),
            (
                AddTab::Workspace,
                Icon::Layers,
                txt::TAB_WORKSPACE,
                "add-tab-workspace",
            ),
        ]
        .into_iter()
        .map(|(tab, icon, label, selector)| {
            Tab::new(selector, icon, label)
                .on(add.tab == tab)
                .selector(selector)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.choose_add_tab(tab, window, cx);
                }))
        })
        .collect();
        let cancel = Button::new("add-cancel")
            .label(crate::text::CANCEL)
            .selector("add-cancel")
            .h(u(30.0))
            .px(u(12.0))
            .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)));
        let mut dialog = Dialog::new(txt::TITLE, self.layer_focus())
            .selector("add-window")
            .on_close(move |window, cx| close(window, cx))
            .child(div().flex_none().mb(u(10.0)).child(TabRow::new(
                "add-tabs",
                txt::WHAT_TO_ADD,
                tabs,
            )));
        match add.tab {
            AddTab::Server => {
                let asking = self.adding.is_some();
                let error = self.address_error.clone().filter(|_| !asking);
                dialog = dialog
                    .child(
                        div()
                            .mb(u(4.0))
                            .typeface(theme.types.label, &theme)
                            .ink(theme.colors.ink_2)
                            .child(txt::ADDRESS_FIELD),
                    )
                    .child(Field::new(&self.address, txt::ADDRESS_FIELD).selector("address-field"))
                    .child(
                        div()
                            .mt(u(6.0))
                            .typeface(theme.types.small, &theme)
                            .ink(theme.colors.ink_2)
                            .debug_selector(|| "address-help".to_owned())
                            .child(if asking {
                                crate::text::ADDRESS_ASKING
                            } else {
                                txt::ADDRESS_HELP
                            }),
                    )
                    .child(
                        crate::ui::notice_slot("address-error-slot").children(error.map(|error| {
                            div()
                                .mt(u(8.0))
                                .child(Notice::error(error).selector("address-error"))
                        })),
                    )
                    .child(dialog_footer([
                        cancel.into_any_element(),
                        Button::new("add-server-submit")
                            .kind(ButtonKind::Primary)
                            .label(txt::ADD)
                            .selector("add-server-submit")
                            .disabled(asking)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_address(window, cx);
                            }))
                            .into_any_element(),
                    ]));
            }
            AddTab::Workspace => {
                let signed = self.signed_in_servers(cx);
                let chosen = add
                    .server
                    .and_then(|id| signed.iter().find(|(held, _)| *held == id))
                    .map_or_else(
                        || SharedString::from(txt::NO_SERVER),
                        |(_, name)| SharedString::from(name.clone()),
                    );
                let mut server = div().flex().flex_col().flex_none().child(
                    SelectBox::new("add-server-choice", txt::SERVER_CHOICE, chosen)
                        .selector("add-server-choice")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(add) = this.add_window() {
                                add.choosing = !add.choosing;
                            }
                            cx.notify();
                        })),
                );
                if add.choosing {
                    let mut choices = div()
                        .flex()
                        .flex_col()
                        .flex_none()
                        .mt(u(4.0))
                        .py(u(4.0))
                        .rounded(u(theme.metrics.radius))
                        .border_1()
                        .border_color(gpui::Hsla::from(theme.colors.rule))
                        .debug_selector(|| "add-server-choices".to_owned());
                    for (server_id, name) in signed {
                        choices = choices.child(super::admin::named(
                            format!("add-server-choice-{server_id}"),
                            ChannelRow::new(name.clone(), Icon::Plug, name)
                                .selected(Some(server_id) == add.server)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(add) = this.add_window() {
                                        add.server = Some(server_id);
                                        add.choosing = false;
                                    }
                                    this.focus_field(&this.add_code.clone(), window, cx);
                                    cx.notify();
                                })),
                        ));
                    }
                    server = server.child(choices);
                }
                dialog = dialog
                    .child(
                        div()
                            .mb(u(4.0))
                            .typeface(theme.types.label, &theme)
                            .ink(theme.colors.ink_2)
                            .child(txt::SERVER_CHOICE),
                    )
                    .child(server)
                    .child(
                        div()
                            .mt(u(10.0))
                            .mb(u(4.0))
                            .typeface(theme.types.label, &theme)
                            .ink(theme.colors.ink_2)
                            .child(txt::CODE_FIELD),
                    )
                    .child(Field::new(&self.add_code, txt::CODE_FIELD).selector("add-code-field"))
                    .child(
                        div()
                            .mt(u(6.0))
                            .typeface(theme.types.small, &theme)
                            .ink(theme.colors.ink_2)
                            .child(txt::CODE_HELP),
                    )
                    .child(
                        crate::ui::notice_slot("add-code-error-slot").children(add.error.map(
                            |error| {
                                div()
                                    .mt(u(8.0))
                                    .child(Notice::error(error).selector("add-code-error"))
                            },
                        )),
                    )
                    .child(dialog_footer([
                        cancel.into_any_element(),
                        Button::new("add-code-submit")
                            .kind(ButtonKind::Primary)
                            .label(txt::JOIN)
                            .selector("add-code-submit")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_add_code(window, cx);
                            }))
                            .into_any_element(),
                    ]));
            }
        }
        dialog.into_any_element()
    }
}
