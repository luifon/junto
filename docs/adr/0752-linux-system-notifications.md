# 0752. Linux system notifications through D-Bus

## Status

Accepted. The private D-Bus service tests cover replacement, withdrawal, activation, escaping, service-owner changes and a missing service. Presentation by a real desktop notification daemon has not been run.

## Context

ADR 0742 separates the decision to notify from the system interface. The Linux window backend does not provide that interface. The desktop notification service supplies notification identifiers, replacement, withdrawal and activation through D-Bus. An identifier belongs to one running service, not to every later owner of its bus name.

## Decision

- Linux implements `SystemNotifier` through `org.freedesktop.Notifications` on the session bus. The existing application policy decides whether a notification is shown. This backend introduces no separate notification rules.
- A dedicated worker orders show and dismiss operations. A later alert with the same application tag replaces the service identifier already associated with that tag. Dismissal calls `CloseNotification` for that identifier.
- Notification text is plain application text. The body escapes markup characters before it is sent to a service that understands markup. The service receives a default Open action, the desktop entry identifier and a hint suppressing notification sound.
- Activation of a known notification returns its application tag to the window thread and uses the same route as the macOS notification response. A closed notification, an unknown identifier or an unrelated action does not select a place.
- A change of notification-service owner discards the old identifier map. Requests are addressed to the unique service owner so a restart cannot turn a dismissal into a request against an unrelated identifier from the new service.
- Absence or failure of the notification service is reported in the log. It does not remove the in-application notice or interrupt the client.
- Production installs the system backend only from the binary's normal application start. Tests and the screenshot tool retain `KeptNotifier`. Protocol tests run a fake notification service on their own private session bus and show no desktop notification.

## Consequences

- A desktop may choose how long it displays a notification, whether to show its action and whether to honour hints. The application still keeps its notices in the notification panel.
- Replacement, withdrawal, activation, service restart and missing-service behaviour require fake-service tests. A successful fake-service test is not evidence of visual presentation by a real desktop daemon.

## References

- [Desktop notification protocol](https://specifications.freedesktop.org/notification/latest/protocol.html).
