//! The arrangement of the server book in the window (ADR 0775): the drags of servers, groups
//! and workspaces with their drops, the heads and the tiles of the groups, and the menus that
//! give the keyboard each action of a drag (ADR 0702).

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, DragMoveEvent, Entity, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Pixels, Point, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, deferred, div, prelude::FluentBuilder, px,
};
use gpui_base::input::InputState;
use junto_client_core::arrangement::{Change, GroupColour, GroupId};
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::person_drag::DragLabel;
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::book::{
    BookDrop, Dragged, DropTarget, Step, Zone, accepts, dropped, takes_into, zone,
};
use crate::state::view::{BookPart, GroupRow, ServerRow, ShellView};
use crate::text;
use crate::text::book as txt;
use crate::theme::{Theme, u};
use crate::ui::{
    Badge, Button, ButtonKind, CornerCount, Dialog, Field, Menu, MenuItem, StyleExt, dialog_footer,
    focus_ring_inside, icon, menu_separator, selection_mark, tooltip, unread_dot,
};

/// A server that is dragged in the server book.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ServerDrag(pub(crate) ServerId);

/// A group that is dragged in the server book.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GroupDrag(pub(crate) GroupId);

/// A workspace that is dragged among the workspaces of its server.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WorkspaceDrag(pub(crate) ServerId, pub(crate) WorkspaceId);

/// What the window holds of the arrangement of the server book.
pub(crate) struct ArrangeUi {
    /// Where the drag that is over the server book would drop now.
    drop: Option<BookDrop>,
    /// The name of the group of the open rename dialog.
    group_name: Entity<InputState>,
}

impl ArrangeUi {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> Self {
        Self {
            drop: None,
            group_name: Field::state(txt::GROUP_NAME_FIELD, window, cx),
        }
    }
}

/// The colour of a group on the basalt of the server book.
pub(crate) fn group_colour(colour: GroupColour, theme: &Theme) -> gpui::Rgba {
    let colors = theme.colors;
    match colour {
        GroupColour::Bone => colors.bone_2,
        GroupColour::Terracotta => colors.terracotta,
        GroupColour::Ochre => colors.ochre,
        GroupColour::Olive => colors.olive_bright,
        GroupColour::Cypress => colors.cypress,
        GroupColour::Bronze => colors.bronze,
        GroupColour::Cinnabar => colors.cinnabar,
    }
}

/// The colour of a group as a wash under its servers.
pub(crate) fn group_wash(colour: GroupColour, theme: &Theme) -> Hsla {
    Hsla::from(group_colour(colour, theme)).opacity(0.16)
}

/// The keys that open the menu of a row: the menu key, and Shift with F10.
fn opens_menu(event: &KeyDownEvent) -> bool {
    let keys = &event.keystroke;
    match keys.key.as_str() {
        "menu" => true,
        "f10" => keys.modifiers.shift,
        _ => false,
    }
}

impl Shell {
    /// Where the drag that is over the server book would drop now, while one runs.
    pub(crate) fn book_drop(&self, cx: &App) -> Option<BookDrop> {
        self.arrange.drop.filter(|_| cx.has_active_drag())
    }

    fn drag_moved(
        &mut self,
        dragged: Dragged,
        target: DropTarget,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if !bounds.contains(&position) || !accepts(dragged, target) {
            return;
        }
        let at = f32::from(position.y - bounds.origin.y);
        let zone = zone(
            at,
            f32::from(bounds.size.height),
            takes_into(dragged, target),
        );
        let drop = Some(BookDrop { target, zone });
        if self.arrange.drop != drop {
            self.arrange.drop = drop;
            cx.notify();
        }
    }

    /// Makes the change of a drop on `target`, in the zone where the drag was last.
    fn drop_on_book(&mut self, dragged: Dragged, target: DropTarget, cx: &mut Context<Self>) {
        let drop = self
            .arrange
            .drop
            .take()
            .filter(|drop| drop.target == target)
            .unwrap_or(BookDrop {
                target,
                zone: Zone::After,
            });
        let view = self.state.read(cx).view();
        let shelves = view.shelves();
        let Some(change) = dropped(dragged, drop, &shelves) else {
            cx.notify();
            return;
        };
        self.state.update(cx, |state, cx| match change {
            Change::MoveWorkspace {
                server,
                workspace,
                to,
                ..
            } => state.drop_workspace(server, workspace, to, cx),
            change => state.arrange(change, cx),
        });
    }

    /// Makes `element` a target of the drags of the server book, with the mark of the zone
    /// where a drag would drop.
    pub(crate) fn drop_target(
        &mut self,
        element: Stateful<gpui::Div>,
        target: DropTarget,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let colors = theme.colors;
        let mark = self
            .book_drop(cx)
            .filter(|drop| drop.target == target)
            .map(|drop| drop.zone);
        let line = |top: bool| {
            div()
                .absolute()
                .left(u(6.0))
                .right(u(6.0))
                .h(px(2.0))
                .when(top, |line| line.top(px(-1.0)))
                .when(!top, |line| line.bottom(px(-1.0)))
                .bg(Hsla::from(colors.ochre))
        };
        element
            .relative()
            .can_drop(move |value, _, _| {
                let dragged = if let Some(ServerDrag(server)) = value.downcast_ref::<ServerDrag>() {
                    Dragged::Server(*server)
                } else if let Some(GroupDrag(group)) = value.downcast_ref::<GroupDrag>() {
                    Dragged::Group(*group)
                } else if let Some(WorkspaceDrag(server, workspace)) =
                    value.downcast_ref::<WorkspaceDrag>()
                {
                    Dragged::Workspace(*server, *workspace)
                } else {
                    return false;
                };
                accepts(dragged, target)
            })
            .on_drag_move::<ServerDrag>(cx.listener(
                move |this, event: &DragMoveEvent<ServerDrag>, _, cx| {
                    let ServerDrag(server) = *event.drag(cx);
                    let (bounds, at) = (event.bounds, event.event.position);
                    this.drag_moved(Dragged::Server(server), target, bounds, at, cx);
                },
            ))
            .on_drag_move::<GroupDrag>(cx.listener(
                move |this, event: &DragMoveEvent<GroupDrag>, _, cx| {
                    let GroupDrag(group) = *event.drag(cx);
                    let (bounds, at) = (event.bounds, event.event.position);
                    this.drag_moved(Dragged::Group(group), target, bounds, at, cx);
                },
            ))
            .on_drag_move::<WorkspaceDrag>(cx.listener(
                move |this, event: &DragMoveEvent<WorkspaceDrag>, _, cx| {
                    let WorkspaceDrag(server, workspace) = *event.drag(cx);
                    let (bounds, at) = (event.bounds, event.event.position);
                    this.drag_moved(
                        Dragged::Workspace(server, workspace),
                        target,
                        bounds,
                        at,
                        cx,
                    );
                },
            ))
            .on_drop(
                cx.listener(move |this, ServerDrag(server): &ServerDrag, _, cx| {
                    this.drop_on_book(Dragged::Server(*server), target, cx);
                }),
            )
            .on_drop(
                cx.listener(move |this, GroupDrag(group): &GroupDrag, _, cx| {
                    this.drop_on_book(Dragged::Group(*group), target, cx);
                }),
            )
            .on_drop(cx.listener(
                move |this, WorkspaceDrag(server, workspace): &WorkspaceDrag, _, cx| {
                    this.drop_on_book(Dragged::Workspace(*server, *workspace), target, cx);
                },
            ))
            .when(mark == Some(Zone::Before), |element| {
                element.child(line(true))
            })
            .when(mark == Some(Zone::After), |element| {
                element.child(line(false))
            })
            .when(mark == Some(Zone::Into), |element| {
                element.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(u(theme.metrics.radius))
                        .border_2()
                        .border_color(Hsla::from(colors.ochre)),
                )
            })
    }

    /// The free space after the last part of the server book: a drop there puts a server or
    /// a group at the end.
    pub(crate) fn render_book_end(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let end = div()
            .id("server-book-end")
            .flex_1()
            .min_h(u(24.0))
            .debug_selector(|| "server-book-end".to_owned());
        self.drop_target(end, DropTarget::End, theme, cx)
            .into_any_element()
    }

    pub fn open_group_menu(&mut self, group: GroupId, window: &mut Window, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.dismiss_arrange_failure(cx));
        self.open_layer(Layer::GroupMenu(group), window, cx);
    }

    pub fn open_workspace_menu(
        &mut self,
        server_id: ServerId,
        workspace: WorkspaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |state, cx| state.dismiss_arrange_failure(cx));
        self.open_layer(Layer::WorkspaceMenu(server_id, workspace), window, cx);
    }

    /// Opens the choice of the group of a server, as the item of its menu does.
    pub fn open_group_choice(
        &mut self,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(Layer::GroupChoice(server_id), window, cx);
    }

    /// Opens the dialog that names a group, as the item of its menu does.
    pub fn open_rename_dialog(
        &mut self,
        group: GroupId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = self.state.read(cx).view();
        if let Some(row) = view.groups.iter().find(|row| row.id == group) {
            self.open_rename_group(&row.clone(), window, cx);
        }
    }

    fn open_rename_group(&mut self, group: &GroupRow, window: &mut Window, cx: &mut Context<Self>) {
        let name = group.name.clone();
        self.arrange
            .group_name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        self.open_layer(Layer::RenameGroup(group.id), window, cx);
        self.focus_field(&self.arrange.group_name.clone(), window, cx);
    }

    /// The head of a group in the expanded server book: its colour, its name, the number of
    /// its servers, what they hold unread and the join requests that wait. A press collapses
    /// or expands the group; the secondary press and the menu key open its menu.
    pub(crate) fn render_group_head(
        &mut self,
        index: usize,
        group: &GroupRow,
        view: &ShellView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (id, collapsed) = (group.id, group.collapsed);
        let ring = focus_ring_inside(colors.ochre, theme.metrics.focus_ring);
        let label: SharedString = group_label(group).into();
        let name: SharedString = group.shown_name.clone().into();
        let menu_open = matches!(self.layer, Layer::GroupMenu(open) if open == id);
        let head = div()
            .id(("group", index))
            .role(Role::ListItem)
            .aria_label(label.clone())
            .aria_expanded(!collapsed)
            .debug_selector(move || format!("group-{index}"))
            .flex()
            .items_center()
            .gap(u(7.0))
            .py(u(5.0))
            .pr(u(8.0))
            .pl(u(10.0))
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .tooltip(tooltip(label))
            .when(collapsed && group.selected, |head| {
                head.bg(Hsla::from(colors.basalt_3))
                    .child(selection_mark(cx))
            })
            .when(menu_open, |head| head.bg(Hsla::from(colors.basalt_3)))
            .hover(move |style| style.bg(Hsla::from(colors.basalt_3)))
            .focus_visible(move |style| style.shadow(ring))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.arrange(
                        Change::CollapseGroup {
                            group: id,
                            collapsed: !collapsed,
                        },
                        cx,
                    );
                });
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| this.open_group_menu(id, window, cx)),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if opens_menu(event) {
                    this.open_group_menu(id, window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_drag(GroupDrag(id), {
                let name = name.clone();
                move |_, _, _, cx| cx.new(|_| DragLabel(name.clone()))
            })
            .child(icon(
                if collapsed {
                    Icon::ChevronRight
                } else {
                    Icon::ChevronDown
                },
                12.0,
                colors.bone_2,
            ))
            .child(
                div()
                    .flex_none()
                    .size(u(10.0))
                    .rounded(u(2.0))
                    .bg(Hsla::from(group_colour(group.colour, theme))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .typeface(theme.types.name, theme)
                    .ink(colors.bone)
                    .truncate()
                    .child(name),
            )
            .child(
                div()
                    .flex_none()
                    .typeface(theme.types.small, theme)
                    .ink(colors.bone_3)
                    .debug_selector(move || format!("group-count-{index}"))
                    .child(group.servers.len().to_string()),
            )
            .when(group.waiting_join_requests > 0, |head| {
                let waiting = group.waiting_join_requests;
                head.child(
                    div()
                        .flex_none()
                        .debug_selector(move || format!("group-waiting-{index}"))
                        .child(Badge::new(text::unread_count(waiting as u64)).icon(
                            text::admin::page_icon(crate::state::admin::AdminPage::JoinRequests),
                        )),
                )
            })
            .children(super::channels::unread_sign(group.unread).map(|sign| {
                let mark = match sign.mentions {
                    Some(count) => Badge::new(count).into_any_element(),
                    None => div()
                        .flex_none()
                        .mr(u(2.0))
                        .child(unread_dot(cx))
                        .into_any_element(),
                };
                div()
                    .flex_none()
                    .debug_selector(move || format!("group-unread-{index}"))
                    .child(mark)
                    .into_any_element()
            }));
        let head = self.drop_target(head, DropTarget::Group(id), theme, cx);
        let menu = menu_open.then(|| self.render_group_menu(group, view, cx));
        div()
            .relative()
            .flex_none()
            .child(head)
            .children(menu)
            .into_any_element()
    }

    /// A group in the collapsed server book: collapsed, a tile with the pictures of its first
    /// four servers; expanded, a tile with the mark of a folder above the icons of its
    /// servers on the wash of its colour.
    pub(crate) fn render_group_tile(
        &mut self,
        index: usize,
        group: &GroupRow,
        view: &ShellView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (id, collapsed) = (group.id, group.collapsed);
        let label: SharedString = group_label(group).into();
        let name: SharedString = group.shown_name.clone().into();
        let focused_ring = focus_ring_inside(colors.ochre, theme.metrics.focus_ring);
        let menu_open = matches!(self.layer, Layer::GroupMenu(open) if open == id);
        let face = if collapsed {
            let pictures: Vec<AnyElement> = group
                .servers
                .iter()
                .take(4)
                .map(|server| {
                    super::rail::picture(
                        "group-picture",
                        index * 1000 + *server,
                        &view.servers[*server],
                        15.0,
                        2.0,
                    )
                })
                .collect();
            div()
                .flex()
                .flex_wrap()
                .gap(u(2.0))
                .p(u(2.0))
                .size(u(36.0))
                .rounded(u(4.0))
                .bg(group_wash(group.colour, theme))
                .children(pictures)
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(u(36.0))
                .rounded(u(4.0))
                .bg(group_wash(group.colour, theme))
                .child(icon(
                    Icon::FolderOpen,
                    18.0,
                    group_colour(group.colour, theme),
                ))
                .into_any_element()
        };
        let tile = div()
            .id(("group-tile", index))
            .role(Role::ListItem)
            .aria_label(label.clone())
            .aria_expanded(!collapsed)
            .debug_selector(move || format!("group-{index}"))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(u(theme.metrics.rail_collapsed_width))
            .h(u(46.0))
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .tooltip(tooltip(label))
            .when(collapsed && group.selected, |tile| {
                tile.bg(Hsla::from(colors.basalt_3))
                    .child(selection_mark(cx))
            })
            .focus_visible(move |style| style.shadow(focused_ring))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.arrange(
                        Change::CollapseGroup {
                            group: id,
                            collapsed: !collapsed,
                        },
                        cx,
                    );
                });
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| this.open_group_menu(id, window, cx)),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if opens_menu(event) {
                    this.open_group_menu(id, window, cx);
                    cx.stop_propagation();
                }
            }))
            .on_drag(GroupDrag(id), move |_, _, _, cx| {
                cx.new(|_| DragLabel(name.clone()))
            })
            .child(face)
            .when(collapsed && group.unread.any(), |tile| {
                tile.child(
                    div()
                        .absolute()
                        .right(u(10.0))
                        .top(u(2.0))
                        .debug_selector(move || format!("group-unread-{index}"))
                        .child(match group.unread.mentions {
                            0 => div()
                                .size(u(8.0))
                                .rounded_full()
                                .border_1()
                                .border_color(Hsla::from(colors.basalt))
                                .bg(Hsla::from(colors.terracotta))
                                .into_any_element(),
                            mentions => {
                                CornerCount::new(text::mention_badge(mentions)).into_any_element()
                            }
                        }),
                )
            })
            .when(collapsed && group.waiting_join_requests > 0, |tile| {
                let waiting = group.waiting_join_requests;
                tile.child(
                    div()
                        .absolute()
                        .right(u(4.0))
                        .bottom(u(1.0))
                        .debug_selector(move || format!("group-waiting-{index}"))
                        .child(CornerCount::new(text::unread_count(waiting as u64))),
                )
            });
        let tile = self.drop_target(tile, DropTarget::Group(id), theme, cx);
        let menu = menu_open.then(|| self.render_group_menu(group, view, cx));
        div()
            .relative()
            .flex_none()
            .child(tile)
            .children(menu)
            .into_any_element()
    }

    /// The menu of a group: its name, its colour, its place, collapse, mark as read and
    /// dissolve. Each item that a drag also does is here for the keyboard (ADR 0702).
    fn render_group_menu(
        &mut self,
        group: &GroupRow,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = crate::theme::theme(cx);
        let (id, collapsed, chosen) = (group.id, group.collapsed, group.colour);
        let position = view
            .book
            .iter()
            .position(|part| matches!(part, BookPart::Group(held) if view.groups[*held].id == id));
        let first = position == Some(0);
        let last = position.is_some_and(|position| position + 1 >= view.book.len());
        let renamed = group.clone();
        let mut items: Vec<AnyElement> = vec![
            MenuItem::new("rename-group", txt::RENAME_GROUP)
                .icon(Icon::Edit)
                .detail(txt::RENAME_GROUP_HELP)
                .selector("rename-group")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_rename_group(&renamed, window, cx);
                }))
                .into_any_element(),
            MenuItem::new(
                "collapse-group",
                if collapsed {
                    txt::EXPAND_GROUP
                } else {
                    txt::COLLAPSE_GROUP
                },
            )
            .icon(if collapsed {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            })
            .selector("collapse-group")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.arrange(
                        Change::CollapseGroup {
                            group: id,
                            collapsed: !collapsed,
                        },
                        cx,
                    );
                });
            }))
            .into_any_element(),
        ];
        for (step, item, selector, at_end) in [
            (Step::Up, "group-move-up", "group-move-up", first),
            (Step::Down, "group-move-down", "group-move-down", last),
        ] {
            items.push(
                MenuItem::new(item, txt::move_title(step))
                    .detail(txt::group_move_help(step, at_end))
                    .disabled(at_end)
                    .selector(selector)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.move_group(id, step, cx));
                    }))
                    .into_any_element(),
            );
        }
        let unread = group.unread.any();
        items.push(
            MenuItem::new("mark-group-read", txt::MARK_GROUP_READ)
                .icon(Icon::CheckDouble)
                .detail(if unread {
                    txt::MARK_GROUP_READ_HELP
                } else {
                    txt::NOTHING_UNREAD
                })
                .disabled(!unread)
                .selector("mark-group-read")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state.update(cx, |state, _| state.mark_group_read(id));
                    this.close_layer(window, cx);
                }))
                .into_any_element(),
        );
        items.push(menu_separator(cx).into_any_element());
        items.push(crate::ui::menu_label(txt::COLOUR, false, cx).into_any_element());
        for (number, colour) in GroupColour::ALL.into_iter().enumerate() {
            let item = MenuItem::new(("group-colour", number), txt::colour_name(colour))
                .swatch(Hsla::from(group_colour(colour, &theme)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.arrange(Change::ColourGroup { group: id, colour }, cx);
                    });
                }));
            let item = if colour == chosen {
                item.trailing(txt::CHOSEN)
            } else {
                item
            };
            items.push(
                div()
                    .flex_none()
                    .debug_selector(move || format!("group-colour-{number}"))
                    .child(item)
                    .into_any_element(),
            );
        }
        items.push(menu_separator(cx).into_any_element());
        items.push(
            MenuItem::new("dissolve-group", txt::DISSOLVE_GROUP)
                .icon(Icon::Trash)
                .detail(txt::DISSOLVE_GROUP_HELP)
                .selector("dissolve-group")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state.update(cx, |state, cx| {
                        state.arrange(Change::DissolveGroup { group: id }, cx);
                    });
                    this.close_layer(window, cx);
                }))
                .into_any_element(),
        );
        items.push(self.render_arrange_error(cx));
        self.book_menu("group-menu", txt::GROUP_MENU.to_owned(), items, cx)
    }

    /// The menu of a workspace in the server book: its place among the workspaces of its
    /// server, and mark as read.
    pub(crate) fn render_workspace_menu(
        &mut self,
        row: &ServerRow,
        position: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = row.id;
        let workspace = &row.workspaces[position];
        let (workspace_id, unread) = (workspace.id, workspace.unread.any());
        let count = row.workspaces.len();
        let mut items: Vec<AnyElement> = Vec::new();
        for (step, item, at_end) in [
            (Step::Up, "workspace-move-up", position == 0),
            (Step::Down, "workspace-move-down", position + 1 >= count),
        ] {
            items.push(
                MenuItem::new(item, txt::move_title(step))
                    .detail(txt::workspace_move_help(step, at_end))
                    .disabled(at_end)
                    .selector(item)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state.update(cx, |state, cx| {
                            state.move_workspace(server_id, workspace_id, step, cx);
                        });
                    }))
                    .into_any_element(),
            );
        }
        items.push(
            MenuItem::new("mark-workspace-read", txt::MARK_WORKSPACE_READ)
                .icon(Icon::CheckDouble)
                .detail(if unread {
                    txt::MARK_WORKSPACE_READ_HELP
                } else {
                    txt::NOTHING_UNREAD
                })
                .disabled(!unread)
                .selector("mark-workspace-read")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state.update(cx, |state, _| {
                        state.mark_server_read(server_id, Some(workspace_id));
                    });
                    this.close_layer(window, cx);
                }))
                .into_any_element(),
        );
        items.push(self.render_arrange_error(cx));
        self.book_menu(
            "workspace-menu",
            format!("{}: {}", txt::WORKSPACE_MENU, workspace.name),
            items,
            cx,
        )
    }

    /// The choice of the group of the selected server: a new group, each other group, and
    /// the way out of its group.
    pub(crate) fn render_server_group_choice(
        &mut self,
        server_id: ServerId,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = crate::theme::theme(cx);
        let held = view
            .servers
            .iter()
            .find(|row| row.id == server_id)
            .and_then(|row| row.group);
        let mut items: Vec<AnyElement> = vec![
            MenuItem::new("new-group", txt::NEW_GROUP)
                .icon(Icon::Plus)
                .detail(txt::NEW_GROUP_HELP)
                .selector("new-group")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.state.update(cx, |state, cx| {
                        state.arrange(
                            Change::NewGroup {
                                server: server_id,
                                name: String::new(),
                            },
                            cx,
                        );
                    });
                    this.close_layer(window, cx);
                }))
                .into_any_element(),
        ];
        for (number, group) in view.groups.iter().enumerate() {
            let id = group.id;
            let inside = held == Some(id);
            items.push(
                div()
                    .flex_none()
                    .debug_selector(move || format!("move-into-group-{number}"))
                    .child(
                        MenuItem::new(
                            ("move-into-group", number),
                            txt::move_into(&group.shown_name),
                        )
                        .swatch(Hsla::from(group_colour(group.colour, &theme)))
                        .when(inside, |item| item.detail(txt::IN_THIS_GROUP))
                        .disabled(inside)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.arrange(
                                        Change::MoveServer {
                                            server: server_id,
                                            to: junto_client_core::arrangement::Spot::InGroup(id),
                                        },
                                        cx,
                                    );
                                });
                                this.close_layer(window, cx);
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }
        if held.is_some() {
            items.push(menu_separator(cx).into_any_element());
            items.push(
                MenuItem::new("remove-from-group", txt::REMOVE_FROM_GROUP)
                    .icon(Icon::Leave)
                    .detail(txt::REMOVE_FROM_GROUP_HELP)
                    .selector("remove-from-group")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.update(cx, |state, cx| {
                            state.arrange(Change::LeaveGroup { server: server_id }, cx);
                        });
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .left(u(self.rail_width + 8.0))
            .top(u(theme.metrics.header_height - 4.0))
            .child(
                Menu::new("group-choice", txt::GROUP_CHOICE, self.layer_focus())
                    .selector("group-choice")
                    .on_close(Self::handler(cx, |this, window, cx| {
                        this.close_layer(window, cx);
                    }))
                    .children(items)
                    .child(div().h(u(4.0))),
            )
            .into_any_element()
    }

    /// The dialog that names a group. An empty name leaves the group without one.
    pub(crate) fn render_rename_group(
        &mut self,
        group: GroupId,
        _view: &ShellView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = crate::theme::theme(cx);
        let save = cx.listener(move |this, _, window, cx| {
            let name = this.arrange.group_name.read(cx).value().to_string();
            this.state.update(cx, |state, cx| {
                state.arrange(Change::RenameGroup { group, name }, cx);
            });
            this.close_layer(window, cx);
        });
        Dialog::new(txt::RENAME_GROUP_TITLE, self.layer_focus())
            .selector("rename-group-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            .child(
                Field::new(&self.arrange.group_name, txt::GROUP_NAME_FIELD).selector("group-name"),
            )
            .child(
                div()
                    .mt(u(6.0))
                    .typeface(theme.types.small, &theme)
                    .ink(theme.colors.ink_2)
                    .child(txt::GROUP_NAME_HELP),
            )
            .child(dialog_footer([
                Button::new("cancel")
                    .label(text::CANCEL)
                    .selector("cancel")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new("save-group-name")
                    .kind(ButtonKind::Primary)
                    .label(text::SAVE)
                    .selector("save-group-name")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(save)
                    .into_any_element(),
            ]))
            .into_any_element()
    }

    /// Why the last change of the arrangement was not written. It follows the last item of a
    /// menu, so that a refusal moves no item (ADR 0730).
    pub(crate) fn render_arrange_error(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let failed = self.state.read(cx).arrange_failure().map(str::to_owned);
        crate::ui::notice_slot("arrange-error-slot")
            .at_end()
            .px(u(8.0))
            .children(
                failed.map(|failed| crate::ui::Notice::error(failed).selector("arrange-error")),
            )
            .into_any_element()
    }

    /// A menu at a row of the server book.
    fn book_menu(
        &mut self,
        selector: &'static str,
        label: String,
        items: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .absolute()
            .left(u(36.0))
            .top(u(28.0))
            .child(deferred(
                Menu::new(selector, label, self.layer_focus())
                    .selector(selector)
                    .width(240.0)
                    .on_close(Self::handler(cx, |this, window, cx| {
                        this.close_layer(window, cx);
                    }))
                    .children(items)
                    .child(div().h(u(4.0))),
            ))
            .into_any_element()
    }
}

/// The name of a group for the accessibility tree and its tooltip.
fn group_label(group: &GroupRow) -> String {
    let mut extra = Vec::new();
    if let Some(sign) = super::channels::unread_sign(group.unread) {
        extra.push(sign.label.to_string());
    }
    if group.waiting_join_requests > 0 {
        extra.push(text::admin::waiting(
            crate::state::admin::AdminPage::JoinRequests,
            group.waiting_join_requests,
        ));
    }
    txt::group_label(
        &group.shown_name,
        group.servers.len(),
        group.collapsed,
        &extra,
    )
}
