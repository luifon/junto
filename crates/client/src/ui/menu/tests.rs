use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, Modifiers, MouseButton,
    ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window, div, px,
};

use crate::assets::Icon;
use crate::ui::ChannelRow;

struct Host {
    parent: usize,
    child: usize,
    focus: Option<FocusHandle>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let child = cx.listener(|this, _: &(), window, cx| {
            this.focus = window.focused(cx);
            this.child += 1;
        });
        div()
            .id("parent")
            .size(px(400.0))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, _| this.parent += 1),
            )
            .child(
                ChannelRow::new("child", Icon::Hash, "Planning")
                    .selector("child")
                    .on_menu(move |window, cx| child(&(), window, cx)),
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
    panic!("menu test did not settle");
}

fn open(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, _| Host {
        parent: 0,
        child: 0,
        focus: None,
    });
    settle(cx);
    (host, cx)
}

#[gpui::test]
fn a_nested_channel_menu_does_not_open_the_parent_menu(cx: &mut TestAppContext) {
    let (host, cx) = open(cx);
    let at = cx.debug_bounds("child").unwrap().center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    settle(cx);
    let counts = host.read_with(cx, |host, _| (host.parent, host.child));
    assert_eq!(counts, (0, 1));
}

use super::{ContextMenuExt, Menu, MenuItem};
use gpui::point;

struct PopupHost {
    trigger: FocusHandle,
    menu: FocusHandle,
    opened: bool,
    chosen: Option<usize>,
    header: Option<FocusHandle>,
}

impl PopupHost {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.opened = false;
        self.trigger.focus(window, cx);
        cx.notify();
    }
}

impl Render for PopupHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open = cx.listener(|this, _: &(), window, cx| {
            this.opened = true;
            this.menu.focus(window, cx);
            cx.notify();
        });
        let menu = self.opened.then(|| {
            let close = cx.listener(|this, _: &(), window, cx| this.close(window, cx));
            let mut menu = Menu::new("test-menu", "Actions", &self.menu)
                .selector("test-menu")
                .on_close(move |window, cx| close(&(), window, cx))
                .children((0..40).map(|index| {
                    MenuItem::new(("item", index), format!("Action {index}"))
                        .selector(if index == 39 { "last-item" } else { "item" })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.chosen = Some(index);
                            this.close(window, cx);
                        }))
                }));
            if let Some(header) = &self.header {
                menu = menu.header(
                    div()
                        .id("profile-header")
                        .debug_selector(|| "profile-header".to_owned())
                        .relative()
                        .h(px(40.0))
                        .track_focus(header)
                        .children(crate::ui::report_focus(header, window))
                        .child("Profile"),
                );
            }
            menu.popup(point(px(20.0), px(20.0)))
        });
        div().relative().size_full().child(
            div()
                .id("trigger")
                .debug_selector(|| "trigger".to_owned())
                .absolute()
                .right_0()
                .bottom_0()
                .size(px(20.0))
                .track_focus(&self.trigger)
                .on_context_menu(move |window, cx| open(&(), window, cx))
                .children(menu),
        )
    }
}

fn popup(cx: &mut TestAppContext) -> (Entity<PopupHost>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| PopupHost {
        trigger: cx.focus_handle().tab_stop(true),
        menu: cx.focus_handle(),
        opened: false,
        chosen: None,
        header: None,
    });
    let trigger = host.read_with(cx, |host, _| host.trigger.clone());
    cx.update(|window, cx| trigger.focus(window, cx));
    settle(cx);
    (host, cx)
}

#[gpui::test]
fn context_keys_open_and_escape_restores_the_target_focus(cx: &mut TestAppContext) {
    let (host, cx) = popup(cx);
    for key in ["shift-f10", "menu"] {
        cx.simulate_keystrokes(key);
        settle(cx);
        assert!(host.read_with(cx, |host, _| host.opened));
        cx.simulate_keystrokes("escape");
        settle(cx);
        assert!(!host.read_with(cx, |host, _| host.opened));
        let trigger = host.read_with(cx, |host, _| host.trigger.clone());
        assert!(cx.update(|window, _| trigger.is_focused(window)));
    }
}

#[gpui::test]
fn a_tall_popup_stays_in_the_window_and_arrows_reveal_its_last_action(cx: &mut TestAppContext) {
    let (host, cx) = popup(cx);
    let at = cx.debug_bounds("trigger").unwrap().center();
    cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    settle(cx);
    let bounds = cx.debug_bounds("test-menu").unwrap();
    let size = cx.update(|window, _| window.viewport_size());
    assert!(
        bounds.left() >= px(0.0) && bounds.right() <= size.width,
        "{bounds:?} {size:?}"
    );
    assert!(
        bounds.top() >= px(0.0) && bounds.bottom() <= size.height,
        "{bounds:?} {size:?}"
    );
    for _ in 0..40 {
        cx.simulate_keystrokes("down");
        settle(cx);
    }
    let last = cx.debug_bounds("last-item").unwrap();
    assert!(
        last.top() >= bounds.top() && last.bottom() <= bounds.bottom(),
        "{last:?} {bounds:?}"
    );
    cx.simulate_mouse_down(last.center(), MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_up(last.center(), MouseButton::Left, Modifiers::none());
    settle(cx);
    assert_eq!(host.read_with(cx, |host, _| host.chosen), Some(39));
    let trigger = host.read_with(cx, |host, _| host.trigger.clone());
    assert!(cx.update(|window, _| trigger.is_focused(window)));
}

#[gpui::test]
fn a_press_outside_a_popup_closes_it(cx: &mut TestAppContext) {
    let (host, cx) = popup(cx);
    cx.simulate_keystrokes("shift-f10");
    settle(cx);
    cx.simulate_mouse_down(
        point(px(2.0), px(2.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_up(
        point(px(2.0), px(2.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    settle(cx);
    assert!(!host.read_with(cx, |host, _| host.opened));
}

#[gpui::test]
fn the_keyboard_chooses_a_menu_action(cx: &mut TestAppContext) {
    let (host, cx) = popup(cx);
    cx.simulate_keystrokes("shift-f10");
    settle(cx);
    cx.simulate_keystrokes("down");
    settle(cx);
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").unwrap(),
    });
    settle(cx);
    assert_eq!(host.read_with(cx, |host, _| host.chosen), Some(0));
    assert!(!host.read_with(cx, |host, _| host.opened));
}

#[gpui::test]
fn a_tall_menu_reveals_its_profile_header_after_arrow_navigation(cx: &mut TestAppContext) {
    let (host, cx) = popup(cx);
    host.update(cx, |host, cx| {
        host.header = Some(cx.focus_handle().tab_stop(true))
    });
    cx.simulate_keystrokes("shift-f10");
    settle(cx);
    for _ in 0..40 {
        cx.simulate_keystrokes("down");
        settle(cx);
    }
    let menu = cx.debug_bounds("test-menu").unwrap();
    let last = cx.debug_bounds("last-item").unwrap();
    assert!(last.top() >= menu.top() && last.bottom() <= menu.bottom());
    let header = host.read_with(cx, |host, _| host.header.clone().unwrap());
    assert!(!cx.update(|window, _| header.is_focused(window)));
    cx.update(|window, cx| header.focus(window, cx));
    settle(cx);
    assert!(cx.update(|window, _| header.is_focused(window)));
    let shown = cx.debug_bounds("profile-header").unwrap();
    assert_eq!(shown.size.height, px(40.0));
    assert!(
        shown.top() >= menu.top() && shown.bottom() <= menu.bottom(),
        "{shown:?} in {menu:?}"
    );
}

#[gpui::test]
fn a_wheel_outside_a_menu_closes_it_but_its_own_list_keeps_scrolling(cx: &mut TestAppContext) {
    let (host, cx) = popup(cx);
    cx.simulate_keystrokes("menu");
    settle(cx);
    let bounds = cx.debug_bounds("test-menu").unwrap();
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: bounds.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
        ..Default::default()
    });
    settle(cx);
    assert!(host.read_with(cx, |host, _| host.opened));
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: point(px(2.0), px(2.0)),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
        ..Default::default()
    });
    settle(cx);
    assert!(!host.read_with(cx, |host, _| host.opened));
    let trigger = host.read_with(cx, |host, _| host.trigger.clone());
    assert!(cx.update(|window, _| trigger.is_focused(window)));
}

struct GrowingPopup {
    absolute: bool,
    focus: FocusHandle,
    placement: std::rc::Rc<super::MenuPlacement>,
    grown: bool,
}

impl Render for GrowingPopup {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let menu = Menu::new("growing", "Actions", &self.focus)
            .selector("growing")
            .children((0_usize..if self.grown { 30 } else { 3 }).map(|index| {
                MenuItem::new(("growing-action", index), format!("Action {index}")).selector(
                    if index == 0 {
                        "growing-first"
                    } else {
                        "growing-action"
                    },
                )
            }));
        let menu = if self.absolute {
            let size = window.viewport_size();
            menu.popup_at_stable(
                point(size.width - px(10.0), size.height - px(10.0)),
                self.placement.clone(),
            )
        } else {
            menu.popup_stable(point(px(10.0), px(10.0)), self.placement.clone())
        };
        div().relative().size_full().child(
            div()
                .absolute()
                .right_0()
                .bottom_0()
                .size(px(20.0))
                .child(menu),
        )
    }
}

#[gpui::test]
fn a_popup_that_grows_keeps_its_existing_actions_in_place(cx: &mut TestAppContext) {
    check_popup_growth(cx, false);
}

#[gpui::test]
fn a_popup_at_a_pointer_keeps_its_existing_actions_in_place(cx: &mut TestAppContext) {
    check_popup_growth(cx, true);
}

fn check_popup_growth(cx: &mut TestAppContext, absolute: bool) {
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| GrowingPopup {
        absolute,
        focus: cx.focus_handle(),
        placement: Default::default(),
        grown: false,
    });
    settle(cx);
    let first = cx.debug_bounds("growing-first").unwrap();
    host.update(cx, |host, cx| {
        host.grown = true;
        cx.notify();
    });
    settle(cx);
    assert_eq!(
        cx.debug_bounds("growing-first").unwrap().origin,
        first.origin
    );
    let bounds = cx.debug_bounds("growing").unwrap();
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!(bounds.top() >= px(0.0) && bounds.bottom() <= viewport.height);
    cx.simulate_resize(gpui::size(px(320.0), px(240.0)));
    settle(cx);
    let resized = cx.debug_bounds("growing").unwrap();
    assert!(resized.left() >= px(0.0) && resized.right() <= px(320.0));
    assert!(resized.top() >= px(0.0) && resized.bottom() <= px(240.0));
}

struct SliderMenu {
    menu: FocusHandle,
    slider: FocusHandle,
    state: Entity<gpui_base::slider::SliderState>,
}

impl Render for SliderMenu {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Menu::new("slider-menu", "Person actions", &self.menu)
            .child(div().flex().w_full().child(crate::ui::VolumeSlider::new(
                "slider-menu-volume",
                &self.state,
                &self.slider,
                "Volume for Rin",
            )))
            .child(MenuItem::new("slider-menu-copy", "Copy name"))
    }
}

#[gpui::test]
fn menu_arrows_reach_an_explicit_volume_focus_handle(cx: &mut TestAppContext) {
    use gpui::AppContext as _;
    cx.update(|cx| {
        gpui_base::init(cx);
        crate::theme::install(crate::theme::Theme::stone(), cx);
    });
    let (host, cx) = cx.add_window_view(|_, cx| SliderMenu {
        menu: cx.focus_handle(),
        slider: cx.focus_handle(),
        state: cx.new(|_| crate::ui::VolumeSlider::state(100)),
    });
    let (menu, slider, state) = host.read_with(cx, |host, _| {
        (host.menu.clone(), host.slider.clone(), host.state.clone())
    });
    cx.update(|window, cx| menu.focus(window, cx));
    settle(cx);
    cx.simulate_keystrokes("down");
    settle(cx);
    assert!(cx.update(|window, _| slider.is_focused(window)));
    cx.simulate_keystrokes("left");
    settle(cx);
    assert_eq!(state.read_with(cx, |state, _| state.value().end()), 90.0);
}
