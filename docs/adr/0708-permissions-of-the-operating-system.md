# 0708. Permissions of the operating system

## Status

Accepted

The window tests of `system_permissions` and the unit tests run on Linux with fixed permissions and the rules of each system. Not run by a test: the calls of macOS and Windows themselves, which the macOS and Windows jobs build, and a person who answers the questions of macOS.

## Context

ADR 0704 keeps the permissions of macOS across updates through one code-signing identity. Before 0.1.0 every package was signed ad hoc. After a new ad-hoc build replaced the earlier one, System Settings showed Junto as allowed for the screen, and the permission did not apply, because macOS keeps the permission for the signature of the earlier copy; after the entry was removed, macOS did not ask again until Junto was added by hand. Notifications failed with "Notifications are not allowed for this application" without a sign in the window. Junto asked macOS for the microphone and the camera at their first use, showed a sentence after a refusal, and opened System Settings only for the screen.

Each system keeps other permissions:

- macOS keeps an answer of the person per application (the TCC database) for the microphone, the camera, the screen and the sound of other applications, the notifications, the Downloads folder, the local network (macOS 15 and later), and Input Monitoring. It shows its question once per application identity. It does not let a program read the answer for the Downloads folder or the local network; a use that it refused fails (`EPERM` for a file, `EHOSTUNREACH` for a connection). For the screen and Input Monitoring it does not tell a permission that it never asked for from a refused one.
- Windows asks a desktop program nothing. It keeps an on and off state for the microphone and the camera (also "Let desktop apps access …"), and for the notifications of every program and of each program. A desktop program needs no permission to capture the screen.
- Linux keeps no state that a program outside a sandbox can read: the screen-cast portal asks at every share, a camera is a device node that the account can open or not, and the microphone and the notifications have no permission.

The client uses no other permission. Files that a person chooses in an open or a save dialog need none.

## Decision

- The client lists the permissions of each system (`permissions::System::permissions`): on macOS the microphone, the camera, Screen & System Audio Recording, the notifications, the Downloads folder, the local network and Input Monitoring; on Windows the microphone, the camera and the notifications; on Linux the camera and the screen sharing. Input Monitoring is listed while the client reads keys while another application is in front: the key of push to talk.
- What the client knows of a permission (`permissions::Access`): allowed, not asked, refused, restricted by a rule of the machine or by the devices, asked at each use, and "not read" for a permission whose answer the system keeps to itself until a use.
- The action of a permission (`permissions::action`): "Allow" makes the system ask while it has not asked; a refused, restricted or not read permission has the button of the page of the settings of the system where one exists. On macOS the screen and Input Monitoring button first makes macOS list Junto and ask once, then opens the page, because macOS states the two cases alike. Linux has no page: the sentence says what to do.
- The pages are `x-apple.systempreferences:` addresses of Privacy & Security (`Privacy_Microphone`, `Privacy_Camera`, `Privacy_ScreenCapture`, `Privacy_FilesAndFolders`, `Privacy_LocalNetwork`, `Privacy_ListenEvent`) and of the notifications of `org.junto.app`, and on Windows `ms-settings:privacy-microphone`, `ms-settings:privacy-webcam` and `ms-settings:notifications`. The window opens them through the platform (`open_url`); the screen page of macOS through the screen capture, as before.
- The reads: the microphone and the camera through the media engine (AVFoundation on macOS, the access information of the device class on Windows, the device nodes on Linux), the screen through the capture, the notifications through the notification centre of macOS and the notifier setting of the identifier of the application on Windows (`notification_access` in the client core), Input Monitoring through CoreGraphics (`input_monitoring`). A read shows no question. The window reads every permission at the start, each time a window of the application becomes active (the person changes a permission in another application and comes back), when the settings show the section "Permissions" or "Notifications", and on "Check again". A read that a newer read follows does not replace it.
- The questions: the notifications are asked for at the first start while macOS has not asked, as Discord and Slack do; the microphone and the camera at their first use, as before; the screen when the person starts a share or presses the button; Input Monitoring on its button. A refusal is not asked again: macOS shows its question once.
- The Downloads folder is refused when a save into the download directory failed with "permission denied"; the local network is refused when the last request to a server failed because macOS answered `EHOSTUNREACH` to a connection to a host that can be on the local network (a private, shared, link-local or unique local address, or a name). The request that the server answers again clears it. The Info.plist of the bundle states why Junto uses the Downloads folder and the local network.
- The settings have a section "Permissions" before "Version" with each permission of the system, what Junto uses it for, its state and the sentence of what to do, and its button. A refused permission of macOS also says what to do when Junto is listed and turned on but not allowed: remove the entry with the minus button and allow it again. The places of use have the same button: the microphone line of the call settings and the warning of a refused microphone, the notice of a refused camera, the dialog of a share, the section "Notifications", a failed download, and the statement of a server that macOS did not let Junto reach; the error of a new address names the step.
- Tests replace each system call: a fixed microphone permission, the synthetic camera and capture, a fixed notification permission and a fixed Input Monitoring permission. A test of the window names the rules of a system, so the rules of macOS and Windows are tested on Linux. `junto-screens` draws the section with the rules of each system.

## Consequences

- A person sees which permission Junto lacks and gets to its page in one press. The steps of macOS for an entry of an earlier signature are written where the refusal is shown.
- 0.1.0 is signed with the permanent identity, so the permissions that a person gives it stay after each update (ADR 0704). The test versions had other signatures: their entries do not apply to 0.1.0, and the person allows Junto once more.
- Junto asks for the notifications at the first start of each installation that macOS has not asked for. The other questions come at their first use.
- The local-network sentence can also appear for a server name whose address is public and that is unreachable for another reason that gives `EHOSTUNREACH`; it is phrased as what macOS did, and "Try again" stays.
- Not tested: the questions and pages of macOS and Windows themselves, the notification centre of macOS, and a refusal of the local network on a real network. The coordinator checks them on the Mac and on the Windows machine.
