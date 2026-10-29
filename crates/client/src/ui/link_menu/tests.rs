use super::{CopyToClipboard, LinkMenu, OpenLink};
use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, InteractiveText, IntoElement, Modifiers,
    MouseButton, ParentElement, Render, Styled, StyledText, TestAppContext, TextLayout,
    VisualTestContext, Window, div, point, px,
};
use junto_client_core::link::Link;

struct Host {
    focus: FocusHandle,
    source: FocusHandle,
    layout: Option<TextLayout>,
    parent: usize,
    changed: bool,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let links = vec![
            (
                6..11,
                Link::parse(if self.changed {
                    "https://example.test/guide-v2"
                } else {
                    "https://example.test/guide"
                })
                .unwrap(),
            ),
            (16..21, Link::parse("https://example.test/terms").unwrap()),
        ];
        let styled = StyledText::new("Start guide and terms end");
        let layout = styled.layout().clone();
        self.layout = Some(layout.clone());
        let ranges = links.iter().map(|(range, _)| range.clone()).collect();
        let clicked = links.clone();
        let text = InteractiveText::new("text", styled).on_click(ranges, move |index, _, cx| {
            cx.open_url(clicked[index].1.as_str())
        });
        div()
            .id("host")
            .size_full()
            .p(px(20.0))
            .text_size(px(16.0))
            .line_height(px(22.0))
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.parent += 1),
            )
            .on_action(cx.listener(|_, action: &CopyToClipboard, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(action.text.clone()))
            }))
            .on_action(cx.listener(|_, action: &OpenLink, _, cx| {
                cx.open_url(action.link.as_str());
            }))
            .child(LinkMenu::text("links", layout, links, text).focus(&self.source))
            .child(LinkMenu::control(
                "control",
                Link::parse("https://example.test/support").unwrap(),
                crate::ui::Button::new("support")
                    .label("Support")
                    .selector("control-link"),
            ))
    }
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        if cx.update(|window, cx| window.simulate_next_frame(cx)) == 0 {
            return;
        }
    }
    panic!("link menu did not settle");
}

fn open(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| Host {
        focus: cx.focus_handle(),
        source: cx.focus_handle().tab_stop(true),
        layout: None,
        parent: 0,
        changed: false,
    });
    settle(cx);
    (host, cx)
}

fn press(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>, button: MouseButton) {
    cx.simulate_mouse_down(at, button, Modifiers::none());
    cx.simulate_mouse_up(at, button, Modifiers::none());
    settle(cx);
}

fn at(host: &Entity<Host>, cx: &mut VisualTestContext, index: usize) -> gpui::Point<gpui::Pixels> {
    host.read_with(cx, |host, _| {
        host.layout
            .as_ref()
            .unwrap()
            .position_for_index(index)
            .unwrap()
            + point(px(1.0), px(5.0))
    })
}

#[gpui::test]
fn a_link_right_click_opens_its_menu_without_opening_the_url_or_parent_menu(
    cx: &mut TestAppContext,
) {
    let (host, cx) = open(cx);
    let position = at(&host, cx, 17);
    press(cx, position, MouseButton::Right);
    assert_eq!(cx.opened_url(), None);
    assert!(cx.debug_bounds("link-menu").is_some());
    assert_eq!(host.read_with(cx, |host, _| host.parent), 0);
    let item = cx.debug_bounds("link-copy:0").unwrap().center();
    press(cx, item, MouseButton::Left);
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().unwrap(),
        "https://example.test/terms"
    );
    assert!(cx.debug_bounds("link-menu").is_none());
    let position = at(&host, cx, 1);
    press(cx, position, MouseButton::Right);
    assert_eq!(host.read_with(cx, |host, _| host.parent), 1);
    assert!(cx.debug_bounds("link-menu").is_none());
}

#[gpui::test]
fn a_link_menu_opens_the_clicked_target(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    let position = at(&host, cx, 7);
    press(cx, position, MouseButton::Right);
    let item = cx.debug_bounds("link-open:0").unwrap().center();
    press(cx, item, MouseButton::Left);
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.test/guide")
    );
}

#[gpui::test]
fn a_keyboard_link_menu_lists_each_target_and_restores_the_text_focus(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    let source = host.read_with(cx, |host, _| host.source.clone());
    cx.update(|window, cx| source.focus(window, cx));
    settle(cx);
    cx.simulate_keystrokes("shift-f10");
    settle(cx);
    for _ in 0..4 {
        cx.simulate_keystrokes("down");
        settle(cx);
    }
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    settle(cx);
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().unwrap(),
        "https://example.test/terms"
    );
    assert!(cx.update(|window, _| source.is_focused(window)));
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(cx.debug_bounds("link-menu").is_some());
}

#[gpui::test]
fn a_link_control_has_the_same_copy_menu(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    let position = cx.debug_bounds("control-link").unwrap().center();
    press(cx, position, MouseButton::Right);
    let item = cx.debug_bounds("link-copy:0").unwrap().center();
    press(cx, item, MouseButton::Left);
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().unwrap(),
        "https://example.test/support"
    );
    assert_eq!(host.read_with(cx, |host, _| host.parent), 0);
}

#[gpui::test]
fn a_changed_link_target_closes_its_old_menu(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    let position = at(&host, cx, 7);
    press(cx, position, MouseButton::Right);
    assert!(cx.debug_bounds("link-menu").is_some());
    host.update(cx, |host, cx| {
        host.changed = true;
        cx.notify();
    });
    settle(cx);
    assert!(cx.debug_bounds("link-menu").is_none());
}
