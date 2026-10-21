use gpui::{
    AnyElement, App, Context, FocusHandle, Global, Hsla, InteractiveElement, IntoElement,
    KeyBinding, ListAlignment, ListState, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, list, px,
};
use gpui_base::{Scrollbar, ScrollbarMode};

use crate::shell::Shell;
use crate::state::admin::view::{AdminView, Scope};
use crate::theme::{theme, u};

gpui::actions!(
    admin_list,
    [
        NextControl,
        PreviousControl,
        PageUp,
        PageDown,
        FirstRow,
        LastRow
    ]
);

struct Bindings;
impl Global for Bindings {}

pub(crate) fn init(cx: &mut App) {
    if cx.try_global::<Bindings>().is_none() {
        cx.set_global(Bindings);
        cx.bind_keys([
            KeyBinding::new("tab", NextControl, Some("AdminList")),
            KeyBinding::new("shift-tab", PreviousControl, Some("AdminList")),
            KeyBinding::new("pageup", PageUp, Some("AdminList")),
            KeyBinding::new("pagedown", PageDown, Some("AdminList")),
            KeyBinding::new("home", FirstRow, Some("AdminList")),
            KeyBinding::new("end", LastRow, Some("AdminList")),
        ]);
    }
}

pub(crate) struct PageList {
    pub(crate) state: ListState,
    pub(crate) drawn: bool,
    pub(crate) requested_end: bool,
    keys: Vec<String>,
    focus: Vec<FocusHandle>,
    ends: Vec<FocusHandle>,
    pub(crate) interactive: Vec<bool>,
    pending_focus: Option<(usize, bool)>,
    view: Option<AdminView>,
}

impl Default for PageList {
    fn default() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(800.0)),
            drawn: false,
            requested_end: false,
            keys: Vec::new(),
            focus: Vec::new(),
            ends: Vec::new(),
            interactive: Vec::new(),
            pending_focus: None,
            view: None,
        }
    }
}

impl PageList {
    pub(crate) fn take(&mut self, view: &AdminView, keys: Vec<String>, cx: &mut Context<Shell>) {
        let prefix = self
            .keys
            .iter()
            .zip(&keys)
            .take_while(|(a, b)| a == b)
            .count();
        let suffix = self.keys[prefix..]
            .iter()
            .rev()
            .zip(keys[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let old_end = self.keys.len() - suffix;
        let new_end = keys.len() - suffix;
        if prefix != old_end || prefix != new_end {
            let added: Vec<_> = (prefix..new_end)
                .map(|_| cx.focus_handle().tab_stop(false))
                .collect();
            self.state
                .splice_focusable(prefix..old_end, added.iter().cloned().map(Some));
            self.state = self.state.clone().with_uniform_item_height(px(40.0));
            self.focus.splice(prefix..old_end, added);
            self.ends.splice(
                prefix..old_end,
                (prefix..new_end).map(|_| cx.focus_handle().tab_stop(false)),
            );
        }
        if self.view.as_ref() != Some(view) {
            self.state.remeasure_items(0..keys.len());
            self.view = Some(view.clone());
        }
        self.keys = keys;
        self.interactive = vec![true; self.keys.len()];
        if self.requested_end {
            self.state.scroll_to_end();
            self.requested_end = false;
        }
    }
}

impl Shell {
    #[cfg(feature = "test-support")]
    pub fn focus_admin_page(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.admin.list.state.scroll_to(gpui::ListOffset::default());
        self.admin.list.pending_focus = Some((0, false));
        cx.notify();
    }

    fn tab_admin_row(
        &mut self,
        index: usize,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if reverse {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        if self.admin.list.focus[index].contains_focused(window, cx) {
            return;
        }
        let next = if reverse {
            (0..index)
                .rev()
                .find(|&index| self.admin.list.interactive[index])
        } else {
            (index + 1..self.admin.list.keys.len())
                .find(|&index| self.admin.list.interactive[index])
        };
        if let Some(next) = next {
            self.admin.list.state.scroll_to_reveal_item(next);
            self.admin.list.pending_focus = Some((next, reverse));
            cx.notify();
        } else {
            let trap = self.layer_focus().clone();
            for _ in 0..100 {
                if trap.contains_focused(window, cx) {
                    break;
                }
                if reverse {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
            }
        }
    }

    pub(crate) fn render_admin_list(
        &mut self,
        render: impl Fn(&mut Self, usize, &mut Window, &mut Context<Self>) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.admin.list.drawn = true;
        let state = self.admin.list.state.clone();
        let focus = self.admin.list.focus.clone();
        let ends = self.admin.list.ends.clone();
        let count = focus.len();
        let keys = self.admin.list.keys.clone();
        let colors = theme(cx).colors;
        let label = match self.admin.list.view.as_ref().map(|view| &view.scope) {
            Some(Scope::Server { selected, .. }) => crate::text::admin::page_name(*selected),
            Some(Scope::Channel(settings)) => {
                crate::text::admin::channel_page_name(settings.selected)
            }
            None => crate::text::admin::TITLE,
        };
        let render = cx.processor(move |this, index: usize, window, cx| {
            let content = render(this, index, window, cx);
            if let Some((pending, reverse)) = this.admin.list.pending_focus
                && pending == index
            {
                this.admin.list.pending_focus = None;
                let key = this.admin.list.keys[index].clone();
                cx.on_next_frame(window, move |this, window, cx| {
                    if this.admin.list.keys.get(index) != Some(&key) {
                        return;
                    }
                    this.admin.list.state.scroll_to_reveal_item(index);
                    cx.notify();
                    let handle = if reverse {
                        &this.admin.list.ends[index]
                    } else {
                        &this.admin.list.focus[index]
                    };
                    window.focus(handle, cx);
                    this.tab_admin_row(index, reverse, window, cx);
                });
            }
            div()
                .id(SharedString::from(keys[index].clone()))
                .track_focus(&focus[index])
                .key_context("AdminList")
                .on_action(cx.listener(move |this, _: &NextControl, window, cx| {
                    this.tab_admin_row(index, false, window, cx)
                }))
                .on_action(cx.listener(move |this, _: &PreviousControl, window, cx| {
                    this.tab_admin_row(index, true, window, cx)
                }))
                .on_action(cx.listener(|this, _: &PageUp, _, cx| {
                    let list = &this.admin.list.state;
                    list.scroll_by(-list.viewport_bounds().size.height * 0.8);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &PageDown, _, cx| {
                    let list = &this.admin.list.state;
                    list.scroll_by(list.viewport_bounds().size.height * 0.8);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &FirstRow, _, cx| {
                    this.admin.list.state.scroll_to(gpui::ListOffset::default());
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &LastRow, _, cx| {
                    this.admin.list.state.scroll_to_end();
                    cx.notify();
                }))
                .px(u(18.0))
                .pt(if index == 0 { u(14.0) } else { u(0.0) })
                .pb(if index + 1 == count { u(18.0) } else { u(0.0) })
                .child(content)
                .child(
                    div()
                        .id(("admin-list-end", index))
                        .track_focus(&ends[index])
                        .size_0(),
                )
                .into_any_element()
        });
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("admin-page")
                    .role(Role::List)
                    .aria_label(label)
                    .debug_selector(|| "admin-page".to_owned())
                    .size_full()
                    .child(list(state.clone(), render).size_full()),
            )
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::vertical(&state)
                        .viewport_from_layout()
                        .mode(ScrollbarMode::Always)
                        .styles(|styles| {
                            styles
                                .track(|track| track.bg(gpui::transparent_black()).width(px(10.0)))
                                .track_hover(|track| track.bg(gpui::transparent_black()))
                                .track_active(|track| track.bg(gpui::transparent_black()))
                                .thumb(|bar| {
                                    bar.bg(Hsla::from(colors.rule))
                                        .width(px(4.0))
                                        .radius(px(2.0))
                                        .inset(px(3.0))
                                })
                                .thumb_hover(|bar| bar.bg(Hsla::from(colors.ink_3)))
                                .thumb_active(|bar| bar.bg(Hsla::from(colors.ink_3)))
                        }),
                ),
            )
            .into_any_element()
    }
}
