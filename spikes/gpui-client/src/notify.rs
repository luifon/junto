use gpui::{
    App, Context, IntoElement, ParentElement as _, Render, Styled as _, SystemNotification,
    SystemNotificationAction, SystemNotificationResponse, Window, div,
};
use gpui_component::{Sizable as _, button::Button, h_flex, v_flex};

use crate::support::{EventLog, Palette, note, u};

const TAG: &str = "junto-spike-mention";

pub fn install_response_handler(cx: &mut App) {
    cx.on_system_notification_response(|response, cx| {
        let SystemNotificationResponse { tag, action_id } = response;
        match action_id {
            Some(action) => note(
                cx,
                format!("notification: action '{action}' received for tag '{tag}'"),
            ),
            None => note(cx, format!("notification: body clicked for tag '{tag}'")),
        }
    });
}

pub struct NotifyGate {
    posted: usize,
}

impl NotifyGate {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { posted: 0 }
    }
}

impl Render for NotifyGate {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bundle = std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string().contains(".app/Contents/MacOS"))
            .unwrap_or(false);
        let mut lines: Vec<_> = cx
            .try_global::<EventLog>()
            .map(|log| {
                log.lines
                    .iter()
                    .filter(|line| line.contains("notification"))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let excess = lines.len().saturating_sub(12);
        lines.drain(..excess);

        v_flex()
            .size_full()
            .p(u(16.0))
            .gap(u(10.0))
            .child(format!(
                "running from an application bundle: {}",
                if bundle { "yes" } else { "no" }
            ))
            .child(
                h_flex()
                    .gap(u(8.0))
                    .child(Button::new("notify").small().label("Show notification").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.posted += 1;
                            cx.show_system_notification(SystemNotification {
                                tag: TAG.into(),
                                title: format!("Luca mentioned you ({})", this.posted).into(),
                                body: "@you can you review the retention settings before lunch?"
                                    .into(),
                                actions: vec![
                                    SystemNotificationAction {
                                        id: "reply".into(),
                                        label: "Reply".into(),
                                    },
                                    SystemNotificationAction {
                                        id: "mark-read".into(),
                                        label: "Mark as read".into(),
                                    },
                                ],
                            });
                            note(cx, format!("notification: posted number {}", this.posted));
                            cx.notify();
                        }),
                    ))
                    .child(Button::new("dismiss").small().label("Dismiss").on_click(
                        |_, _, cx| {
                            cx.dismiss_system_notification(TAG);
                            note(cx, "notification: dismissed");
                        },
                    )),
            )
            .child(div().text_color(Palette::muted()).child(format!("posted: {}", self.posted)))
            .children(lines.into_iter().map(|line| div().text_size(u(13.0)).child(line)))
    }
}
