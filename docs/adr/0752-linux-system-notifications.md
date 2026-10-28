# 0752. Linux system notifications through D-Bus

## Status

Accepted. The private D-Bus service tests cover replacement, withdrawal, activation, escaping with and without the markup capability, service-owner changes, a missing service, a replaced session bus and an alert that waited too long, and the activation token of a press. The test target `notification_focus` checks the focus after a press under the Wayland compositor sway. Presentation by a real desktop notification daemon has not been run.

## Context

ADR 0742 separates the decision to notify from the system interface. The Linux window backend does not provide that interface. The desktop notification service supplies notification identifiers, replacement, withdrawal and activation through D-Bus. An identifier belongs to one running service, not to every later owner of its bus name.

## Decision

- Linux implements `SystemNotifier` through `org.freedesktop.Notifications` on the session bus. The existing application policy decides whether a notification is shown. This backend introduces no separate notification rules.
- A dedicated worker orders show and dismiss operations. A later alert with the same application tag replaces the service identifier already associated with that tag. Dismissal calls `CloseNotification` for that identifier.
- Notification text is plain application text. The worker asks each service owner once for its capabilities. For a service with the capability `body-markup` the body escapes the markup characters; a service without it shows a body as written and receives the text unchanged. A service that does not answer the question is treated as one that interprets markup, because unescaped text of another person must not reach a service that interprets it.
- The service receives a default Open action, the desktop entry identifier and a hint suppressing notification sound.
- Activation of a known notification returns its application tag to the window thread and uses the same route as the macOS notification response. A closed notification, an unknown identifier or an unrelated action does not select a place.
- gpui has no activation of an application on Linux. After a press the main window is asked to activate itself. Under X11 the window manager gives it the focus. A Wayland compositor gives the focus only for an activation token that it issued: a window that asks for a token of its own without a recent input event of its own gets none that the compositor honours. The notification service sends the token of the press in the signal `ActivationToken` before `ActionInvoked` (version 1.2 of the specification). The worker keeps the token of each shown notification until its next action or its close, and gives it with the press of the default action; the window is then activated with `Window::activate_with_token` of the corrected gpui copy (ADR 0757), which hands the token to `xdg_activation_v1`. A press without a token, from an older service or under X11, activates the window as before. A token that comes with another action is dropped with that action.
- The worker connects to the session bus again, after a pause of 5 seconds, when the connection is lost or cannot be made. The identifiers that the service gave stay valid while the same owner holds the service name on the same bus; the worker compares the identifier of the bus and the owner after it has connected again, and discards the identifiers when either differs. An alert that the application asks for while there is no connection waits for the next one, and the alert that was being shown when the connection was lost is shown first.
- A signal about a new owner of the service name can be older than what the worker already knows. The worker therefore asks the bus for the present owner when such a signal arrives.
- An alert that waited more than 30 seconds for the service is not shown. A service that does not answer therefore cannot cause a row of old notifications when it answers again. The notice stays in the notification panel of the application.
- A change of notification-service owner discards the old identifier map. Requests are addressed to the unique service owner so a restart cannot turn a dismissal into a request against an unrelated identifier from the new service.
- Absence or failure of the notification service is reported in the log, once for each interruption. It does not remove the in-application notice or interrupt the client.
- Production installs the system backend only from the binary's normal application start. Tests and the screenshot tool retain `KeptNotifier`. Protocol tests run a fake notification service on their own private session bus and show no desktop notification.

## Consequences

- A desktop may choose how long it displays a notification, whether to show its action and whether to honour hints. The application still keeps its notices in the notification panel.
- Replacement, withdrawal, activation, service restart and missing-service behaviour require fake-service tests. A successful fake-service test is not evidence of visual presentation by a real desktop daemon.
- Under Wayland the focus after a press depends on the policy of the compositor for a valid token. GNOME and KDE give the focus; sway gives it only with `focus_on_window_activation focus` and else marks the window as urgent. The test `notification_focus` sets that option.
- Tests: `notify::linux::tests::a_press_carries_the_activation_token_of_its_notification` (the token goes only with the press of its own notification), the window test `notices::a_pressed_notification_brings_the_window_to_the_front` (with and without a token), and `notification_focus` (`tests/notification_focus/main.rs`, without the test runner, `JUNTO_TEST_NOTIFICATION_FOCUS=1`, run by the Linux CI job under the sway of `tools/linux/display.sh`): the application on the run loop of the system with the main window and a second window that has the focus, a notification service on a private session bus that takes a token from the compositor and sends it with the press, and a check through `swaymsg` that the main window has the focus. Before the token was passed, the focus stayed on the second window.

## References

- [Desktop notification protocol](https://specifications.freedesktop.org/notification/latest/protocol.html).
