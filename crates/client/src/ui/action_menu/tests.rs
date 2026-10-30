use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton,
    ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window, div,
};

use super::{bind_action_menu, collect_actions};
use crate::ui::{Button, Dialog};

struct Host {
    trap: FocusHandle,
    row: FocusHandle,
    calls: usize,
    step: usize,
}

impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.step;
        let (frame, actions) = collect_actions(|| {
            div()
                .id("action-row")
                .debug_selector(|| "action-row".to_owned())
                .track_focus(&self.row)
                .child(
                    Button::new("rename")
                        .label("Rename")
                        .selector("rename")
                        .on_click(cx.listener(move |this, _, _, _| this.calls += step)),
                )
                .child(
                    Button::new("delete")
                        .label("Delete")
                        .disabled(true)
                        .tooltip("Permission required")
                        .on_click(cx.listener(|this, _, _, _| this.calls += 100)),
                )
        });
        let actions = if step == 0 { Vec::new() } else { actions };
        let row = bind_action_menu(frame, "action-row-menu", actions, window, cx);
        div()
            .relative()
            .size_full()
            .child(Dialog::new("Actions", &self.trap).child(row))
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            return;
        }
    }
    panic!("row menu did not settle");
}

fn open(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| Host {
        trap: cx.focus_handle(),
        row: cx.focus_handle().tab_stop(true),
        calls: 0,
        step: 1,
    });
    settle(cx);
    let focus = host.read_with(cx, |host, _| host.row.clone());
    cx.update(|window, cx| focus.focus(window, cx));
    settle(cx);
    (host, cx)
}

fn click(cx: &mut VisualTestContext, selector: &'static str, button: MouseButton) {
    let at = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_down(at, button, Modifiers::none());
    cx.simulate_mouse_up(at, button, Modifiers::none());
    settle(cx);
}

#[gpui::test]
fn a_row_context_menu_uses_the_same_action_as_its_button(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    click(cx, "rename", MouseButton::Left);
    assert_eq!(host.read_with(cx, |host, _| host.calls), 1);
    click(cx, "action-row", MouseButton::Right);
    assert!(cx.debug_bounds("row-context-menu").is_some());
    host.update(cx, |host, cx| {
        host.step = 2;
        cx.notify();
    });
    settle(cx);
    click(cx, "context-action:Rename", MouseButton::Left);
    assert_eq!(host.read_with(cx, |host, _| host.calls), 3);
    assert!(cx.debug_bounds("row-context-menu").is_none());
    assert!(cx.debug_bounds("rename").is_some());
}

#[gpui::test]
fn a_keyboard_row_menu_preserves_disabled_actions_and_its_form(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    cx.simulate_keystrokes("shift-f10");
    settle(cx);
    assert!(cx.debug_bounds("row-context-menu").is_some());
    click(cx, "context-action:Delete", MouseButton::Left);
    assert_eq!(host.read_with(cx, |host, _| host.calls), 0);
    assert!(cx.debug_bounds("row-context-menu").is_some());
    cx.simulate_keystrokes("menu");
    settle(cx);
    cx.simulate_keystrokes("escape");
    settle(cx);
    let focus = host.read_with(cx, |host, _| host.row.clone());
    assert!(cx.update(|window, _| focus.is_focused(window)));
    assert!(cx.debug_bounds("rename").is_some());
}

struct ChoiceHost {
    on: bool,
    choice: usize,
    inspected: bool,
}

impl Render for ChoiceHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (row, actions) = collect_actions(|| {
            div()
                .id("choice-row")
                .child(
                    div()
                        .debug_selector(|| "choice-label".to_owned())
                        .child("Options"),
                )
                .child(
                    crate::ui::Toggle::new("choice-toggle", "Enabled", self.on, ("On", "Off"))
                        .selector("choice-toggle")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.on = !this.on;
                            cx.notify();
                        })),
                )
                .child(
                    crate::ui::Segmented::new(
                        "choice-policy",
                        "Policy",
                        ["Allow".into(), "Deny".into()],
                    )
                    .selected(Some(self.choice))
                    .selector("choice-policy")
                    .on_select({
                        let select = cx.listener(|this, index: &usize, _, cx| {
                            this.choice = *index;
                            cx.notify();
                        });
                        move |index, window, cx| select(&index, window, cx)
                    }),
                )
                .child(
                    crate::ui::MenuItem::new("choice-inspect", "Inspect").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.inspected = true;
                            cx.notify();
                        },
                    )),
                )
        });
        bind_action_menu(row, "choice-menu", actions, window, cx)
    }
}

#[gpui::test]
fn choice_controls_and_menu_rows_expose_their_existing_actions(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, _| ChoiceHost {
        on: false,
        choice: 0,
        inspected: false,
    });
    settle(cx);
    click(cx, "choice-label", MouseButton::Right);
    assert!(cx.debug_bounds("context-action:Enabled: On").is_some());
    click(cx, "context-action:Enabled: On", MouseButton::Left);
    assert!(host.read_with(cx, |host, _| host.on));
    click(cx, "choice-label", MouseButton::Right);
    click(cx, "context-action:Policy: Deny", MouseButton::Left);
    assert_eq!(host.read_with(cx, |host, _| host.choice), 1);
    click(cx, "choice-label", MouseButton::Right);
    click(cx, "context-action:Inspect", MouseButton::Left);
    assert!(host.read_with(cx, |host, _| host.inspected));
    click(cx, "choice-toggle", MouseButton::Right);
    click(cx, "context-action:Enabled: Off", MouseButton::Left);
    assert!(!host.read_with(cx, |host, _| host.on));
    click(cx, "choice-policy-1", MouseButton::Right);
    click(cx, "context-action:Policy: Allow", MouseButton::Left);
    assert_eq!(host.read_with(cx, |host, _| host.choice), 0);
}

struct NestedHost {
    focus: FocusHandle,
    calls: usize,
    closed: bool,
}

impl Render for NestedHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.closed {
            return div().into_any_element();
        }
        let (row, actions) = collect_actions(|| {
            div()
                .id("nested-choice")
                .debug_selector(|| "nested-choice".to_owned())
                .child(
                    crate::ui::MenuItem::new("nested-copy", "Copy metadata").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.calls += 1;
                            cx.notify();
                        },
                    )),
                )
        });
        let row = bind_action_menu(row, "nested-choice-menu", actions, window, cx);
        let close = cx.listener(|this, _: &(), _, cx| {
            this.closed = true;
            cx.notify();
        });
        crate::ui::Menu::new("outer-choices", "Files", &self.focus)
            .width(180.0)
            .on_close(move |window, cx| close(&(), window, cx))
            .child(row)
            .popup_at(gpui::point(gpui::px(20.0), gpui::px(20.0)))
    }
}

#[gpui::test]
fn a_context_action_of_a_menu_row_keeps_the_enclosing_list_open(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| NestedHost {
        focus: cx.focus_handle(),
        calls: 0,
        closed: false,
    });
    settle(cx);
    click(cx, "nested-choice", MouseButton::Right);
    click(cx, "context-action:Copy metadata", MouseButton::Left);
    assert_eq!(host.read_with(cx, |host, _| host.calls), 1);
    assert!(!host.read_with(cx, |host, _| host.closed));
    click(cx, "nested-choice", MouseButton::Right);
    cx.simulate_keystrokes("escape");
    settle(cx);
    assert!(!host.read_with(cx, |host, _| host.closed));
    assert!(cx.debug_bounds("row-context-menu").is_none());
    cx.simulate_mouse_down(
        gpui::point(gpui::px(2.0), gpui::px(2.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        gpui::point(gpui::px(2.0), gpui::px(2.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    settle(cx);
    assert!(host.read_with(cx, |host, _| host.closed));
}

#[gpui::test]
fn losing_all_actions_closes_the_menu_restores_focus_and_does_not_reopen_it(
    cx: &mut TestAppContext,
) {
    let (host, cx) = open(cx);
    cx.simulate_keystrokes("menu");
    settle(cx);
    assert!(cx.debug_bounds("row-context-menu").is_some());
    host.update(cx, |host, cx| {
        host.step = 0;
        cx.notify();
    });
    settle(cx);
    assert!(cx.debug_bounds("row-context-menu").is_none());
    let focus = host.read_with(cx, |host, _| host.row.clone());
    assert!(cx.update(|window, _| focus.is_focused(window)));
    host.update(cx, |host, cx| {
        host.step = 1;
        cx.notify();
    });
    settle(cx);
    assert!(cx.debug_bounds("row-context-menu").is_none());
    cx.simulate_keystrokes("shift-f10");
    settle(cx);
    assert!(cx.debug_bounds("row-context-menu").is_some());
}
