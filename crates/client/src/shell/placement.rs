use super::Shell;
use gpui::{Context, Window};

impl Shell {
    pub(crate) fn observe_placement(
        &mut self,
        detached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions
            .push(cx.observe_window_bounds(window, move |this, window, cx| {
                let saved = if detached {
                    this.state.read(cx).call_window_settings()
                } else {
                    this.state.read(cx).main_window_settings()
                };
                if let Some(settings) = crate::window_placement::remember(
                    saved,
                    window.window_bounds(),
                    window.display(cx),
                ) {
                    this.state.update(cx, |state, cx| {
                        if detached {
                            state.save_call_window(settings, cx);
                        } else {
                            state.save_main_window(settings, cx);
                        }
                    });
                }
            }));
    }
}
