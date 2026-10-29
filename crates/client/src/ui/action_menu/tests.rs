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
