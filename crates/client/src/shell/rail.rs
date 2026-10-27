//! The server book: every saved server with its workspaces and its state, the groups of the
//! person (ADR 0775), and the field that adds a server. Collapsed, it shows one icon per
//! server with a menu, and one tile per group.

use gpui::{
    AnyElement, AppContext, ClickEvent, Context, Hsla, InteractiveElement, IntoElement,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, Window, deferred, div,
    prelude::FluentBuilder, px,
};
use junto_client_core::flow::add_server::{AddServerAction, AddServerState};

use super::{Layer, Shell, join, window_controls_height};
use crate::assets::Icon;
use crate::state::book::DropTarget;
use crate::state::view::{BookPart, ServerRow, ServerStatus, ShellView};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Avatar, Band, BandKind, Button, ButtonKind, CornerCount, Field, FieldKind, Menu, ScrollArea,
    StatusLine, StyleExt, WorkspaceRow, focus_ring, focus_ring_inside, menu_label, selection_mark,
};

/// The line under the name of a server: an icon and a text for every state that is not
/// "logged in and current".
pub(crate) fn status_line(row: &ServerRow) -> Option<(Option<Icon>, &'static str)> {
    Some(match &row.status {
        ServerStatus::Ready | ServerStatus::Standby => {
            if row.has_recovery {
                (Some(Icon::Warning), text::STATUS_RECOVERY)
            } else {
                return None;
            }
        }
        ServerStatus::Connecting => (None, text::STATUS_CONNECTING),
        ServerStatus::Reconnecting => (Some(Icon::Retry), text::STATUS_RECONNECTING),
        ServerStatus::LoggedOut => (Some(Icon::Warning), text::STATUS_LOGGED_OUT),
        ServerStatus::LoggedOutByPerson => (None, text::STATUS_LOGGED_OUT_BY_PERSON),
        ServerStatus::WaitingForApproval { .. } => {
            (Some(Icon::Clock), text::STATUS_WAITING_FOR_APPROVAL)
        }
        ServerStatus::WaitingForLink { .. } => (Some(Icon::Clock), text::STATUS_WAITING_FOR_LINK),
        ServerStatus::NoAccount(notice) => (None, text::no_account_status(*notice)),
        ServerStatus::Banned => (Some(Icon::Warning), text::STATUS_BANNED),
        ServerStatus::Inactive => (None, text::STATUS_INACTIVE),
        ServerStatus::Deleted => (Some(Icon::Warning), text::STATUS_DELETED),
        ServerStatus::Unreachable => (Some(Icon::Warning), text::STATUS_UNREACHABLE),
        ServerStatus::Incompatible(reason) => (
            Some(Icon::Warning),
            match reason {
                junto_client_core::version::Incompatibility::ClientTooOld { .. } => {
                    text::STATUS_NEEDS_UPDATE
                }
                junto_client_core::version::Incompatibility::ServerTooOld { .. } => {
                    text::STATUS_TOO_OLD
                }
                _ => text::STATUS_INCOMPATIBLE,
            },
        ),
        ServerStatus::OtherServer => (Some(Icon::Warning), text::STATUS_OTHER_SERVER),
    })
}

fn seed(row: &ServerRow) -> u128 {
    row.id.as_uuid().as_u128()
}

/// The picture of a server at a size in design pixels: its logo when the window holds the
/// image (ADR 0101), and the generated picture otherwise. Only the logo has a name for a
/// screen reader and for a test: the generated picture is decoration beside the name of
/// the server.
pub(super) fn picture(
    place: &'static str,
    index: usize,
    row: &ServerRow,
    size: f32,
    radius: f32,
) -> AnyElement {
    let picture = Avatar::new(seed(row), size)
        .radius(radius)
        .image(row.logo.clone());
    if row.logo.is_none() {
        return picture.into_any_element();
    }
    div()
        .id((place, index))
        .role(Role::Image)
        .aria_label(text::logo_of(&row.name))
        .debug_selector(move || format!("{place}-{index}"))
        .flex_none()
        .child(picture)
        .into_any_element()
}

impl Shell {
    /// Sends the typed address to the flow that adds a server.
    pub fn submit_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.adding.is_some() {
            return;
        }
        let address = self.address.read(cx).value().to_string();
        let flow = self.state.read(cx).client().add_server();
        let asking = flow.dispatch(AddServerAction::Fetch {
            address: address.clone(),
        });
        self.adding = Some(flow.clone());
        self.address_error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = asking.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.adding = None;
                match (result, flow.view().state) {
                    (_, AddServerState::Preview(_) | AddServerState::KeyOfOtherAddress { .. }) => {
                        this.code
                            .update(cx, |code, cx| code.set_value("", window, cx));
                        let opened = this.next_join();
                        this.open_layer(
                            Layer::Join(join::Join::preview(opened, flow, address)),
                            window,
                            cx,
                        );
                        this.focus_field(&this.code.clone(), window, cx);
                    }
                    (Err(error), _) => {
                        this.address_error = Some(text::flow_error(&error));
                    }
                    (Ok(()), _) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_rail(
        &mut self,
        view: &ShellView,
        collapsed: bool,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let colors = theme.colors;
        let width = if collapsed {
            theme.metrics.rail_collapsed_width
        } else {
            theme.metrics.rail_width
        };
        let frame = div()
            .id("server-book")
            .role(Role::Navigation)
            .aria_label(text::SERVER_BOOK)
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(u(width))
            .h_full()
            .bg(Hsla::from(colors.basalt))
            .border_r_1()
            .border_color(Hsla::from(colors.basalt_edge))
            .ink(colors.bone)
            .child(super::title_bar::header_zone(
                div().flex_none().h(u(window_controls_height(cx))),
            ));
        if collapsed {
            frame.children(self.render_rail_collapsed(view, narrow, &theme, window, cx))
        } else {
            frame.children(self.render_rail_expanded(view, narrow, &theme, window, cx))
        }
    }

    fn render_rail_expanded(
        &mut self,
        view: &ShellView,
        narrow: bool,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .pt(u(2.0))
            .pr(u(8.0))
            .pb(u(8.0))
            .pl(u(12.0))
            .child(
                div()
                    .typeface(theme.types.label, theme)
                    .ink(colors.bone_label)
                    .child(text::SERVER_BOOK),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(u(2.0))
                    .child({
                        // One control collapses every server and group while one of them is
                        // expanded, and expands them all otherwise.
                        let expanded = view.anything_expanded();
                        let label = if expanded {
                            text::book::COLLAPSE_EVERYTHING
                        } else {
                            text::book::EXPAND_EVERYTHING
                        };
                        Button::new("collapse-everything")
                            .kind(ButtonKind::Rail)
                            .icon(if expanded { Icon::Shrink } else { Icon::List })
                            .tooltip(label)
                            .accessible_label(label)
                            .selector("collapse-everything")
                            .disabled(view.servers.is_empty())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.arrange(
                                        junto_client_core::arrangement::Change::CollapseEverything {
                                            collapsed: expanded,
                                        },
                                        cx,
                                    );
                                });
                            }))
                    })
                    .child(
                        Button::new("collapse-server-book")
                            .kind(ButtonKind::Rail)
                            .icon(Icon::Collapse)
                            .tooltip(text::COLLAPSE_SERVER_BOOK)
                            .selector("collapse-server-book")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_server_book(narrow, cx);
                            })),
                    ),
            );

        let mut servers: Vec<AnyElement> = Vec::new();
        let band = || {
            div()
                .flex_none()
                .mt(u(6.0))
                .mx(u(12.0))
                .mb(u(10.0))
                .child(Band::new(BandKind::Rail))
                .into_any_element()
        };
        for (position, part) in view.book.iter().enumerate() {
            if position > 0 {
                servers.push(band());
            }
            match part {
                BookPart::Server(index) => {
                    servers.push(self.render_server(*index, &view.servers[*index], theme, cx));
                }
                BookPart::Group(group) => {
                    let row = &view.groups[*group];
                    servers.push(self.render_group_head(*group, row, view, theme, cx));
                    if !row.collapsed {
                        let members: Vec<AnyElement> = row
                            .servers
                            .iter()
                            .map(|index| {
                                self.render_server(*index, &view.servers[*index], theme, cx)
                            })
                            .collect();
                        servers.push(
                            div()
                                .flex()
                                .flex_col()
                                .flex_none()
                                .ml(u(6.0))
                                .border_l_2()
                                .border_color(Hsla::from(super::arrange::group_colour(
                                    row.colour, theme,
                                )))
                                .debug_selector({
                                    let group = *group;
                                    move || format!("group-servers-{group}")
                                })
                                .children(members)
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        servers.push(self.render_book_end(theme, cx));
        let arrange_failed = self.state.read(cx).arrange_failure().is_some()
            && !matches!(
                self.layer,
                Layer::GroupMenu(_) | Layer::WorkspaceMenu(..) | Layer::ServerMenu
            );

        let asking = self.adding.is_some();
        let help: SharedString = if asking {
            text::ADDRESS_ASKING.into()
        } else {
            self.address_error
                .clone()
                .map_or_else(|| text::ADDRESS_HELP.into(), SharedString::from)
        };
        let failed = self.address_error.is_some() && !asking;
        let add = div()
            .flex()
            .flex_col()
            .flex_none()
            .px(u(12.0))
            .pb(u(12.0))
            .child(
                div()
                    .pb(u(6.0))
                    .typeface(theme.types.label, theme)
                    .ink(colors.bone_label)
                    .child(text::ADD_SERVER),
            )
            .child(
                Field::new(&self.address, text::ADDRESS_FIELD)
                    .kind(FieldKind::RailAddress)
                    .selector("address-field")
                    .trailing(
                        Button::new("add-server")
                            .kind(ButtonKind::RailAccent)
                            .icon(Icon::Plus)
                            .tooltip(text::ADD_SERVER_BY_ADDRESS)
                            .selector("add-server")
                            .disabled(asking)
                            .rounded_l(px(0.0))
                            .m(px(-1.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_address(window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("address-help")
                    .h(u(64.0))
                    .overflow_y_scroll()
                    .debug_selector(|| "address-help".to_owned())
                    .when(failed, |help| help.role(Role::Alert))
                    .aria_label(help.clone())
                    .mt(u(7.0))
                    .typeface(theme.types.small, theme)
                    .ink(if failed { colors.ochre } else { colors.bone_2 })
                    .child(help),
            );

        vec![
            header.into_any_element(),
            div()
                .flex_none()
                .when(arrange_failed, |slot| {
                    slot.child(self.render_arrange_error(cx))
                })
                .into_any_element(),
            ScrollArea::new("servers", &self.servers_scroll, text::SERVER_BOOK)
                .dark()
                .children(servers)
                .into_any_element(),
            add.into_any_element(),
            self.render_client_version(false, theme, cx),
        ]
    }

    fn render_server(
        &mut self,
        index: usize,
        row: &ServerRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let server_id = row.id;
        let collapsed = row.workspaces_collapsed;
        let name: SharedString = row.name.clone().into();
        // The server is marked itself when no workspace row carries the selection.
        let marked = row.selected && !row.workspaces.iter().any(|workspace| workspace.selected);
        let ring = focus_ring_inside(colors.ochre, theme.metrics.focus_ring);
        let line = match status_line(row) {
            Some((icon, status)) => {
                let line = StatusLine::new(status);
                match icon {
                    Some(icon) => line.icon(icon),
                    None => line,
                }
                .into_any_element()
            }
            None => div()
                .typeface(theme.types.mono_small, theme)
                .ink(colors.bone_3)
                .truncate()
                .child(row.address.clone())
                .into_any_element(),
        };
        let head = div()
            .id(("server", index))
            .role(Role::ListItem)
            .aria_label(super::book::head_label(row))
            .aria_selected(row.selected)
            .debug_selector(move || format!("server-{index}"))
            .relative()
            .flex()
            .items_center()
            .gap(u(8.0))
            .py(u(5.0))
            .pr(u(8.0))
            .pl(u(10.0))
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .when(marked, |head| {
                head.bg(Hsla::from(colors.basalt_3))
                    .child(selection_mark(cx))
            })
            .hover(move |style| style.bg(Hsla::from(colors.basalt_3)))
            .focus_visible(move |style| style.shadow(ring))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state
                    .update(cx, |state, cx| state.select_server(server_id, cx));
            }))
            .on_drag(super::arrange::ServerDrag(server_id), {
                let name = name.clone();
                move |_, _, _, cx| cx.new(|_| super::person_drag::DragLabel(name.clone()))
            })
            .child(
                div()
                    .flex_none()
                    .rounded(u(theme.metrics.radius))
                    .shadow(focus_ring(colors.basalt_edge, 1.0))
                    .child(picture("server-logo", index, row, 28.0, 3.0)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.name, theme)
                            .ink(colors.bone)
                            .truncate()
                            .child(row.name.clone()),
                    )
                    .child(line),
            )
            .children(self.render_waiting_sign(index, row, cx))
            // What the server holds unread that no row of a workspace shows: with its
            // workspaces collapsed, all that it holds.
            .children(super::book::head_sign(row, index, cx))
            .when(!row.workspaces.is_empty(), |head| {
                let label = if collapsed {
                    text::book::EXPAND_WORKSPACES
                } else {
                    text::book::COLLAPSE_WORKSPACES
                };
                head.child(
                    div()
                        .flex_none()
                        .debug_selector(move || format!("workspaces-toggle-{index}"))
                        .child(
                            Button::new(("workspaces-toggle", index))
                                .kind(ButtonKind::Rail)
                                .icon(if collapsed {
                                    Icon::ChevronRight
                                } else {
                                    Icon::ChevronDown
                                })
                                .tooltip(label)
                                .accessible_label(label)
                                .w(u(22.0))
                                .h(u(22.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.state.update(cx, |state, cx| {
                                        state.arrange(
                                            junto_client_core::arrangement::Change::CollapseWorkspaces {
                                                server: server_id,
                                                collapsed: !collapsed,
                                            },
                                            cx,
                                        );
                                    });
                                })),
                        ),
                )
            });
        let head = self.drop_target(head, DropTarget::Server(server_id), theme, cx);
        let open_menu = match &self.layer {
            Layer::WorkspaceMenu(open_server, open) if *open_server == server_id => Some(*open),
            _ => None,
        };
        let mut workspaces: Vec<AnyElement> = Vec::new();
        for (position, workspace) in row.workspaces.iter().enumerate() {
            let workspace_id = workspace.id;
            let shown: SharedString = workspace.name.clone().into();
            let row_element = WorkspaceRow::new(
                ("workspace", index * 1000 + position),
                workspace.name.clone(),
                workspace.id.as_uuid().as_u128(),
            )
            .selected(workspace.selected)
            .unread(super::channels::unread_sign(workspace.unread))
            .open(open_menu == Some(workspace_id))
            .on_menu(Self::handler(cx, move |this, window, cx| {
                this.open_workspace_menu(server_id, workspace_id, window, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.select_workspace(server_id, workspace_id, cx);
                });
            }));
            let dragged = div()
                .id(("workspace-drag", index * 1000 + position))
                .flex_none()
                .on_drag(
                    super::arrange::WorkspaceDrag(server_id, workspace_id),
                    move |_, _, _, cx| cx.new(|_| super::person_drag::DragLabel(shown.clone())),
                )
                .child(row_element);
            let dragged = self.drop_target(
                dragged,
                DropTarget::Workspace(server_id, workspace_id),
                theme,
                cx,
            );
            let menu = (open_menu == Some(workspace_id))
                .then(|| self.render_workspace_menu(row, position, cx));
            workspaces.push(
                div()
                    .relative()
                    .flex_none()
                    .child(dragged)
                    .children(menu)
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .pt(u(2.0))
            .pb(u(8.0))
            .child(head)
            .when(!row.workspaces.is_empty() && !collapsed, |server| {
                server.child(
                    div()
                        .relative()
                        .flex()
                        .flex_col()
                        .mt(u(2.0))
                        // The trunk of the tree line.
                        .child(
                            div()
                                .absolute()
                                .left(u(24.0))
                                .top_0()
                                .bottom(u(14.0))
                                .w(px(1.0))
                                .bg(Hsla::from(colors.basalt_rule)),
                        )
                        .children(workspaces),
                )
            })
            .into_any_element()
    }

    fn render_rail_collapsed(
        &mut self,
        view: &ShellView,
        narrow: bool,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let mut icons: Vec<AnyElement> = Vec::new();
        let rule = || {
            div()
                .flex_none()
                .w(u(28.0))
                .h(px(1.0))
                .my(u(7.0))
                .bg(Hsla::from(colors.basalt_rule))
                .into_any_element()
        };
        for (position, part) in view.book.iter().enumerate() {
            if position > 0 {
                icons.push(rule());
            }
            match part {
                BookPart::Server(index) => {
                    icons.push(self.render_server_icon(*index, &view.servers[*index], theme, cx));
                }
                BookPart::Group(group) => {
                    let row = &view.groups[*group];
                    let tile = self.render_group_tile(*group, row, view, theme, cx);
                    if row.collapsed {
                        icons.push(tile);
                    } else {
                        let members: Vec<AnyElement> = row
                            .servers
                            .iter()
                            .map(|index| {
                                self.render_server_icon(*index, &view.servers[*index], theme, cx)
                            })
                            .collect();
                        icons.push(
                            div()
                                .flex()
                                .flex_col()
                                .flex_none()
                                .items_center()
                                .rounded(u(6.0))
                                .bg(super::arrange::group_wash(row.colour, theme))
                                .pb(u(4.0))
                                .child(tile)
                                .children(members)
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        icons.push(self.render_book_end(theme, cx));
        vec![
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .items_center()
                .child(
                    Button::new("expand-server-book")
                        .kind(ButtonKind::Rail)
                        .icon(Icon::Expand)
                        .tooltip(text::EXPAND_SERVER_BOOK)
                        .selector("expand-server-book")
                        .w(u(36.0))
                        .h(u(26.0))
                        .mb(u(12.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_server_book(narrow, cx);
                        })),
                )
                .child(
                    div()
                        .id("servers")
                        .role(Role::List)
                        .aria_label(text::SERVER_BOOK)
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .items_center()
                        .children(icons),
                )
                .child(
                    Button::new("add-server")
                        .kind(ButtonKind::RailAccent)
                        .icon(Icon::Plus)
                        .tooltip(text::ADD_SERVER_BY_ADDRESS)
                        .selector("add-server")
                        .w(u(36.0))
                        .h(u(30.0))
                        .mb(u(12.0))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.show_address_field(narrow, window, cx);
                        })),
                )
                .child(self.render_client_version(true, theme, cx))
                .into_any_element(),
        ]
    }

    fn render_server_icon(
        &mut self,
        index: usize,
        row: &ServerRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let server_id = row.id;
        let open = self.hover_menu.map(|(open, _)| open) == Some(server_id);
        let ring = if open {
            focus_ring(colors.ochre, 2.0)
        } else {
            focus_ring(colors.basalt_edge, 1.0)
        };
        let focused_ring = focus_ring_inside(colors.ochre, theme.metrics.focus_ring);
        let name: SharedString = row.name.clone().into();
        let label = if row.for_person > 0 {
            format!("{}, {}", row.name, text::for_person(row.for_person))
        } else {
            row.name.clone()
        };
        let icon = div()
            .id(("server", index))
            .role(Role::ListItem)
            .aria_label(label)
            .aria_selected(row.selected)
            .aria_expanded(open)
            .debug_selector(move || format!("server-{index}"))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(u(theme.metrics.rail_collapsed_width))
            .h(u(46.0))
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .group("server-icon")
            .when(row.selected, |icon| {
                icon.bg(Hsla::from(colors.basalt_3))
                    .child(selection_mark(cx))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                if *hovered {
                    this.open_hover_menu(server_id, false, window, cx);
                } else {
                    this.leave_hover_menu(cx);
                }
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // The keyboard opens the menu and moves into it. The pointer selects the
                // server; its menu is open already.
                if matches!(event, ClickEvent::Keyboard(_)) {
                    this.open_hover_menu(server_id, true, window, cx);
                } else {
                    this.state
                        .update(cx, |state, cx| state.select_server(server_id, cx));
                }
            }))
            .focus_visible(move |style| style.shadow(focused_ring))
            .on_drag(super::arrange::ServerDrag(server_id), move |_, _, _, cx| {
                cx.new(|_| super::person_drag::DragLabel(name.clone()))
            })
            .child(div().rounded(u(4.0)).shadow(ring).child(picture(
                "server-logo",
                index,
                row,
                36.0,
                4.0,
            )))
            .when(row.unread.any() && status_line(row).is_none(), |icon| {
                // The server holds messages that the person has not read: the count of the
                // messages for the person alone at the upper corner of the avatar, as
                // Discord counts mentions on a server, or the terracotta sign of the
                // channel list without a count.
                let sign = if row.for_person > 0 {
                    div()
                        .absolute()
                        .right(u(4.0))
                        .top(u(0.0))
                        .child(CornerCount::new(text::unread_count(row.for_person)))
                } else {
                    div()
                        .absolute()
                        .right(u(10.0))
                        .top(u(2.0))
                        .size(u(8.0))
                        .rounded_full()
                        .border_1()
                        .border_color(Hsla::from(colors.basalt))
                        .bg(Hsla::from(colors.terracotta))
                };
                icon.child(sign.debug_selector(move || format!("server-unread-{index}")))
            })
            .when(row.waiting_join_requests > 0, |icon| {
                // The join requests that wait for the person: their count at the lower
                // corner of the avatar.
                let waiting = row.waiting_join_requests;
                icon.child(
                    div()
                        .absolute()
                        .right(u(4.0))
                        .bottom(u(1.0))
                        .debug_selector(move || format!("server-waiting-{index}"))
                        .child(CornerCount::new(text::unread_count(waiting as u64))),
                )
            })
            .when(status_line(row).is_some(), |icon| {
                // A dot at the corner of the avatar tells that the server needs a look.
                icon.child(
                    div()
                        .absolute()
                        .right(u(10.0))
                        .top(u(2.0))
                        .size(u(8.0))
                        .rounded_full()
                        .border_1()
                        .border_color(Hsla::from(colors.basalt))
                        .bg(Hsla::from(colors.ochre)),
                )
            });
        let icon = self.drop_target(icon, DropTarget::Server(server_id), theme, cx);
        // The menu of a workspace that the hover menu opened stays at the tile of its server.
        let workspace_menu = match &self.layer {
            Layer::WorkspaceMenu(open_server, open) if *open_server == server_id => row
                .workspaces
                .iter()
                .position(|workspace| workspace.id == *open)
                .map(|position| self.render_workspace_menu(row, position, cx)),
            _ => None,
        };
        div()
            .relative()
            .flex_none()
            .child(icon)
            .children(workspace_menu)
            .when(open, |wrapper| {
                wrapper.child(
                    deferred(
                        div()
                            .absolute()
                            .left(u(60.0))
                            .top(u(-4.0))
                            .child(self.render_hover_menu(index, row, theme, cx)),
                    )
                    .with_priority(1),
                )
            })
            .into_any_element()
    }

    fn render_hover_menu(
        &mut self,
        index: usize,
        row: &ServerRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme.colors;
        let server_id = row.id;
        let line: SharedString = match status_line(row) {
            Some((_, status)) => status.into(),
            None => row.address.clone().into(),
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(9.0))
            .pt(u(10.0))
            .px(u(12.0))
            .pb(u(9.0))
            .border_b_1()
            .border_color(Hsla::from(colors.basalt_rule))
            .child(picture("server-menu-logo", index, row, 30.0, 3.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.name, theme)
                            .ink(colors.bone)
                            .truncate()
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .typeface(theme.types.mono_small, theme)
                            .ink(colors.bone_3)
                            .truncate()
                            .child(line),
                    ),
            );
        // The workspaces of the server as the expanded server book has them: dragged into an
        // order, their menu from the secondary press or the menu key, and hidden while the
        // workspaces of the server are collapsed (ADR 0775).
        let collapsed = row.workspaces_collapsed;
        let mut workspaces: Vec<AnyElement> = Vec::new();
        for (position, workspace) in row.workspaces.iter().enumerate().filter(|_| !collapsed) {
            let workspace_id = workspace.id;
            let shown: SharedString = workspace.name.clone().into();
            let row_element = WorkspaceRow::new(
                ("menu-workspace", index * 1000 + position),
                workspace.name.clone(),
                workspace.id.as_uuid().as_u128(),
            )
            .selected(workspace.selected)
            .unread(super::channels::unread_sign(workspace.unread))
            .in_menu()
            .on_menu(Self::handler(cx, move |this, window, cx| {
                this.open_workspace_menu(server_id, workspace_id, window, cx);
            }))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.state.update(cx, |state, cx| {
                    state.select_workspace(server_id, workspace_id, cx);
                });
                this.dismiss_hover_menu(window, cx);
            }));
            let dragged = div()
                .id(("menu-workspace-drag", index * 1000 + position))
                .flex_none()
                .on_drag(
                    super::arrange::WorkspaceDrag(server_id, workspace_id),
                    move |_, _, _, cx| cx.new(|_| super::person_drag::DragLabel(shown.clone())),
                )
                .child(row_element);
            workspaces.push(
                self.drop_target(
                    dragged,
                    DropTarget::Workspace(server_id, workspace_id),
                    theme,
                    cx,
                )
                .into_any_element(),
            );
        }
        let toggle_label = if collapsed {
            text::book::EXPAND_WORKSPACES
        } else {
            text::book::COLLAPSE_WORKSPACES
        };
        let toggle = div()
            .id(("menu-workspaces-toggle", index))
            .role(Role::MenuItem)
            .aria_label(toggle_label)
            .aria_expanded(!collapsed)
            .debug_selector(|| "menu-workspaces-toggle".to_owned())
            .px(u(12.0))
            .py(u(8.0))
            .typeface(theme.types.small, theme)
            .ink(colors.bone_2)
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .hover(move |style| style.bg(Hsla::from(colors.basalt_3)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.arrange(
                        junto_client_core::arrangement::Change::CollapseWorkspaces {
                            server: server_id,
                            collapsed: !collapsed,
                        },
                        cx,
                    );
                });
            }))
            .child(toggle_label);
        let empty = row.workspaces.is_empty();
        div()
            .id(("hover-menu", index))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.stay_in_hover_menu();
                } else {
                    this.leave_hover_menu(cx);
                }
            }))
            .child(
                Menu::new(
                    "server-hover-menu",
                    row.name.clone(),
                    self.hover_menu_focus(),
                )
                .dark()
                .selector("server-hover-menu")
                .on_close(Self::handler(cx, |this, window, cx| {
                    this.dismiss_hover_menu(window, cx);
                }))
                .child(header)
                .when(!empty, |menu| {
                    menu.child(menu_label(text::WORKSPACES, true, cx))
                        .children(workspaces)
                        .child(toggle)
                })
                .when(empty, |menu| {
                    menu.child(
                        div()
                            .id("open-server")
                            .role(Role::MenuItem)
                            .aria_label(row.name.clone())
                            .px(u(12.0))
                            .py(u(8.0))
                            .typeface(theme.types.small, theme)
                            .ink(colors.bone_2)
                            .cursor_pointer()
                            .focusable()
                            .tab_stop(true)
                            .hover(move |style| style.bg(Hsla::from(colors.basalt_3)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.state
                                    .update(cx, |state, cx| state.select_server(server_id, cx));
                                this.dismiss_hover_menu(window, cx);
                            }))
                            .child(text::SHOW_SERVER),
                    )
                }),
            )
    }
}
