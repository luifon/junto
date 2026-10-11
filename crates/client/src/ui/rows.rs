//! The rows of the two lists at the left: workspaces in the server book, and channels with
//! the people in voice channels in the channel column.

use gpui::{
    AnyElement, App, ClickEvent, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder,
};

use super::{
    Meter, Mosaic, MosaicKind, RecordingMark, StyleExt, focus_ring_inside, icon, selection_mark,
    tooltip,
};
use crate::assets::Icon;
use crate::theme::{theme, u};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// One workspace of a server, in the server book or in the menu of a collapsed server.
#[derive(IntoElement)]
pub struct WorkspaceRow {
    id: ElementId,
    name: SharedString,
    seed: u128,
    selected: bool,
    in_menu: bool,
    selector: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl WorkspaceRow {
    pub fn new(id: impl Into<ElementId>, name: impl Into<SharedString>, seed: u128) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            seed,
            selected: false,
            in_menu: false,
            selector: None,
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The row of the menu of a collapsed server: without the tree line.
    pub fn in_menu(mut self) -> Self {
        self.in_menu = true;
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for WorkspaceRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring_inside(colors.ochre, theme.metrics.focus_ring);
        let style = if self.selected {
            theme.types.control_strong
        } else {
            theme.types.control
        };
        div()
            .id(self.id)
            .role(Role::ListItem)
            .aria_label(self.name.clone())
            .aria_selected(self.selected)
            .when_some(self.selector, |row, selector| {
                row.debug_selector(|| selector.to_owned())
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .h(u(theme.metrics.workspace_row_height))
            .pl(u(if self.in_menu { 12.0 } else { 38.0 }))
            .pr(u(10.0))
            .typeface(style, &theme)
            .ink(if self.selected {
                colors.bone_hi
            } else {
                colors.bone_2
            })
            .when(self.selected, |row| {
                row.bg(Hsla::from(colors.basalt_3))
                    .child(selection_mark(cx))
            })
            .when(!self.in_menu, |row| {
                // The branch of the tree line that joins the row to its server.
                row.child(
                    div()
                        .absolute()
                        .left(u(24.0))
                        .top(u(13.0))
                        .w(u(9.0))
                        .h(gpui::px(1.0))
                        .bg(Hsla::from(colors.basalt_rule)),
                )
            })
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .hover(move |style| style.bg(Hsla::from(colors.basalt_3)))
            .focus_visible(move |style| style.shadow(ring))
            .when_some(self.on_click, |row, handler| {
                row.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .child(Mosaic::new(MosaicKind::Small, self.seed, 15.0).radius(2.0))
            .child(div().min_w_0().truncate().child(self.name))
    }
}

/// The label above a group of channels.
#[derive(IntoElement)]
pub struct CategoryHeader {
    name: SharedString,
    first: bool,
}

impl CategoryHeader {
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            first: false,
        }
    }

    /// The first header of a list has less space above it.
    pub fn first(mut self, first: bool) -> Self {
        self.first = first;
        self
    }
}

impl RenderOnce for CategoryHeader {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(7.0))
            .pt(u(if self.first { 10.0 } else { 16.0 }))
            .pb(u(4.0))
            .pl(u(14.0))
            .pr(u(10.0))
            .typeface(theme.types.label, &theme)
            .ink(theme.colors.bronze)
            .child(
                div()
                    .flex_none()
                    .max_w(u(170.0))
                    .truncate()
                    .child(self.name),
            )
            .child(
                div()
                    .flex_1()
                    .h(gpui::px(1.0))
                    .bg(Hsla::from(theme.colors.rule)),
            )
    }
}

/// A state of a voice channel that its row shows at the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelState {
    /// The call of the channel is recorded.
    Recording(SharedString),
    /// An icon and a short text: "locked", "in use", "8 / 8 · full".
    Marked {
        icon: Icon,
        text: SharedString,
        strong: bool,
    },
}

/// One channel of the channel column.
#[derive(IntoElement)]
pub struct ChannelRow {
    id: ElementId,
    icon: Icon,
    name: SharedString,
    selected: bool,
    states: Vec<ChannelState>,
    selector: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl ChannelRow {
    pub fn new(id: impl Into<ElementId>, icon: Icon, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            name: name.into(),
            selected: false,
            states: Vec::new(),
            selector: None,
            on_click: None,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn states(mut self, states: Vec<ChannelState>) -> Self {
        self.states = states;
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for ChannelRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        let style = if self.selected {
            theme.types.row_selected
        } else {
            theme.types.row
        };
        let states: Vec<AnyElement> = self
            .states
            .into_iter()
            .map(|state| match state {
                ChannelState::Recording(text) => RecordingMark::new(text).into_any_element(),
                ChannelState::Marked {
                    icon: shown,
                    text,
                    strong,
                } => div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(5.0))
                    .typeface(
                        if strong {
                            theme.types.small_strong
                        } else {
                            theme.types.tally
                        },
                        &theme,
                    )
                    .ink(if strong { colors.ink } else { colors.ink_2 })
                    .child(icon(shown, 13.0, colors.bronze))
                    .child(text)
                    .into_any_element(),
            })
            .collect();
        div()
            .id(self.id)
            .role(Role::ListItem)
            .aria_label(self.name.clone())
            .aria_selected(self.selected)
            .when_some(self.selector, |row, selector| {
                row.debug_selector(|| selector.to_owned())
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .h(u(theme.metrics.channel_row_height))
            .pl(u(14.0))
            .pr(u(10.0))
            .typeface(style, &theme)
            .ink(if self.selected {
                colors.ink
            } else {
                colors.ink_2
            })
            .when(self.selected, |row| {
                row.bg(Hsla::from(colors.limestone_lo))
                    .child(selection_mark(cx))
            })
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .when(!self.selected, |row| {
                row.hover(move |style| style.bg(Hsla::from(colors.limestone_hover)))
            })
            .focus_visible(move |style| style.shadow(ring))
            .when_some(self.on_click, |row, handler| {
                row.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .child(icon(
                self.icon,
                15.0,
                if self.selected {
                    colors.terracotta_deep
                } else {
                    colors.ink_3
                },
            ))
            .child(div().flex_1().min_w_0().truncate().child(self.name))
            .when(!states.is_empty(), |row| {
                row.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(8.0))
                        .children(states),
                )
            })
    }
}

/// A mark at the right of a person in a voice channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OccupantMark {
    /// The person closed the microphone and can open it.
    MicrophoneOff(SharedString),
    /// A moderator or meeting mode closed the microphone. The person cannot open it.
    MicrophoneHeld(SharedString),
    Sharing(SharedString),
    HandRaised(SharedString),
    SoundOff(SharedString),
    /// The level of an open microphone, as lit segments of five.
    Level {
        lit: usize,
        label: SharedString,
    },
    /// A state in words, such as "reconnecting…".
    Status(SharedString),
}

impl OccupantMark {
    pub fn label(&self) -> &SharedString {
        match self {
            Self::MicrophoneOff(label)
            | Self::MicrophoneHeld(label)
            | Self::Sharing(label)
            | Self::HandRaised(label)
            | Self::SoundOff(label)
            | Self::Status(label)
            | Self::Level { label, .. } => label,
        }
    }
}

/// One person in a voice channel.
#[derive(IntoElement)]
pub struct OccupantRow {
    id: ElementId,
    name: SharedString,
    seed: u128,
    marks: Vec<OccupantMark>,
    selector: Option<SharedString>,
    /// The menu of the person is open.
    open: bool,
    on_menu: Option<MenuHandler>,
}

type MenuHandler = std::rc::Rc<dyn Fn(&mut Window, &mut App) + 'static>;

impl OccupantRow {
    pub fn new(id: impl Into<ElementId>, name: impl Into<SharedString>, seed: u128) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            seed,
            marks: Vec::new(),
            selector: None,
            open: false,
            on_menu: None,
        }
    }

    pub fn marks(mut self, marks: Vec<OccupantMark>) -> Self {
        self.marks = marks;
        self
    }

    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    /// The menu of the person is open: the row is drawn as the one that the menu is about.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// The row opens a menu about its person: with a right click, and from the keyboard
    /// with the Menu key, Shift and F10, Enter or Space. The row is a tab stop then.
    pub fn on_menu(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_menu = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for OccupantRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let mut label = self.name.to_string();
        for mark in &self.marks {
            label.push_str(", ");
            label.push_str(mark.label());
        }
        let marks = self.marks.into_iter().enumerate().map(|(index, mark)| {
            let label = mark.label().clone();
            let drawn = match mark {
                OccupantMark::MicrophoneOff(_) => {
                    icon(Icon::MicrophoneOff, 14.0, colors.terracotta_deep).into_any_element()
                }
                OccupantMark::MicrophoneHeld(_) => {
                    icon(Icon::MicrophoneHeld, 14.0, colors.terracotta_deep).into_any_element()
                }
                OccupantMark::Sharing(_) => {
                    icon(Icon::Screen, 14.0, colors.bronze).into_any_element()
                }
                OccupantMark::HandRaised(_) => {
                    icon(Icon::Hand, 14.0, colors.bronze).into_any_element()
                }
                OccupantMark::SoundOff(_) => {
                    icon(Icon::PhonesOff, 14.0, colors.terracotta_deep).into_any_element()
                }
                OccupantMark::Level { lit, .. } => Meter::new(lit).into_any_element(),
                OccupantMark::Status(text) => div()
                    .typeface(theme.types.small, &theme)
                    .ink(colors.ink_2)
                    .whitespace_nowrap()
                    .child(text)
                    .into_any_element(),
            };
            div()
                .id(("mark", index))
                .flex()
                .flex_none()
                .items_center()
                .tooltip(tooltip(label))
                .child(drawn)
        });
        div()
            .id(self.id)
            .role(Role::ListItem)
            .aria_label(label)
            .when_some(self.selector, |row, selector| {
                row.debug_selector(move || selector.to_string())
            })
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .h(u(theme.metrics.occupant_row_height))
            .pl(u(37.0))
            .pr(u(10.0))
            .typeface(theme.types.control, &theme)
            .font_weight(gpui::FontWeight::NORMAL)
            .ink(colors.ink_2)
            .hover(move |style| style.bg(Hsla::from(colors.limestone_hover)))
            .when(self.open, |row| row.bg(Hsla::from(colors.limestone_hover)))
            .when_some(self.on_menu, |row, open| {
                let ring = focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
                let (by_click, by_key) = (open.clone(), open);
                row.focusable()
                    .tab_stop(true)
                    .focus_visible(move |style| style.shadow(ring))
                    .on_mouse_down(gpui::MouseButton::Right, move |_, window, cx| {
                        by_click(window, cx);
                    })
                    .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                        let keys = &event.keystroke;
                        let opens = match keys.key.as_str() {
                            "menu" | "enter" | "space" => true,
                            "f10" => keys.modifiers.shift,
                            _ => false,
                        };
                        if opens {
                            by_key(window, cx);
                            cx.stop_propagation();
                        }
                    })
            })
            .child(Mosaic::new(MosaicKind::Large, self.seed, 20.0).radius(2.0))
            .child(div().flex_1().min_w_0().truncate().child(self.name))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(7.0))
                    .children(marks),
            )
    }
}
