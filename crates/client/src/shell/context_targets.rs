use std::rc::Rc;

use gpui::{
    Context, Div, InteractiveElement, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window,
};
use junto_client_core::{Owner, protocol::ids::ServerId};

use super::Shell;
use crate::{
    state::{AppState, view::ShellView},
    text,
    ui::{RowAction, bind_lazy_action_menu, collect_actions},
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

    pub(super) fn bind_server_context(
        &mut self,
        frame: Stateful<Div>,
        surface: &'static str,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let Some(server) = self.state.read(cx).server(server_id) else {
            return frame;
        };
        let key = server.entry().key();
        let owner = server.owner();
        bind_lazy_action_menu(
            frame.focusable().tab_stop(true),
            SharedString::from(format!("server-context:{surface}:{key:?}:{owner:?}")),
            window,
            cx,
            |window, cx| {
                if !matches!(self.layer, super::Layer::None) {
                    return Vec::new();
                }
                let view = self.state.read(cx).view_for_server(server_id);
                let Some(selected) = &view.selected else {
                    return Vec::new();
                };
                let name = selected.name.clone();
                let (_, mut actions) =
                    collect_actions(|| self.render_server_menu(&view, window, cx));
                actions.insert(
                    0,
                    RowAction::new(
                        text::COPY_NAME,
                        cx.listener(move |this, _, _, cx| this.copy_text(name.clone(), cx)),
                    ),
                );
                self.guard_main_context_actions(
                    &mut actions,
                    move |state| {
                        state.server(server_id).is_some_and(|server| {
                            server.entry().key() == key && server.owner() == owner
                        })
                    },
                    cx,
                );
                actions
            },
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
                    let (_, existing) =
                        collect_actions(|| self.render_workspace_menu(row, position, cx));
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
