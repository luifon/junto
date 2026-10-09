use gpui::{
    AppContext as _, Context, ExternalPaths, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use gpui_component::{
    Sizable as _,
    button::Button,
    h_flex,
    menu::DropdownMenu as _,
    v_flex,
};

use crate::support::{Palette, note, u};

#[derive(Clone, PartialEq)]
struct Person {
    name: SharedString,
}

struct DraggedPerson {
    name: SharedString,
    position: Point<Pixels>,
}

impl Render for DraggedPerson {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .pl(self.position.x - px(40.0))
            .pt(self.position.y - px(14.0))
            .child(
                div()
                    .px(u(10.0))
                    .py(u(4.0))
                    .rounded(u(6.0))
                    .bg(Palette::terracotta())
                    .text_color(Palette::white())
                    .shadow_md()
                    .child(self.name.clone()),
            )
    }
}

#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = dnd, no_json)]
struct MovePerson {
    name: SharedString,
    channel: usize,
}

struct Channel {
    name: SharedString,
    people: Vec<Person>,
}

pub struct DragDropGate {
    dropped_paths: Vec<SharedString>,
    channels: Vec<Channel>,
}

impl DragDropGate {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let people = |names: &[&str]| {
            names
                .iter()
                .map(|name| Person {
                    name: SharedString::from(name.to_string()),
                })
                .collect()
        };
        Self {
            dropped_paths: Vec::new(),
            channels: vec![
                Channel {
                    name: "Daily".into(),
                    people: people(&["Ana", "Bruno", "Carla", "Marta"]),
                },
                Channel {
                    name: "Focus room".into(),
                    people: people(&["Diego", "Elisa"]),
                },
                Channel {
                    name: "HR".into(),
                    people: Vec::new(),
                },
            ],
        }
    }

    fn move_person(&mut self, name: &SharedString, target: usize, how: &str, cx: &mut Context<Self>) {
        let mut moved = None;
        for channel in &mut self.channels {
            if let Some(index) = channel.people.iter().position(|person| &person.name == name) {
                moved = Some(channel.people.remove(index));
            }
        }
        if let Some(person) = moved {
            note(
                cx,
                format!(
                    "drag and drop: {} moved to {} by {how}",
                    person.name, self.channels[target].name
                ),
            );
            self.channels[target].people.push(person);
            cx.notify();
        }
    }
}

impl Render for DragDropGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let channel_names: Vec<SharedString> =
            self.channels.iter().map(|channel| channel.name.clone()).collect();

        h_flex()
            .id("dnd")
            .size_full()
            .items_start()
            .p(u(16.0))
            .gap(u(16.0))
            .on_action(cx.listener(|this, action: &MovePerson, _, cx| {
                this.move_person(&action.name.clone(), action.channel, "menu", cx)
            }))
            .child(
                v_flex()
                    .w(u(520.0))
                    .gap(u(8.0))
                    .child("(a) Drop files from Finder here")
                    .child(
                        v_flex()
                            .id("file-drop")
                            .h(u(260.0))
                            .p(u(12.0))
                            .rounded(u(8.0))
                            .border_2()
                            .border_dashed()
                            .border_color(Palette::border())
                            .bg(Palette::white())
                            .drag_over::<ExternalPaths>(|style, _, _, _| {
                                style.bg(Palette::ochre().opacity(0.25))
                            })
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                for path in paths.paths() {
                                    let shown = path.display().to_string();
                                    note(cx, format!("drag and drop: file received {shown}"));
                                    this.dropped_paths.push(shown.into());
                                }
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .text_color(Palette::muted())
                                    .child(format!("{} paths received", self.dropped_paths.len())),
                            )
                            .children(self.dropped_paths.iter().map(|path| {
                                div().text_size(u(13.0)).child(path.clone())
                            })),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap(u(8.0))
                    .child("(b) Drag a person to another voice channel, or use the Move menu")
                    .child(h_flex().items_start().gap(u(12.0)).children(
                        self.channels.iter().enumerate().map(|(channel_index, channel)| {
                            let names = channel_names.clone();
                            v_flex()
                                .id(("channel", channel_index))
                                .w(u(220.0))
                                .min_h(u(220.0))
                                .p(u(10.0))
                                .gap(u(6.0))
                                .rounded(u(8.0))
                                .bg(Palette::panel())
                                .border_1()
                                .border_color(Palette::border())
                                .drag_over::<Person>(|style, _, _, _| {
                                    style.bg(Palette::green().opacity(0.25))
                                })
                                .on_drop(cx.listener(move |this, person: &Person, _, cx| {
                                    this.move_person(&person.name.clone(), channel_index, "drag", cx)
                                }))
                                .child(format!("🔊 {} ({})", channel.name, channel.people.len()))
                                .children(channel.people.iter().map(|person| {
                                    let name = person.name.clone();
                                    let names = names.clone();
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            div()
                                                .id(SharedString::from(format!("person-{name}")))
                                                .px(u(8.0))
                                                .py(u(4.0))
                                                .rounded(u(6.0))
                                                .bg(Palette::white())
                                                .cursor_move()
                                                .child(name.clone())
                                                .on_drag(person.clone(), |person, position, _, cx| {
                                                    let name = person.name.clone();
                                                    cx.new(|_| DraggedPerson { name, position })
                                                }),
                                        )
                                        .child(
                                            Button::new(SharedString::from(format!("move-{name}")))
                                                .xsmall()
                                                .label("Move")
                                                .dropdown_menu(move |mut menu, _, _| {
                                                    for (index, channel_name) in names.iter().enumerate() {
                                                        if index != channel_index {
                                                            menu = menu.menu(
                                                                format!("Move to {channel_name}"),
                                                                Box::new(MovePerson {
                                                                    name: name.clone(),
                                                                    channel: index,
                                                                }),
                                                            );
                                                        }
                                                    }
                                                    menu
                                                }),
                                        )
                                }))
                        }),
                    )),
            )
    }
}
