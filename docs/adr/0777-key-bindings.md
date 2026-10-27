# 0777. The key bindings of the person

## Status

Accepted

## Context

Junto had a key to quit, the key of push to talk of the sound settings (ADR 0507) and the key that mutes (ADR 0776); every other action needed the pointer or a menu. Discord's Keybinds list lets a person give each action a key combination, with defaults, also for actions that work while another application is in front, such as the mute key. Slack and Teams list keyboard shortcuts for moving between workspaces, channels and unread channels, for search and for marking read.

## Decision

The actions (`keybinds::KeyAction`), each with a default where Discord has the action (the actions of the window use Command on macOS where they use Control elsewhere; the actions of a call use Control and Shift on every system, as Discord):

- Call: mute or unmute (Control+Shift+M), turn the sound off or on (Control+Shift+D), push to talk (held; the key of the sound settings while the person sets no other), push to mute (held; no default), turn the camera on or off (Control+Shift+E), share a screen or a window (Control+Shift+S, the dialog of a share), leave the call (Control+Shift+H), answer and decline a call that rings (Control+Enter, Control+Shift+Backspace; the first ringing call), show the call (Control+Alt+A), open or close the panel of the call (Control+Shift+P; the action `toggle_call_panel`, whose wide form branch B builds).
- Navigation: back and forward through the places that the window showed (Alt+Left and Alt+Right, Command+[ and Command+] on macOS, as Discord and Slack), previous and next server (Control+Alt+Up and Down), workspace (Control+Shift+Up and Down), channel (Alt+Up and Down), unread channel (Alt+Shift+Up and Down), collapse or expand the server book (Control+Shift+B), search (Control+F), notices (Control+I), jump to the newest message (Shift+Page Down), members (Control+U), pinned messages (Control+P). A step goes around the end of its list.
- Messages: mark the channel as read (Shift+Escape; Escape closes menus and dialogs in Junto), mark the server as read (Control+Shift+Escape), reply to the newest message (Control+Shift+R), upload a file (Control+Shift+U), the direct messages (Control+Shift+K, as Slack: the newest conversation of the shown server), a new direct message (Control+N). The edit of the last own message with Up in an empty composer stays as it is.
- Junto: settings (Control+Comma), administration (Control+Shift+Comma), the Keybinds section (Control+Slash). Quit stays the key of the system (Command+Q, Control+Q) and is not a binding.
- The list is the one registry of the actions of Junto that a key does: a later action joins it with its default. The keys of the quick switcher (Command or Control with K) and of the zoom (with =, - and 0) are reserved for such later actions (`KeyAction::reserved`); no default takes them.
- Back and Forward: the window state keeps the places that it showed (a server with its workspace and its channel, or a direct conversation), at most 50 in each direction; a new place after Back forgets what Forward held.

The bindings (`keybinds::Keybinds`):

- A binding has its keys or none, and for an action of a call that needs no window (mute, sound off, push to talk, push to mute, camera, leave, answer, decline) whether it also works while another application is in front. Mute, sound off, push to talk and push to mute work outside by default, as in Discord; the others while the person switches it on.
- Keys are modifiers and at most one other key: a letter, a digit, F1 to F24, an arrow, Enter, Escape, Backspace, Page Up, Page Down, comma, slash, =, -, [ or ]. The keys of an action that happens at a press need a modifier, so that typing does not do it; the keys of a held action can be one key.
- The settings file of the interface keeps the bindings that differ from the defaults, by the name of the action (format 3; a file of format 1 has none, every action has its default). A file whose binding breaks these rules is read without that binding. A change is written under the lock of the data directory with only that change, so that a second program keeps its other changes (ADR 0710).
- The key that mutes of ADR 0776 is this binding: the sound settings stay at format 4 and have no key that mutes. Push to talk keeps the key of the sound settings (Alt or Control, ADR 0507) until the person sets another one; its reset returns to that key. The binding stores inheritance separately from an explicitly removed key, so changing only the outside switch keeps the inherited key. Format 2 push-to-talk overrides with no keys migrate to inheritance; format 2 could not keep an explicit removal with the default outside switch.
- Two actions with the same keys are a conflict, which both rows state; a press does the first of them in the order of the list. The polling reader and portal remove the other equal bindings before dispatch or registration, preserving the order of distinct bindings.

In the window:

- Every window of Junto, the main window and the call window, reads its key presses before the focused element (`capture_key_down`): a press whose keys are a binding does its action and goes no further. A binding that the keyboard of the system reads now is not done a second time by the window.
- Push to talk and push to mute hold while the window holds their keys: the modifiers and the last key that went down and not up.
- The settings have a section "Keybinds": how the keys reach Junto on this system (ADR 0776, with the permission of macOS), "Reset all", and for each group and action its keys with "Set" (the next keys that the person presses; Escape stops; refused keys are stated and the wait goes on), "Remove", "Reset" while it differs from its default, the switch "Also while another application is in front" for an action that may, and the conflict. The Voice section points to it.

Outside the window: the keyboard of the system (ADR 0776) reads the bindings that work outside while this device is in a call, and answer and decline while a call rings; push to talk only while push to talk is on. A Wayland desktop gets one shortcut per binding through its portal, named by the action. When the held keys hold two bound combinations and one holds the other, only the larger one is a press, and a press counts when its own combination becomes held.

Tests: `keybinds::tests` (defaults without conflicts and without a reserved combination, change, conflict, reset, the modifier rule, the actions that may work outside, the file form), `settings::tests::a_file_of_version_1_has_the_default_bindings_and_each_change_keeps_the_other_part`, `global_keys::tests` (presses and holds of several bindings, the larger combination), `global_keys::portal::tests`, on Windows `global_keys::windows::tests::a_binding_of_a_modifier_and_an_arrow_is_one_press` (Control and Up pressed through `SendInput`), and the window tests `keybinds::a_binding_is_recorded_its_conflict_shown_and_reset_and_a_second_start_keeps_it`, `keybinds::a_binding_does_its_action_while_the_window_is_in_front`, `keybinds::a_call_binding_works_from_the_keyboard_of_the_system_when_it_may_work_outside`, `keybinds::back_forward_and_the_direct_messages_at_their_keys` and `calls::the_keys_of_a_call_reach_the_call_from_the_source_and_from_the_window`.

## Consequences

- A person can give every action of the list a key, and the call actions also work while another application is in front where the system lets Junto read the keys (ADR 0776).
- The actions of the window work while a window of Junto is in front only.
- A binding of the window can take keys that a field also uses, such as Alt+Up in the composer; the binding takes them.
