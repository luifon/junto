# 0710. Client data directory and file formats

## Status

Accepted

## Context

The client keeps a server book (ADR 0101), a profile preset (ADR 0105) and one device key per server (ADR 0103). The records do not state where these files are, in which format, or what happens when a write is interrupted, when two programs use the files at once, or when a file cannot be read. The desktop client and the command-line tool use the same files.

## Decision

Location:

- One data directory holds every file of the client.
  - macOS: `$HOME/Library/Application Support/Junto`.
  - Windows: `%LOCALAPPDATA%\Junto`. The data does not roam to other machines, because a device key belongs to one device.
  - Linux: `$XDG_DATA_HOME/junto` when the variable holds an absolute path, `$HOME/.local/share/junto` otherwise.
- The environment variable `JUNTO_DATA_DIR` with an absolute path replaces the location. Tests and the command-line tool use it.
- The client creates the directory so that only the user can access it. A directory that already exists is used only when it belongs to the user and nobody else can access it; a link in its place is refused (ADR 0711).

Files:

| Path | Content |
|---|---|
| `servers.json` | The server book |
| `profile.json` | The profile preset: a name and the media type of the avatar |
| `avatar.png`, `avatar.jpg` or `avatar.webp` | The avatar of the preset, at most 256 KiB, the bound of a server (ADR 0119) |
| `settings.json` | The settings of the interface: the interface scale and the form of the server book (ADR 0731) |
| `voice.json` | The sound settings: the chosen devices, the processing of the microphone, the input mode and its key, the volume per account of a server (ADR 0734), and the settings of shares: resolution, frame rate, the volume of a watched share, and "hide people without video" (ADR 0738), and the switch of the sounds of call events (ADR 0707) |
| `emoji-fonts.json` | The emoji of the table that the fonts of the system do not draw whole and those that a font without colour draws, with the key of the fonts that they were measured with (ADR 0758) |
| `keys/<server identifier>.key` | The device key of one server (ADR 0711) |
| `lock` | An empty file that carries the lock of the directory |

- A file name is fixed text or a server identifier written as a UUID. No file name contains text that a server sent.
- Every file is created with access for the user only.

Server book:

- An entry holds the server identifier, a generation, the address, the name, the path of the logo on the server, the API versions that the server reported last, the connection mode (`always` or `when_selected`, ADR 0107), the membership, the last event cursor, the state of the profile preset (ADR 0717), the identifiers of the newest 100 moderation notices that the person put away (ADR 0764), and whether the person logged out of the account of the membership on this device (ADR 0712).
- The generation is a random number that the book gives an entry when it is added. A server that is removed and added again gets another generation. A change names the identifier and the generation of the entry that it was made for, and the book refuses it when that entry is gone, so a client with an older view cannot change or remove the entry that replaced it.
- The membership is what the client learned at its last contact: `none`, `join_requested` with the name and, once the server has answered, the request identifier, `member` with the account and device identifiers, `banned`, or `removed`.
- A join request is stored as `join_requested` without an identifier before the request is sent (ADR 0712).
- A server identifier is saved once and an address is saved once. A second address that reports a saved identifier is refused.
- The identifier, the generation and the address of an entry never change. A server at a new address is a new entry.
- The name is stored without control characters and with at most 64 characters. The logo path is stored only when it is a path on the same server. At most 16 API versions are stored.
- The event cursor is text of at most 512 bytes. The client keeps what a file holds there and writes no cursor: it stores no state of a server that a cursor could continue (ADR 0718).

Not stored:

- A session token is held in memory only. A client that starts logs in with its device key (ADR 0109).
- An owner, invite or link code is never written to a file.

Format version:

- `servers.json` and `profile.json` are JSON objects with the field `format_version`. This record describes version 4 of `servers.json` and version 2 of `profile.json`.
- `settings.json` has the same form and is at version 1. A scale outside 90 to 150 percent is read as the nearest bound.
- `voice.json` has the same form and is at version 4. Version 1 had no settings of shares; a file of version 1 gets their defaults. Version 2 had no chosen camera; a file of version 2 gets none. Version 3 had no switch of the sounds of call events; a file of version 3 gets the sounds on. A frame rate that is not 15, 30 or 60 is read as 30. A volume above 200 percent is read as 200, a volume of 100 percent of an account is not stored, and the file holds at most 2048 volumes. The identifier of a device is text of the system of at most 512 bytes; the file holds no text that a server sent.
- Version 1 of `profile.json` named the format of the avatar `png`, `jpeg` or `webp`; version 2 stores its media type. Version 1 of `servers.json` had no state `refused` of the profile preset (ADR 0717); its content is otherwise the content of version 2. Version 2 had no moderation notices put away; its content is otherwise the content of version 3, with none put away. Version 3 had no logout of the person; its content is otherwise the content of version 4, without a logout. A new field is a new version too, so that an older client does not drop it when it writes the file. A new value of a stored field is a new version, so that an older client refuses the file and does not report it as corrupted.
- A file with a higher version than the client knows is refused and never overwritten.
- A file with a lower version is migrated in memory, one version step at a time, and written in the current version at the next change.
- A key file states its version in its first line (ADR 0711).

Writes:

- A file is replaced in one step: the new content is written to a temporary file in the same directory, flushed to the disk, and renamed over the file. On Unix the directory is flushed too.
- Every change of a stored file happens under an exclusive lock on `lock`. A change of the server book reads the file again under the lock, changes one entry and writes the file. Two programs that use one data directory do not lose each other's changes. A thread that holds the lock can take it again, so one step can change the book and a key file under one lock. The lock is known by the identity of the opened directory (the device and file number on Unix, the volume and file index on Windows) and not by the path, so a second path to the same directory is the same lock.
- Removing a server deletes its entry and its key file under one hold of the lock. Forgetting an account deletes the key file and clears the membership in the same way. A key is created only while the book still has the entry that it is for, under the lock.
- Every file is opened through the opened data directory and never through a path that could name another object since the directory was checked (ADR 0711).
- A membership or a profile state that could not be written stays marked as unsaved in memory. The mark is cleared only by a write that succeeded, and every later action of that server tries the write again.
- The saves of one server run one after the other: reading what is written, writing it and clearing the mark are one step that no other save of that server enters. A save that started earlier therefore cannot write after a later one.
- A temporary file that an interrupted write left is removed at the next start, under the lock.

Unreadable files:

- A server book or a profile that is not valid JSON, has no version, or holds an invalid entry is reported as corrupted. Both are read when the client starts. The client does not start with a corrupted one and does not change it.
- `settings.json` is the exception: it holds nothing that a person cannot set again. The desktop client starts with the default settings when the file cannot be read. A change of a setting replaces a file that is not a settings file. It replaces no other file that it cannot read: not a file of a newer format, and not a file that is larger than the client reads, whose format the client does not learn.
- A change of one setting reads the file again under the lock, changes that setting and writes the file, as a change of the server book does.
- `voice.json` is read and replaced by the same rules as `settings.json`.
- `emoji-fonts.json` is at version 1 and holds at most 512 KiB. It holds nothing that a person set: a file that cannot be read, or whose key differs from the key of the fonts, is measured again and replaced. A file of a newer format is not replaced.
- The client can move a corrupted server book aside under the name `servers.json.corrupt-<time>` and start with an empty book. This is an explicit call, never an automatic step. The key files stay, so a server that is added again finds its key.

## Consequences

- A reader of a file sees the old content or the new content, never a part of either.
- The command-line tool and the desktop client can run at the same time on one data directory.
- A newer client can change the format, and an older client then refuses the files and does not damage them.
- Each start of the client costs one login per server, and no token exists on the disk.
- The membership in the book can be older than the state on the server. The client corrects it at the next contact (ADR 0712).
- A data directory that the user created by hand with access for others, or that another user owns, makes the client refuse to start, with an error that names the path.
- A corrupted book needs a decision of the user. Without the book the addresses are lost and the keys remain.
- A stop of the process between the write of the book and the deletion of a key leaves a key file without an entry. The key file names the address that it was created for (ADR 0711): the client uses it again when the same server is added at that address, and at no other address.
- A client that holds an older view of the book gets "not saved" for a server that another client removed and added again, until it starts again.
