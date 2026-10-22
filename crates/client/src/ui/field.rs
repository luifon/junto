//! The text field: one line of text that a person types. The editing engine is the text
//! input of the component library (ADR 0701); the frame, the colours and the type are the
//! design's.

use gpui::{
    AnyElement, App, Context, Entity, Focusable, Hsla, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, Rgba, Role, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder,
};
use gpui_base::input::{Input, InputEditorStyle, InputEvent, InputState};

use super::StyleExt;
use crate::theme::{Theme, Type, theme, u};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldKind {
    /// A field on stone.
    #[default]
    Plain,
    /// A code that a person received: fixed-width type.
    Code,
    /// The address field of the server book, on basalt.
    RailAddress,
    /// A field that searches a list: lower, with the search icon before the text.
    Find,
}

/// What a field tells its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldEvent {
    Changed,
    /// Enter was pressed in the field.
    Submitted,
}

#[derive(IntoElement)]
pub struct Field {
    state: Entity<InputState>,
    kind: FieldKind,
    label: SharedString,
    selector: Option<&'static str>,
    trailing: Option<AnyElement>,
}

impl Field {
    /// A field over the text that `state` holds. `label` names it in the accessibility
    /// tree.
    pub fn new(state: &Entity<InputState>, label: impl Into<SharedString>) -> Self {
        Self {
            state: state.clone(),
            kind: FieldKind::Plain,
            label: label.into(),
            selector: None,
            trailing: None,
        }
    }

    /// The text of a new field: empty, with a placeholder.
    pub fn state<T: 'static>(
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Entity<InputState> {
        let placeholder = placeholder.into();
        gpui::AppContext::new(cx, |cx| {
            InputState::new(window, cx).placeholder(placeholder)
        })
    }

    /// Tells `owner` each change of the text and each press of Enter.
    pub fn observe<T: 'static>(
        state: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<T>,
        handler: impl Fn(&mut T, FieldEvent, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        cx.subscribe_in(
            state,
            window,
            move |owner, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => handler(owner, FieldEvent::Changed, window, cx),
                InputEvent::PressEnter { .. } => {
                    handler(owner, FieldEvent::Submitted, window, cx);
                }
                InputEvent::Focus | InputEvent::Blur => cx.notify(),
            },
        )
    }

    pub fn kind(mut self, kind: FieldKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn selector(mut self, selector: &'static str) -> Self {
        self.selector = Some(selector);
        self
    }

    /// An element at the end of the field, inside its frame.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

struct Look {
    height: f32,
    padding: f32,
    background: Rgba,
    border: Rgba,
    border_focused: Rgba,
    text: Rgba,
    placeholder: Rgba,
    caret: Rgba,
    selection: Rgba,
    type_style: Type,
}

fn look(kind: FieldKind, theme: &Theme) -> Look {
    let c = &theme.colors;
    match kind {
        FieldKind::Plain => Look {
            height: 32.0,
            padding: 9.0,
            background: c.travertine_hi,
            border: c.rule,
            border_focused: c.ink_2,
            text: c.ink,
            placeholder: c.ink_3,
            caret: c.ink,
            selection: c.terracotta_wash,
            type_style: theme.types.row,
        },
        FieldKind::Code => Look {
            height: 32.0,
            padding: 9.0,
            background: c.travertine,
            border: c.ink_2,
            border_focused: c.ink,
            text: c.ink,
            placeholder: c.ink_3,
            caret: c.ink,
            selection: c.terracotta_wash,
            type_style: theme.types.mono_code,
        },
        FieldKind::Find => Look {
            height: 28.0,
            padding: 6.0,
            background: c.travertine,
            border: c.rule,
            border_focused: c.ink_2,
            text: c.ink,
            placeholder: c.ink_3,
            caret: c.ink,
            selection: c.terracotta_wash,
            type_style: theme.types.control,
        },
        FieldKind::RailAddress => Look {
            height: 30.0,
            padding: 8.0,
            background: c.basalt_deep,
            border: c.basalt_rule,
            border_focused: c.ochre,
            text: c.bone,
            placeholder: c.bone_3,
            caret: c.bone,
            selection: c.basalt_rule,
            type_style: theme.types.mono_field,
        },
    }
}

impl RenderOnce for Field {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let look = look(self.kind, &theme);
        let focus = self.state.read(cx).focus_handle(cx);
        let focused = focus.is_focused(window);
        self.state.update(cx, |state, _| {
            state.set_editor_style(InputEditorStyle {
                foreground: look.text.into(),
                muted_foreground: look.placeholder.into(),
                background: look.background.into(),
                border: look.border.into(),
                selection: look.selection.into(),
                caret: look.caret.into(),
                ..InputEditorStyle::default()
            });
        });
        let border = if focused {
            look.border_focused
        } else {
            look.border
        };
        div()
            .id(("field", self.state.entity_id()))
            .role(Role::TextInput)
            .aria_label(self.label)
            .when_some(self.selector, |frame, selector| {
                frame.debug_selector(|| selector.to_owned())
            })
            .flex()
            .items_center()
            .h(u(look.height))
            .min_w_0()
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(border))
            .bg(Hsla::from(look.background))
            .typeface(look.type_style, &theme)
            .ink(look.text)
            .when(self.kind == FieldKind::Find, |frame| {
                frame.child(div().flex_none().pl(u(8.0)).child(super::icon(
                    crate::assets::Icon::Search,
                    14.0,
                    look.placeholder,
                )))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(u(look.padding))
                    .child(Input::new(&self.state)),
            )
            .children(self.trailing)
            .children(super::reveal::report(&focus, window))
    }
}
