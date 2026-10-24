use gpui::{IntoElement, SharedString};

pub(crate) fn paint_text(text: SharedString, label: Option<SharedString>) -> impl IntoElement {
    #[cfg(feature = "test-support")]
    {
        RecordedText { text, label }
    }
    #[cfg(not(feature = "test-support"))]
    {
        let _ = label;
        text
    }
}

#[cfg(feature = "test-support")]
#[derive(Debug, Clone)]
pub struct PaintedText {
    pub text: SharedString,
    pub label: Option<SharedString>,
    pub bounds: gpui::Bounds<gpui::Pixels>,
}

#[cfg(feature = "test-support")]
thread_local! {
    static PAINTED: std::cell::RefCell<Option<Vec<PaintedText>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "test-support")]
#[must_use]
pub struct TextPaintRecord(());

#[cfg(feature = "test-support")]
impl Drop for TextPaintRecord {
    fn drop(&mut self) {
        PAINTED.set(None);
    }
}

#[cfg(feature = "test-support")]
pub fn record_painted_text() -> TextPaintRecord {
    PAINTED.set(Some(Vec::new()));
    TextPaintRecord(())
}

#[cfg(feature = "test-support")]
pub fn take_painted_text() -> Vec<PaintedText> {
    PAINTED.with_borrow_mut(|painted| painted.as_mut().map(std::mem::take).unwrap_or_default())
}

#[cfg(feature = "test-support")]
struct RecordedText {
    text: SharedString,
    label: Option<SharedString>,
}

#[cfg(feature = "test-support")]
impl IntoElement for RecordedText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[cfg(feature = "test-support")]
impl gpui::Element for RecordedText {
    type RequestLayoutState = <SharedString as gpui::Element>::RequestLayoutState;
    type PrepaintState = <SharedString as gpui::Element>::PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        layout: &mut Self::RequestLayoutState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Self::PrepaintState {
        self.text
            .prepaint(id, inspector, bounds, layout, window, cx)
    }
    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        self.text
            .paint(id, inspector, bounds, layout, prepaint, window, cx);
        if window.content_mask().bounds.intersects(&bounds) {
            PAINTED.with_borrow_mut(|painted| {
                if let Some(painted) = painted {
                    painted.push(PaintedText {
                        text: self.text.clone(),
                        label: self.label.clone(),
                        bounds,
                    });
                }
            });
        }
    }
}
