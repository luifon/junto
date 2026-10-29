use super::TextTargets;
use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton,
    ParentElement, Render, Styled, StyledText, TestAppContext, TextLayout, VisualTestContext,
    Window, div, point, px,
};

struct Host {
    focus: FocusHandle,
    layout: Option<TextLayout>,
    chosen: Vec<Vec<usize>>,
    parent: usize,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host = cx.entity().clone();
        let text = StyledText::new("Hello Rin and Noor, goodbye");
        let layout = text.layout().clone();
        self.layout = Some(layout.clone());
        div()
            .size_full()
            .p(px(20.0))
            .text_size(px(16.0))
            .line_height(px(22.0))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.parent += 1),
            )
            .child(
                TextTargets::new(
                    "people",
                    "People mentioned",
                    layout,
                    vec![6..9, 14..18],
                    text,
                )
                .focus(&self.focus)
                .on_menu(move |chosen, _, _, cx| {
                    host.update(cx, |this, _| this.chosen.push(chosen))
                }),
            )
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            return;
        }
    }
    panic!("text targets did not settle");
}

fn open(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| Host {
        focus: cx.focus_handle().tab_stop(true),
        layout: None,
        chosen: Vec::new(),
        parent: 0,
    });
    settle(cx);
    (host, cx)
}

#[gpui::test]
fn a_text_target_uses_its_painted_range_and_leaves_plain_text_to_its_parent(
    cx: &mut TestAppContext,
) {
    let (host, cx) = open(cx);
    for index in [15, 2] {
        let at = host.read_with(cx, |host, _| {
            let at = host
                .layout
                .as_ref()
                .unwrap()
                .position_for_index(index)
                .unwrap();
            point(at.x + px(2.0), at.y + px(8.0))
        });
        cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
        settle(cx);
    }
    assert_eq!(
        host.read_with(cx, |host, _| host.chosen.clone()),
        vec![vec![1]]
    );
    assert_eq!(host.read_with(cx, |host, _| host.parent), 1);
}

#[gpui::test]
fn a_keyboard_text_target_offers_all_ranges_in_document_order(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    let focus = host.read_with(cx, |host, _| host.focus.clone());
    cx.update(|window, cx| focus.focus(window, cx));
    for key in ["shift-f10", "menu", "enter", "space"] {
        cx.simulate_keystrokes(key);
        settle(cx);
    }
    assert_eq!(
        host.read_with(cx, |host, _| host.chosen.clone()),
        vec![vec![0, 1]; 4]
    );
}
