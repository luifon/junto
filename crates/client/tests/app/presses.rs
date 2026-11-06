//! A press whose control moves away from the pointer between the press and the release,
//! because the window draws again with content that changed (ADR 0757). The view of the test
//! uses gpui alone: a spacer and a second button above a button, a cover over it, and a
//! clipping frame around it, which the test changes between the press and the release.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    InteractiveElement as _, IntoElement as _, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    VisualTestContext, div, point, px,
};

use crate::harness::{open_view, view_input, view_redraw};

#[derive(Default)]
struct Scene {
    spacer: Cell<f32>,
    /// A second button above the first one.
    above: Cell<bool>,
    /// An element over the whole window that blocks the pointer, as a dialog cover does.
    cover: Cell<bool>,
    /// The height of the frame that clips the button, when it clips it.
    clip: Cell<Option<f32>>,
    /// The buttons lie on a layer over the whole window that blocks the pointer, as the
    /// content of a dialog does.
    layer: Cell<bool>,
    clicks: Cell<u32>,
    above_clicks: Cell<u32>,
}

struct Moving(Rc<Scene>);

impl gpui::Render for Moving {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let scene = self.0.clone();
        let clicked = scene.clone();
        let button = div()
            .id("moving-button")
            .flex_none()
            .w(px(100.0))
            .h(px(30.0))
            .on_click(move |_, _, _| clicked.clicks.set(clicked.clicks.get() + 1));
        let button = match scene.clip.get() {
            Some(height) => div()
                .flex_none()
                .h(px(height))
                .overflow_hidden()
                .child(button)
                .into_any_element(),
            None => button.into_any_element(),
        };
        let above = scene.above.get().then(|| {
            let clicked = scene.clone();
            div()
                .id("button-above")
                .flex_none()
                .w(px(100.0))
                .h(px(30.0))
                .on_click(move |_, _, _| clicked.above_clicks.set(clicked.above_clicks.get() + 1))
        });
        let column = div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_none().h(px(scene.spacer.get())))
            .children(above)
            .child(button);
        let content = if scene.layer.get() {
            div()
                .id("layer")
                .absolute()
                .inset_0()
                .occlude()
                .child(column)
                .into_any_element()
        } else {
            column.into_any_element()
        };
        div().size_full().relative().child(content).children(
            scene
                .cover
                .get()
                .then(|| div().id("cover").absolute().inset_0().occlude()),
        )
    }
}

fn moving(cx: &mut TestAppContext) -> (VisualTestContext, Rc<Scene>) {
    let scene = Rc::new(Scene::default());
    let view = Moving(scene.clone());
    (open_view(cx, move |_, _| view), scene)
}

fn down(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    view_input(
        cx,
        MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers: gpui::Modifiers::none(),
        },
    );
    view_input(
        cx,
        MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: gpui::Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        },
    );
}

fn up(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    view_input(
        cx,
        MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: gpui::Modifiers::none(),
            click_count: 1,
        },
    );
}

#[gpui::test]
fn a_press_is_delivered_when_a_redraw_moves_its_control_before_the_release(
    cx: &mut TestAppContext,
) {
    let (mut cx, scene) = moving(cx);
    let at = point(px(50.0), px(10.0));
    down(&mut cx, at);
    scene.spacer.set(20.0);
    view_redraw(&mut cx);
    up(&mut cx, at);
    assert_eq!(
        scene.clicks.get(),
        1,
        "the press ends on the control that it started on"
    );

    // A press that is released outside both the place where the control was at the press
    // and the place where it is now is no click.
    down(&mut cx, point(px(50.0), px(30.0)));
    scene.spacer.set(60.0);
    view_redraw(&mut cx);
    up(&mut cx, point(px(50.0), px(100.0)));
    assert_eq!(scene.clicks.get(), 1);

    // A press that ends where it started, on a control that did not move, is one click.
    let on = point(px(50.0), px(75.0));
    down(&mut cx, on);
    up(&mut cx, on);
    assert_eq!(scene.clicks.get(), 2);
}

#[gpui::test]
fn a_control_that_another_control_replaced_under_the_release_takes_no_click(
    cx: &mut TestAppContext,
) {
    let (mut cx, scene) = moving(cx);
    let at = point(px(50.0), px(15.0));
    down(&mut cx, at);
    // A second button comes above the pressed one: it lies under the release point now, and
    // the pressed button is under it.
    scene.above.set(true);
    view_redraw(&mut cx);
    up(&mut cx, at);
    assert_eq!(scene.clicks.get(), 0, "the moved button took the release");
    assert_eq!(
        scene.above_clicks.get(),
        0,
        "the button under the release held no press"
    );
}

#[gpui::test]
fn a_control_that_a_cover_hides_before_the_release_takes_no_click(cx: &mut TestAppContext) {
    let (mut cx, scene) = moving(cx);
    let at = point(px(50.0), px(15.0));
    down(&mut cx, at);
    scene.cover.set(true);
    view_redraw(&mut cx);
    up(&mut cx, at);
    assert_eq!(
        scene.clicks.get(),
        0,
        "the control under the cover took the release"
    );

    // The same press without the cover is a click: the cover alone refused it.
    scene.cover.set(false);
    view_redraw(&mut cx);
    down(&mut cx, at);
    up(&mut cx, at);
    assert_eq!(scene.clicks.get(), 1);
}

#[gpui::test]
fn a_second_cover_of_the_window_over_a_pressed_control_on_a_cover_takes_no_click(
    cx: &mut TestAppContext,
) {
    let (mut cx, scene) = moving(cx);
    scene.layer.set(true);
    view_redraw(&mut cx);
    let at = point(px(50.0), px(15.0));
    let refused = gpui::refused_presses();
    down(&mut cx, at);
    // A second layer of the same bounds and behaviour as the one that holds the button.
    scene.cover.set(true);
    view_redraw(&mut cx);
    up(&mut cx, at);
    assert_eq!(
        scene.clicks.get(),
        0,
        "the release passed through the second cover"
    );
    assert_eq!(
        gpui::refused_presses(),
        refused + 1,
        "the refused press is counted for the window tests"
    );

    // Without the second cover the same press is a click, and no press is refused.
    scene.cover.set(false);
    view_redraw(&mut cx);
    let refused = gpui::refused_presses();
    down(&mut cx, at);
    up(&mut cx, at);
    assert_eq!(scene.clicks.get(), 1);
    assert_eq!(gpui::refused_presses(), refused);
}

#[gpui::test]
fn a_release_on_the_clipped_part_of_a_control_is_no_click(cx: &mut TestAppContext) {
    let (mut cx, scene) = moving(cx);
    // The frame shows the upper 20 of the 30 pixels of the button.
    scene.clip.set(Some(20.0));
    view_redraw(&mut cx);
    down(&mut cx, point(px(50.0), px(10.0)));
    up(&mut cx, point(px(50.0), px(25.0)));
    assert_eq!(
        scene.clicks.get(),
        0,
        "the clipped part of the button took the release"
    );

    // The button moves down while it is pressed; the release on its old place, inside the
    // visible part, is a click.
    down(&mut cx, point(px(50.0), px(10.0)));
    scene.spacer.set(40.0);
    view_redraw(&mut cx);
    up(&mut cx, point(px(50.0), px(10.0)));
    assert_eq!(scene.clicks.get(), 1);
}

/// A press of the harness waits for the requests that it started, also while a proxy of the
/// test holds a request from before the press (ADR 0902): the held read of a quote does not
/// end the wait for the read of the devices that "Try again" starts, which one `busy` answer
/// makes last a second.
#[gpui::test]
fn a_press_waits_for_its_own_request_while_a_proxy_holds_an_earlier_one(cx: &mut TestAppContext) {
    use crate::harness::{World, member};
    use crate::messages::document;
    use crate::support::{Fault, FaultProxy};
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::SendMessage;
    const DEVICES: &str = "/api/v1/account/devices";
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let channel = world.run(api.snapshot()).unwrap().snapshot.workspaces[0]
        .channels
        .iter()
        .find(|channel| channel.kind == junto_client_core::protocol::structure::ChannelKind::Text)
        .unwrap()
        .id;
    // The original is older than the page that the window reads first, so the window reads
    // it for the reply.
    let original = admin.send(&world, channel, document("the held original"));
    for index in 0..60 {
        admin.send(&world, channel, document(&format!("later {index}")));
    }
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until_message("later 59");
    proxy.hold_ending(&format!("/messages/{}", original.id));
    world
        .run(api.send_message(
            PlaceId::Channel(channel),
            &SendMessage {
                document: document("the answer"),
                reply_to: Some(original.id),
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("the read of the quote waits at the proxy", |_| {
        !crate::support::held_by_proxies().is_empty()
    });
    // The first read of the devices fails; "Try again" reads them with one `busy` answer.
    let attempts = junto_client_core::api::RetryPolicy::default().attempts as usize;
    proxy.fail(DEVICES, std::iter::repeat_n(Fault::Busy, attempts));
    window.open_account();
    window.click("account-nav-devices");
    window.until("the failure of the read is shown", |window| {
        window.shows("devices-read-failed")
    });
    proxy.fail(DEVICES, [Fault::Busy]);
    window.click("devices-read-again");
    assert_eq!(
        window.devices().len(),
        1,
        "the press returned before its read of the devices was answered"
    );
    proxy.release();
}

/// A press that opens a channel waits for the read of its first page, which the stream task
/// of the client core starts after a command from the window (ADR 0902): one `busy` answer
/// makes that read last a second.
#[gpui::test]
fn a_press_waits_for_the_page_that_the_stream_task_reads_for_it(cx: &mut TestAppContext) {
    use crate::harness::{World, member};
    use crate::messages::document;
    use crate::support::{Fault, FaultProxy};
    use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let admin = world.owner(&server);
    let api = admin.handle.api();
    let workspace = world.run(api.snapshot()).unwrap().snapshot.workspaces[0]
        .workspace
        .id;
    world
        .run(api.create_channel(
            workspace,
            &CreateChannel {
                topic: String::new(),
                name: "elsewhere".to_owned(),
                kind: ChannelKind::Text,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    let elsewhere = world.run(api.snapshot()).unwrap().snapshot.workspaces[0]
        .channels
        .iter()
        .find(|channel| channel.name == "elsewhere")
        .unwrap()
        .id;
    admin.send(&world, elsewhere, document("the slowed message"));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.until("the other channel is listed", |window| {
        window.channel_row("elsewhere").is_some()
    });
    proxy.fail_ending(&format!("/channels/{elsewhere}/messages"), [Fault::Busy]);
    window.click_channel("elsewhere");
    assert!(
        window
            .texts()
            .iter()
            .any(|text| text == "the slowed message"),
        "the press returned before the stream task read the page of the channel"
    );
}
