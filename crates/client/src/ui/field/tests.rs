use gpui::{
    Context, Entity, FocusHandle, Focusable, IntoElement, Modifiers, MouseButton, ParentElement,
    Render, Styled, TestAppContext, VisualTestContext, Window, div, point, px,
};
use gpui_base::input::InputState;

use crate::ui::{Dialog, Field};

struct Host {
    focus: FocusHandle,
    input: Entity<InputState>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(
            Dialog::new("Edit", &self.focus)
                .child(Field::new(&self.input, "Value").selector("sample-field")),
        )
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            cx.run_until_parked();
            return;
        }
    }
    panic!("the input-menu window did not settle");
}

fn open(
    cx: &mut TestAppContext,
    readonly: bool,
    masked: bool,
) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|window, cx| {
        let input = Field::state("Value", window, cx);
        input.update(cx, |input, cx| {
            input.set_value("Sample text", window, cx);
            input.set_readonly(readonly, cx);
            input.set_masked(masked, window, cx);
        });
        Host {
            focus: cx.focus_handle(),
            input,
        }
    });
    let focus = host.read_with(cx, |host, cx| host.input.read(cx).focus_handle(cx));
    cx.update(|window, cx| focus.focus(window, cx));
    settle(cx);
    cx.simulate_keystrokes(if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    });
    settle(cx);
    (host, cx)
}

fn menu(cx: &mut VisualTestContext) {
    let bounds = cx.debug_bounds("sample-field").unwrap();
    let at = point(bounds.left() + px(15.0), bounds.center().y);
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    settle(cx);
    assert!(cx.debug_bounds("input-edit-menu").is_some());
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
    settle(cx);
}

#[gpui::test]
fn a_field_context_menu_copies_and_pastes_the_selection_without_closing_its_dialog(
    cx: &mut TestAppContext,
) {
    let (host, cx) = open(cx, false, false);
    menu(cx);
    click(cx, "input-edit-copy");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("Sample text")
    );
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("Replacement".to_owned()));
    menu(cx);
    click(cx, "input-edit-paste");
    assert_eq!(
        host.read_with(cx, |host, cx| host.input.read(cx).value().to_string()),
        "Replacement"
    );
    assert!(cx.debug_bounds("sample-field").is_some());
}

#[gpui::test]
fn a_readonly_field_menu_does_not_paste_or_cut(cx: &mut TestAppContext) {
    let (host, cx) = open(cx, true, false);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("Replacement".to_owned()));
    menu(cx);
    for selector in ["input-edit-cut", "input-edit-paste"] {
        click(cx, selector);
        assert_eq!(
            host.read_with(cx, |host, cx| host.input.read(cx).value().to_string()),
            "Sample text"
        );
        assert!(cx.debug_bounds("input-edit-menu").is_some());
    }
    click(cx, "input-edit-copy");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("Sample text")
    );
}

#[gpui::test]
fn a_masked_field_menu_does_not_copy_or_cut_its_value(cx: &mut TestAppContext) {
    let (host, cx) = open(cx, false, true);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("Kept".to_owned()));
    menu(cx);
    for selector in ["input-edit-copy", "input-edit-cut"] {
        click(cx, selector);
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("Kept")
        );
        assert_eq!(
            host.read_with(cx, |host, cx| host.input.read(cx).value().to_string()),
            "Sample text"
        );
    }
}

#[gpui::test]
fn a_keyboard_field_menu_selects_all_and_cuts_its_text(cx: &mut TestAppContext) {
    let (host, cx) = open(cx, false, false);
    cx.simulate_keystrokes("right");
    settle(cx);
    cx.simulate_keystrokes("shift-f10");
    settle(cx);
    assert!(cx.debug_bounds("input-edit-menu").is_some());
    click(cx, "input-edit-select-all");
    cx.simulate_keystrokes("menu");
    settle(cx);
    click(cx, "input-edit-cut");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("Sample text")
    );
    assert_eq!(
        host.read_with(cx, |host, cx| host.input.read(cx).value().to_string()),
        ""
    );
    assert!(cx.debug_bounds("sample-field").is_some());
    assert!(cx.debug_bounds("input-edit-menu").is_none());
}
