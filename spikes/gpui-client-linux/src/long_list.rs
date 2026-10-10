use std::{cell::RefCell, rc::Rc, time::Duration, time::Instant};

use gpui::{
    AppContext as _, Context, Entity, FollowMode, IntoElement,
    ListAlignment, ListState, ParentElement as _, Render, SharedString, Styled as _, Task, Window,
    div, list, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    message_scroller::{MessageScroller, MessageScrollerState},
    v_flex,
};

use crate::{
    rich::message_view,
    support::{Palette, note, out_dir, process_cpu_seconds, tabular_numerals, u},
};

pub const MESSAGE_COUNT: usize = 20_000;

#[derive(Clone)]
struct Message {
    author: SharedString,
    time: SharedString,
    body: SharedString,
    color: gpui::Hsla,
}

fn hash(index: usize) -> usize {
    let mut value = index as u64 ^ 0x9e37_79b9_7f4a_7c15;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 29;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    (value ^ (value >> 32)) as usize
}

fn generate(index: usize) -> Message {
    const AUTHORS: [&str; 6] = ["Ana", "Bruno", "Carla", "Diego", "Elisa", "Marta"];
    const SENTENCES: [&str; 8] = [
        "The **v0.9** checklist is up, please tick your items.",
        "Ran it on the 40-person room: the mixdown went from 96 s to 11 s.",
        "Should recordings keep one track per speaker, or only the mixdown?",
        "Reviewed. The default is `30 days`, with a per-channel override.",
        "I am writing it up now, and I will fork this thread if it grows.",
        "Staging is back up. Deploy *v0.9.0-rc2* went out at 09:31.",
        "Two more minutes on the screen share, then we close 🎉",
        "See [the retention spec](https://example.org/spec) before lunch.",
    ];
    let seed = hash(index);
    let sentences = 1 + seed % 7;
    let mut body = String::new();
    for part in 0..sentences {
        body.push_str(SENTENCES[(seed / 7 + part * 3) % SENTENCES.len()]);
        body.push(' ');
    }
    if index % 13 == 0 {
        body.push_str("\n\n```rust\nfn mix_tracks(tracks: &[Track]) -> Mixdown {\n    tracks.iter().filter(|t| !t.muted).collect()\n}\n```");
    }
    if index % 17 == 0 {
        body.push_str("\n\n- first point\n- second point\n- third point");
    }
    let colors = [Palette::terracotta(), Palette::green(), Palette::ochre()];
    Message {
        author: AUTHORS[seed % AUTHORS.len()].into(),
        time: format!("{:02}:{:02}", (index / 60) % 24, index % 60).into(),
        body: format!("#{index} {body}").into(),
        color: colors[seed % colors.len()],
    }
}

fn row(message: &Message, index: usize) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .items_start()
        .gap(u(10.0))
        .px(u(16.0))
        .py(u(6.0))
        .child(
            div()
                .flex_none()
                .size(u(36.0))
                .rounded(u(6.0))
                .bg(message.color)
                .text_color(Palette::white())
                .flex()
                .items_center()
                .justify_center()
                .child(SharedString::from(
                    message.author.chars().next().unwrap_or('?').to_string(),
                )),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .gap(u(8.0))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(message.author.clone()),
                        )
                        .child(
                            div()
                                .text_size(u(12.0))
                                .text_color(Palette::muted())
                                .font_features(tabular_numerals())
                                .child(message.time.clone()),
                        ),
                )
                .child(message_view(("message", index), message.body.clone())),
        )
}

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    GpuiList,
    MessageScroller,
}

struct Phase {
    name: &'static str,
    frames: usize,
    pixels_per_frame: f32,
}

const PHASES: [Phase; 3] = [
    Phase {
        name: "up 30 px per frame",
        frames: 600,
        pixels_per_frame: -30.0,
    },
    Phase {
        name: "up 400 px per frame",
        frames: 600,
        pixels_per_frame: -400.0,
    },
    Phase {
        name: "down 400 px per frame",
        frames: 600,
        pixels_per_frame: 400.0,
    },
];

struct Benchmark {
    phase: usize,
    frame: usize,
    last_frame: Instant,
    intervals_ms: Vec<f64>,
    report: String,
    before: gpui::profiler::FrameDurationSnapshot,
    cpu_start: f64,
    wall_start: Instant,
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = ((sorted.len() - 1) as f64 * quantile).round() as usize;
    sorted[position]
}

pub struct LongListGate {
    variant: Variant,
    messages: Rc<RefCell<Vec<Message>>>,
    list: ListState,
    scroller: Entity<MessageScrollerState>,
    auto_add: Option<Task<()>>,
    benchmark: Option<Benchmark>,
    result: SharedString,
}

impl LongListGate {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let started = Instant::now();
        let messages: Vec<Message> = (0..MESSAGE_COUNT).map(generate).collect();
        note(
            cx,
            format!(
                "long list: generated {} messages in {:.1} ms",
                messages.len(),
                started.elapsed().as_secs_f64() * 1000.0
            ),
        );
        let list = ListState::new(messages.len(), ListAlignment::Top, px(400.0));
        list.set_follow_mode(FollowMode::Tail);
        let this = cx.weak_entity();
        list.set_scroll_handler(move |_, _, cx| {
            let this = this.clone();
            cx.defer(move |cx| {
                this.update(cx, |_, cx| cx.notify()).ok();
            });
        });
        let scroller = cx.new(|cx| MessageScrollerState::new(MESSAGE_COUNT, cx));
        Self {
            variant: Variant::GpuiList,
            messages: Rc::new(RefCell::new(messages)),
            list,
            scroller,
            auto_add: None,
            benchmark: None,
            result: "no benchmark run yet".into(),
        }
    }

    fn add_message(&mut self, cx: &mut Context<Self>) {
        let index = self.messages.borrow().len();
        self.messages.borrow_mut().push(generate(index));
        self.list.splice(index..index, 1);
        self.scroller.update(cx, |state, cx| {
            state.append(1, cx);
        });
        cx.notify();
    }

    fn toggle_auto_add(&mut self, cx: &mut Context<Self>) {
        if self.auto_add.take().is_some() {
            note(cx, "long list: automatic messages stopped");
            cx.notify();
            return;
        }
        note(cx, "long list: one new message every 250 ms");
        self.auto_add = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if this.update(cx, |this, cx| this.add_message(cx)).is_err() {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        self.scroller.update(cx, |state, cx| state.scroll_to_end(cx));
        note(cx, "long list: jump to latest");
        cx.notify();
    }

    fn start_benchmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.variant = Variant::GpuiList;
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        self.benchmark = Some(Benchmark {
            phase: 0,
            frame: 0,
            last_frame: Instant::now(),
            intervals_ms: Vec::new(),
            report: String::new(),
            before: window.frame_duration_snapshot(),
            cpu_start: process_cpu_seconds(),
            wall_start: Instant::now(),
        });
        note(cx, "long list: scroll benchmark started");
        self.schedule_frame(window, cx);
    }

    fn schedule_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            this.update(cx, |this, cx| this.benchmark_frame(window, cx)).ok();
        });
    }

    fn benchmark_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(benchmark) = self.benchmark.as_mut() else {
            return;
        };
        let now = Instant::now();
        if benchmark.frame > 0 {
            benchmark
                .intervals_ms
                .push(now.duration_since(benchmark.last_frame).as_secs_f64() * 1000.0);
        }
        benchmark.last_frame = now;
        benchmark.frame += 1;

        let phase = &PHASES[benchmark.phase];
        if benchmark.frame > phase.frames {
            let mut sorted = benchmark.intervals_ms.clone();
            sorted.sort_by(|a, b| a.total_cmp(b));
            let mean = sorted.iter().sum::<f64>() / sorted.len().max(1) as f64;
            let slow = sorted.iter().filter(|value| **value > 12.5).count();
            let very_slow = sorted.iter().filter(|value| **value > 25.0).count();
            benchmark.report.push_str(&format!(
                "{}: frames={} mean={:.2} ms p50={:.2} p95={:.2} p99={:.2} max={:.2} over12.5ms={} over25ms={}\n",
                phase.name,
                sorted.len(),
                mean,
                percentile(&sorted, 0.50),
                percentile(&sorted, 0.95),
                percentile(&sorted, 0.99),
                sorted.last().copied().unwrap_or(0.0),
                slow,
                very_slow
            ));
            benchmark.intervals_ms.clear();
            benchmark.frame = 0;
            benchmark.phase += 1;
            if benchmark.phase >= PHASES.len() {
                self.finish_benchmark(window, cx);
                return;
            }
        } else {
            self.list.scroll_by(px(phase.pixels_per_frame));
        }
        cx.notify();
        self.schedule_frame(window, cx);
    }

    fn finish_benchmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(benchmark) = self.benchmark.take() else {
            return;
        };
        let after = window.frame_duration_snapshot();
        let mut draw = after.draw_duration_histogram.clone();
        draw.subtract(&benchmark.before.draw_duration_histogram).ok();
        let mut present = after.present_interval_histogram.clone();
        present
            .subtract(&benchmark.before.present_interval_histogram)
            .ok();
        let milliseconds = |nanoseconds: u64| nanoseconds as f64 / 1e6;
        let wall = benchmark.wall_start.elapsed().as_secs_f64();
        let cpu = process_cpu_seconds() - benchmark.cpu_start;
        let report = format!(
            "{}gpui Window::draw duration: samples={} p50={:.2} ms p95={:.2} p99={:.2} max={:.2}\ngpui present interval: samples={} p50={:.2} ms p95={:.2} p99={:.2} max={:.2}\nprocess CPU during benchmark: {:.0} % of one core over {:.1} s\n",
            benchmark.report,
            draw.len(),
            milliseconds(draw.value_at_quantile(0.50)),
            milliseconds(draw.value_at_quantile(0.95)),
            milliseconds(draw.value_at_quantile(0.99)),
            milliseconds(draw.max()),
            present.len(),
            milliseconds(present.value_at_quantile(0.50)),
            milliseconds(present.value_at_quantile(0.95)),
            milliseconds(present.value_at_quantile(0.99)),
            milliseconds(present.max()),
            cpu / wall * 100.0,
            wall
        );
        std::fs::write(out_dir().join("c4-bench.txt"), &report).ok();
        for line in report.lines() {
            note(cx, format!("long list bench: {line}"));
        }
        self.result = report.into();
        cx.notify();
    }
}

impl Render for LongListGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.messages.borrow().len();
        let top = self.list.logical_scroll_top();
        let status = match self.variant {
            Variant::GpuiList => format!(
                "gpui list · {} messages · top item {} · following tail {} · at end {:?}",
                count,
                top.item_ix,
                self.list.is_following_tail(),
                self.list.is_scrolled_to_end()
            ),
            Variant::MessageScroller => {
                let state = self.scroller.read(cx);
                format!(
                    "MessageScroller · {} messages · following tail {} · scrolled up {}",
                    state.item_count(),
                    state.is_following_tail(),
                    state.is_scrolled_up()
                )
            }
        };

        let messages = self.messages.clone();
        let content = match self.variant {
            Variant::GpuiList => list(self.list.clone(), move |index, _, _| {
                row(&messages.borrow()[index], index).into_any_element()
            })
            .size_full()
            .into_any_element(),
            Variant::MessageScroller => {
                MessageScroller::new("library-scroller", self.scroller.clone(), move |index, _, _| {
                    row(&messages.borrow()[index], index)
                })
                .size_full()
                .into_any_element()
            }
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .flex_wrap()
                    .gap(u(6.0))
                    .p(u(8.0))
                    .border_b_1()
                    .border_color(Palette::border())
                    .child(
                        Button::new("variant-list")
                            .small()
                            .label("gpui list")
                            .when(self.variant == Variant::GpuiList, |button| button.primary())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.variant = Variant::GpuiList;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("variant-scroller")
                            .small()
                            .label("MessageScroller")
                            .when(self.variant == Variant::MessageScroller, |button| {
                                button.primary()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.variant = Variant::MessageScroller;
                                cx.notify();
                            })),
                    )
                    .child(Button::new("add").small().label("Add message").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.add_message(cx);
                            note(cx, "long list: one message added");
                        }),
                    ))
                    .child(
                        Button::new("auto")
                            .small()
                            .label(if self.auto_add.is_some() {
                                "Stop automatic messages"
                            } else {
                                "Start automatic messages"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_auto_add(cx))),
                    )
                    .child(
                        Button::new("jump")
                            .small()
                            .label("Jump to latest")
                            .on_click(cx.listener(|this, _, _, cx| this.jump_to_latest(cx))),
                    )
                    .child(
                        Button::new("bench")
                            .small()
                            .label("Run scroll benchmark")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_benchmark(window, cx)
                            })),
                    )
                    .child(Button::new("overlay").small().label("Frame overlay").on_click(
                        |_, window, _| window.cycle_debug_frame_overlay_mode(),
                    ))
                    .child(
                        div()
                            .text_size(u(12.0))
                            .font_features(tabular_numerals())
                            .text_color(Palette::muted())
                            .child(status),
                    ),
            )
            .child(div().flex_1().min_h_0().w_full().child(content))
            .child(
                div()
                    .flex_none()
                    .p(u(8.0))
                    .text_size(u(11.0))
                    .font_family("Menlo")
                    .border_t_1()
                    .border_color(Palette::border())
                    .children(
                        self.result
                            .lines()
                            .map(|line| div().child(SharedString::from(line.to_string()))),
                    ),
            )
    }
}
