use std::rc::Rc;

use gpui::{
    Context, Div, InteractiveElement, ParentElement, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::{Owner, protocol::ids::ServerId};

use super::{MenuAt, Shell};
use crate::{
    state::{AppState, view::ShellView},
    text,
    ui::{ContextMenuExt, MENU_ROW, MenuButton, RowAction, bind_lazy_action_menu, collect_actions},
};

impl Shell {
    pub(super) fn guard_main_context_actions(
        &self,
        actions: &mut [RowAction],
        current: impl Fn(&AppState) -> bool + 'static,
        cx: &Context<Self>,
    ) {
        let current = Rc::new(current);
        for action in actions {
            if let Some(handler) = action.handler.take() {
                let shell = cx.entity().downgrade();
                let current = current.clone();
                action.handler = Some(Rc::new(move |event, window, cx| {
                    if shell.upgrade().is_some_and(|shell| {
                        let this = shell.read(cx);
                        matches!(this.layer, super::Layer::None) && current(this.state.read(cx))
                    }) {
                        handler(event, window, cx);
                    }
                }));
            }
        }
    }

    /// The menu of a server on a surface that shows the server: its row and its icon in the
    /// server book, and the title of the channel column of a server that shows no workspace.
    /// A secondary press, the Menu key and Shift+F10 open it; the row also shows the menu
    /// button while the pointer is over it, while it holds the keyboard focus and while the
    /// menu is open (ADR 0731).
    pub(super) fn bind_server_context(
        &mut self,
        frame: Stateful<Div>,
        surface: &'static str,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let at = match surface {
            "server-row" | "server-icon" => MenuAt::Row,
            _ => MenuAt::Header,
        };
        let open = Rc::new(Self::handler(cx, move |this, window, cx| {
            this.open_server_menu_of(server_id, at, window, cx);
        }));
        let frame = frame.focusable().tab_stop(true);
        if surface != "server-row" {
            let open = open.clone();
            return frame.on_context_menu(move |window, cx| open(window, cx));
        }
        let focus = crate::ui::row_focus(
            ("server-row-focus", server_id.as_uuid().as_u128() as u64),
            window,
            cx,
        );
        let shown = matches!(self.layer, super::Layer::ServerMenu(open, MenuAt::Row) if open == server_id)
            || focus.is_focused(window);
        let by_menu = open.clone();
        frame
            .track_focus(&focus)
            .group(MENU_ROW)
            .on_context_menu(move |window, cx| by_menu(window, cx))
            .child(
                div().flex_none().child(
                    MenuButton::new(("server-menu-button", server_id.as_uuid().as_u128() as u64))
                        .selector(format!("menu-button:server:{server_id}"))
                        .shown(shown)
                        .dark()
                        .on_open(move |window, cx| open(window, cx)),
                ),
            )
    }

    pub(super) fn bind_workspace_title_context(
        &mut self,
        frame: Stateful<Div>,
        view: &ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(server_id) = view.selected.as_ref().map(|server| server.id) else {
            return frame;
        };
        let Some(workspace) = view.workspace() else {
            let theme = crate::theme::theme(cx);
            let ring = crate::ui::focus_ring_inside(
                theme.colors.terracotta_deep,
                theme.metrics.focus_ring,
            );
            return self.bind_server_context(
                frame
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(move |style| style.shadow(ring)),
                "workspace-server-title",
                server_id,
                window,
                cx,
            );
        };
        let Some(server) = self.state.read(cx).server(server_id) else {
            return frame;
        };
        let key = server.entry().key();
        let owner = server.owner();
        let workspace_id = workspace.id;
        let name = workspace.name.clone();
        let theme = crate::theme::theme(cx);
        let ring =
            crate::ui::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
        bind_lazy_action_menu(
            frame
                .focusable()
                .tab_stop(true)
                .focus_visible(move |style| style.shadow(ring)),
            SharedString::from(format!(
                "workspace-title-context:{key:?}:{owner:?}:{workspace_id}"
            )),
            window,
            cx,
            |_, cx| {
                if !matches!(self.layer, super::Layer::None) {
                    return Vec::new();
                }
                let mut actions = vec![RowAction::new(
                    text::COPY_NAME,
                    cx.listener(move |this, _, _, cx| this.copy_text(name.clone(), cx)),
                )];
                if let Some(row) = view.servers.iter().find(|row| row.id == server_id)
                    && let Some(position) = row
                        .workspaces
                        .iter()
                        .position(|workspace| workspace.id == workspace_id)
                {
                    let (_, existing) = collect_actions(|| {
                        self.render_workspace_menu_items(row, position, MenuAt::Header, cx)
                    });
                    actions.extend(existing);
                }
                self.guard_main_context_actions(
                    &mut actions,
                    move |state| {
                        let view = state.view();
                        state.server(server_id).is_some_and(|server| {
                            server.entry().key() == key && server.owner() == owner
                        }) && view
                            .selected
                            .as_ref()
                            .is_some_and(|server| server.id == server_id)
                            && view
                                .workspace()
                                .is_some_and(|workspace| workspace.id == workspace_id)
                    },
                    cx,
                );
                actions
            },
        )
    }

    pub(super) fn bind_main_title_context(
        &mut self,
        frame: Stateful<Div>,
        view: &ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(server_id) = view.selected.as_ref().map(|server| server.id) else {
            return frame;
        };
        let Some(owner) = self
            .state
            .read(cx)
            .server(server_id)
            .and_then(|server| server.owner())
        else {
            return frame;
        };
        let detached = self.detached;
        let Some(place) = view
            .lists()
            .and_then(|lists| lists.conversation.as_ref())
            .map(|direct| junto_client_core::model::PlaceId::Direct(direct.id))
            .or_else(|| {
                view.workspace()
                    .and_then(|workspace| workspace.channel.as_ref())
                    .map(|channel| junto_client_core::model::PlaceId::Channel(channel.id))
            })
        else {
            return frame;
        };
        let theme = crate::theme::theme(cx);
        let ring =
            crate::ui::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
        bind_lazy_action_menu(
            frame
                .focusable()
                .tab_stop(true)
                .focus_visible(move |style| style.shadow(ring)),
            SharedString::from(format!("main-title-context:{owner:?}:{place:?}")),
            window,
            cx,
            |_, cx| {
                if !matches!(self.layer, super::Layer::None) {
                    return Vec::new();
                }
                let (_, mut actions) = if let Some(direct) =
                    view.lists().and_then(|lists| lists.conversation.as_ref())
                {
                    let (_, actions) =
                        collect_actions(|| self.render_direct_menu(server_id, direct, cx));
                    (
                        junto_client_core::model::PlaceId::Direct(direct.id),
                        actions,
                    )
                } else if let Some(workspace) = view.workspace()
                    && let Some(channel) = &workspace.channel
                {
                    let (_, actions) = collect_actions(|| {
                        self.render_channel_menu(server_id, workspace.id, channel, cx)
                    });
                    (
                        junto_client_core::model::PlaceId::Channel(channel.id),
                        actions,
                    )
                } else {
                    return Vec::new();
                };
                self.guard_main_context_actions(
                    &mut actions,
                    move |state| {
                        use junto_client_core::model::PlaceId;
                        if !owner_current(state, owner) {
                            return false;
                        }
                        let view = if detached {
                            state.call_view()
                        } else {
                            state.view()
                        };
                        view.selected
                            .as_ref()
                            .is_some_and(|server| server.id == server_id)
                            && match place {
                                PlaceId::Direct(id) => view
                                    .lists()
                                    .and_then(|lists| lists.conversation.as_ref())
                                    .is_some_and(|direct| direct.id == id),
                                PlaceId::Channel(id) => {
                                    view.lists()
                                        .is_some_and(|lists| lists.conversation.is_none())
                                        && view
                                            .workspace()
                                            .and_then(|workspace| workspace.channel.as_ref())
                                            .is_some_and(|channel| channel.id == id)
                                }
                                PlaceId::Thread { .. } => false,
                            }
                    },
                    cx,
                );
                actions
            },
        )
    }
}

fn owner_current(state: &AppState, owner: Owner) -> bool {
    state
        .server(owner.entry.server_id)
        .and_then(|server| server.owner())
        == Some(owner)
}
