use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Role, StatefulInteractiveElement as _, Styled as _, Toggled, Window, div, text,
};
use gpui_component::{
    Sizable as _,
    button::Button,
    checkbox::Checkbox,
    h_flex,
    input::{Input, InputState},
    switch::Switch,
    v_flex,
};

use crate::{
    rich::message_view,
    support::{Palette, note, u},
};

pub struct AccessibilityGate {
    muted: bool,
    noise_filter: bool,
    notify_all: bool,
    address: Entity<InputState>,
}

impl AccessibilityGate {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            muted: false,
            noise_filter: true,
            notify_all: false,
            address: cx.new(|cx| InputState::new(window, cx).placeholder("host:port")),
        }
    }
}

impl Render for AccessibilityGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("a11y-region")
            .role(Role::Group)
            .aria_label("Call controls test region")
            .size_full()
            .p(u(16.0))
            .gap(u(12.0))
            .child(
                div()
                    .id("a11y-heading")
                    .role(Role::Heading)
                    .aria_level(1)
                    .aria_label("Call controls")
                    .text_size(u(22.0))
                    .child(text!("Call controls")),
            )
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(
                        Button::new("a11y-mute")
                            .small()
                            .label("🎙")
                            .accessibility_label("Mute microphone")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.muted = !this.muted;
                                note(cx, format!("accessibility: mute button pressed, muted={}", this.muted));
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("a11y-leave")
                            .role(Role::Button)
                            .aria_label("Leave call")
                            .focusable()
                            .tab_stop(true)
                            .px(u(10.0))
                            .py(u(4.0))
                            .rounded(u(6.0))
                            .bg(Palette::red())
                            .text_color(Palette::white())
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, _, cx| {
                                note(cx, "accessibility: leave button pressed");
                            }))
                            .child("Leave"),
                    )
                    .child(
                        div()
                            .id("a11y-switch")
                            .role(Role::Switch)
                            .aria_label("Noise filter")
                            .aria_toggled(if self.noise_filter {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .focusable()
                            .tab_stop(true)
                            .px(u(10.0))
                            .py(u(4.0))
                            .rounded(u(6.0))
                            .border_1()
                            .border_color(Palette::border())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.noise_filter = !this.noise_filter;
                                note(cx, format!("accessibility: noise filter {}", this.noise_filter));
                                cx.notify();
                            }))
                            .child(if self.noise_filter {
                                "Noise filter: on"
                            } else {
                                "Noise filter: off"
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap(u(12.0))
                    .child(
                        Switch::new("a11y-library-switch")
                            .checked(self.muted)
                            .label("Push to talk")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.muted = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        Checkbox::new("a11y-library-checkbox")
                            .checked(self.notify_all)
                            .label("Notify for all messages")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.notify_all = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .w(u(320.0))
                    .child(Input::new(&self.address).aria_label("Server address")),
            )
            .child(
                v_flex()
                    .id("a11y-people")
                    .role(Role::List)
                    .aria_label("People in the call")
                    .gap(u(4.0))
                    .children(["Ana", "Bruno", "Carla"].into_iter().enumerate().map(
                        |(index, name)| {
                            div()
                                .id(("a11y-person", index))
                                .role(Role::ListItem)
                                .aria_label(name)
                                .child(text!(id = ("a11y-person-text", index), name))
                        },
                    )),
            )
            .child(div().child("Plain string child without an id or a role"))
            .child(message_view(
                "a11y-message",
                "A **Markdown** message with a [link](https://example.org/docs) for the screen reader.",
            ))
    }
}
