//! The release notes in a dialog: once at the first start after an update, with the notes of
//! every version since the one that started last, and from "What is new" in the settings
//! (ADR 0727).

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _,
};
use junto_client_core::protocol::message::Document;
use junto_client_core::updates::Version;

use super::settings::SettingsSection;
use super::{Layer, Shell};
use crate::state::feed::Names;
use crate::text;
use crate::theme::{theme, u};
use crate::ui::{Button, ButtonKind, Dialog, StyleExt as _, dialog_footer};

pub(crate) struct ReleaseNotesDialog {
    notes: Vec<(Version, Document)>,
    /// The settings opened the dialog, and they open again when it closes.
    from_settings: bool,
}

impl Shell {
    pub(crate) fn open_release_notes(
        &mut self,
        notes: &[(Version, &'static str)],
        from_settings: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if notes.is_empty() {
            return;
        }
        let notes = notes
            .iter()
            .map(|(version, text)| (*version, crate::compose::parse(text, &[])))
            .collect();
        let dialog = ReleaseNotesDialog {
            notes,
            from_settings,
        };
        self.open_layer(Layer::ReleaseNotes(Box::new(dialog)), window, cx);
    }

    fn close_release_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let back = matches!(&self.layer, Layer::ReleaseNotes(dialog) if dialog.from_settings);
        self.close_layer(window, cx);
        if back {
            self.open_settings(window, cx);
            self.open_settings_section(SettingsSection::About, window, cx);
        }
    }

    pub(crate) fn render_release_notes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Layer::ReleaseNotes(dialog) = &self.layer else {
            return div().into_any_element();
        };
        let notes = dialog.notes.clone();
        let theme = theme(cx);
        let names = Names::default();
        let mut parts: Vec<AnyElement> = Vec::new();
        for (index, (version, document)) in notes.iter().enumerate() {
            let selector = format!("release-notes-{version}");
            let title = div()
                .typeface(theme.types.name, &theme)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .ink(theme.colors.ink)
                .child(text::release_title(&version.to_string()));
            parts.push(
                div()
                    .debug_selector(move || selector.clone())
                    .flex()
                    .flex_col()
                    .gap(u(6.0))
                    .when(index > 0, |part| part.mt(u(18.0)))
                    .child(title)
                    .children(self.document((None, None), document, None, &names, &theme, cx))
                    .into_any_element(),
            );
        }
        Dialog::new(text::WHAT_IS_NEW, self.layer_focus())
            .selector("release-notes")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_release_notes(window, cx);
            }))
            .children(parts)
            .child(dialog_footer([Button::new("release-notes-close")
                .kind(ButtonKind::Primary)
                .label(text::CLOSE)
                .selector("release-notes-close")
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(|this, _, window, cx| this.close_release_notes(window, cx)))
                .into_any_element()]))
            .into_any_element()
    }
}
