//! The taskbar of Windows (ADR 0755): the main window states the identifier of the
//! application and what a pin made from its taskbar button starts, and takes them away before
//! it closes; during a call its preview has the buttons of the microphone, the sound and
//! leaving.

use gpui::Window;

pub(crate) fn handle(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Gives the main window the identifier and the relaunch properties of this program.
pub fn identify(window: &Window) {
    let Some(handle) = handle(window) else {
        return;
    };
    let program = match std::env::current_exe() {
        Ok(program) => program.display().to_string(),
        Err(error) => {
            tracing::warn!(%error, "the taskbar properties were not set: no program path");
            return;
        }
    };
    let relaunch = junto_client_core::taskbar::Relaunch {
        identifier: junto_client_core::app_identity::APP_ID,
        command: &format!("\"{program}\""),
        name: crate::text::APP_NAME,
        icon: &format!("{program},0"),
    };
    if let Err(error) = junto_client_core::taskbar::set(handle, &relaunch) {
        tracing::warn!(%error, "the taskbar properties were not set");
    }
    junto_client_core::taskbar::mark_pins_of_this_program(junto_client_core::app_identity::APP_ID);
}

/// Takes the properties of [`identify`] away, before the window is destroyed.
pub fn forget(window: &Window) {
    if let Some(handle) = handle(window)
        && let Err(error) = junto_client_core::taskbar::clear(handle)
    {
        tracing::warn!(%error, "the taskbar properties were not taken away");
    }
}

/// Takes the presses on the buttons of the taskbar preview of the main window during a call
/// and gives them to the window (ADR 0755).
pub fn watch_buttons(window: &Window, cx: &mut gpui::Context<crate::shell::Shell>) {
    use futures_util::StreamExt as _;
    let Some(handle) = handle(window) else {
        return;
    };
    let (sender, mut presses) = futures_channel::mpsc::unbounded();
    let watched = junto_client_core::taskbar::thumb::watch(
        handle,
        Box::new(move |press| {
            let _ = sender.unbounded_send(press);
        }),
    );
    if let Err(error) = watched {
        tracing::warn!(%error, "the buttons of the taskbar preview take no presses");
        return;
    }
    cx.spawn(async move |shell, cx| {
        while let Some(press) = presses.next().await {
            if shell
                .update(cx, |shell, cx| shell.thumb_press(press, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

thread_local! {
    static SHOWN: std::cell::RefCell<Option<Option<junto_client_core::taskbar::thumb::ThumbButtons>>> =
        const { std::cell::RefCell::new(None) };
}

/// Shows the buttons of the call of the controls `view` on the preview of the main window, or
/// hides them outside a call: the microphone, the sound and leaving, with the state of the
/// controls of the window.
pub fn show_buttons(window: &Window, view: &crate::state::view::ShellView) {
    use crate::state::view::MicrophoneButton;
    use junto_client_core::taskbar::thumb::ThumbButtons;
    let controls = &view.controls;
    let buttons = view.call.is_some().then(|| {
        let open = controls.microphone == MicrophoneButton::Open;
        ThumbButtons {
            microphone_open: open,
            microphone_usable: matches!(
                controls.microphone,
                MicrophoneButton::Open | MicrophoneButton::Muted
            ),
            sound_off: controls.sound_off,
            tips: [
                if open {
                    crate::text::CLOSE_MICROPHONE
                } else {
                    crate::text::OPEN_MICROPHONE
                }
                .to_owned(),
                if controls.sound_off {
                    crate::text::SOUND_OUTPUT_OFF
                } else {
                    crate::text::SOUND_OUTPUT_ON
                }
                .to_owned(),
                crate::text::LEAVE_CALL.to_owned(),
            ],
        }
    });
    let changed = SHOWN.with_borrow_mut(|shown| {
        let changed = shown.as_ref() != Some(&buttons);
        *shown = Some(buttons.clone());
        changed
    });
    if changed && let Some(handle) = handle(window) {
        junto_client_core::taskbar::thumb::show(handle, buttons);
    }
}
