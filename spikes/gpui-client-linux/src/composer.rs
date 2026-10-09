use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};
use gpui_component::{
    Sizable as _,
    button::Button,
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    v_flex,
};

use crate::support::{Palette, note, u};

pub struct ComposerGate {
    input: Entity<TextareaState>,
    sent: Vec<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl ComposerGate {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder("Message #general")
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { shift, secondary } => {
                    note(cx, format!("composer: PressEnter shift={shift} secondary={secondary}"));
                    if !*shift {
                        this.send(input.clone(), window, cx);
                    }
                }
                InputEvent::Change => cx.notify(),
                InputEvent::Focus => note(cx, "composer: focus"),
                InputEvent::Blur => note(cx, "composer: blur"),
            },
        );
        input.update(cx, |state, cx| state.focus(window, cx));
        Self {
            input,
            sent: Vec::new(),
            _subscriptions: vec![subscription],
        }
    }

    fn send(&mut self, input: Entity<TextareaState>, window: &mut Window, cx: &mut Context<Self>) {
        let value = input.read(cx).value();
        if value.trim().is_empty() {
            return;
        }
        note(cx, format!("composer: sent {:?}", value.as_ref()));
        self.sent.push(value);
        input.update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }
}

impl Render for ComposerGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.input.read(cx);
        let value = state.value();
        let selected = state.selected_range();
        let cursor = state.cursor();

        v_flex()
            .size_full()
            .p(u(16.0))
            .gap(u(10.0))
            .child(
                div()
                    .text_size(u(13.0))
                    .text_color(Palette::muted())
                    .child("Enter sends · Shift+Enter adds a line · grows to 6 rows, then scrolls"),
            )
            .child(
                v_flex()
                    .id("sent")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap(u(6.0))
                    .children(self.sent.iter().enumerate().map(|(index, text)| {
                        v_flex()
                            .p(u(8.0))
                            .rounded(u(6.0))
                            .bg(Palette::white())
                            .border_1()
                            .border_color(Palette::border())
                            .child(
                                div()
                                    .text_size(u(12.0))
                                    .text_color(Palette::muted())
                                    .child(format!("sent #{}", index + 1)),
                            )
                            .children(
                                text.split('\n')
                                    .map(|line| div().child(SharedString::from(line.to_string()))),
                            )
                    })),
            )
            .child(
                div()
                    .text_size(u(12.0))
                    .text_color(Palette::muted())
                    .font_family("Menlo")
                    .child(format!(
                        "value={:?} chars={} cursor={} selection={:?}",
                        value.as_ref(),
                        value.chars().count(),
                        cursor,
                        selected
                    )),
            )
            .child(
                h_flex()
                    .w(u(640.0))
                    .items_end()
                    .gap(u(8.0))
                    .child(div().flex_1().child(Textarea::new(&self.input)))
                    .child(
                        Button::new("emoji")
                            .small()
                            .label("Emoji picker")
                            .on_click(|_, window, _| window.show_character_palette()),
                    )
                    .child(Button::new("send").small().label("Send").on_click(cx.listener(
                        |this, _, window, cx| {
                            let input = this.input.clone();
                            this.send(input, window, cx)
                        },
                    ))),
            )
    }
}
