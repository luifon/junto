//! The application of gpui on the platform of macOS, for windows of the harness.
//!
//! `VisualTestAppContext` opens real windows of the system and draws them with Metal. Its
//! tasks run on the test scheduler of gpui, as in the harness, and not on the run loop
//! of the product. The harness reads drawn elements through a `TestAppContext`; this
//! module gives it one whose application is the native one (ADR 0902).

use std::sync::Arc;

use gpui::{TestAppContext, TestDispatcher, VisualTestAppContext};

/// The native application and the test context that the harness uses on it.
pub struct Native {
    /// Holds the application of the system while the windows run.
    _visual: VisualTestAppContext,
    pub cx: TestAppContext,
}

impl Native {
    /// Starts the application on the platform of the system. Must run on the main thread.
    pub fn start() -> Self {
        let visual = VisualTestAppContext::with_asset_source(
            gpui_platform::current_platform(false),
            Arc::new(junto_client::assets::Assets),
        );
        // The test context of gpui has no constructor for another platform. Its fields
        // for the application and the executors are public in the pinned version; the
        // context that `build` makes on the test platform is replaced by the native one,
        // so that every call of the harness reaches the native windows.
        let dispatcher = visual
            .background_executor
            .dispatcher()
            .as_test()
            .expect("the visual test platform schedules on the test dispatcher")
            .clone();
        let mut cx = TestAppContext::build(TestDispatcher::new(0), None);
        cx.app = visual.app.clone();
        cx.background_executor = visual.background_executor.clone();
        cx.foreground_executor = visual.foreground_executor.clone();
        cx.dispatcher = dispatcher;
        Self {
            _visual: visual,
            cx,
        }
    }
}
