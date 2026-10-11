# 0711. Device key file and its protection on each system

## Status

Accepted

## Context

ADR 0103 lists a place for the device key per system: the system key store on macOS with a key file as the fallback, the secret service of the desktop on Linux with a key file otherwise, and the Windows credential store. Step 0 (`spikes/macos-identity/RESULT.md`) found that on macOS an updated build cannot read the Keychain item of the build before it without a prompt for the login keychain password, with either way of signing, so macOS uses the key file. A secret service does not exist on many Linux setups, so Linux needs the key file in any case. The records do not state the format of the key file, what "only the user" means on each system, or what the client does with a key file that others can reach.

A key file that is named only by a server identifier can be used at any address that reports that identifier. A server identifier is public. After a book was moved aside, or a removal stopped between its two steps, a key has no book entry, and an address that copies the identifier of the real server could relay its challenges to this client and get a signature for the real server.

## Decision

One key store for the first version:

- The first version stores device keys in key files on all three systems. This replaces the choice per system of ADR 0103 for the first version.
- The reasons: one code path that is tested on every system; on macOS the system key store ties access to the identity of the application and fails after an update; a secret service is absent on many Linux setups, so the key file must exist there anyway.
- The client reaches device keys only through one interface with four operations: load the key of a server for an origin, read the origin of the key of a server, store the key of a server that has none, and delete the key of a server. A store for the secret service of a Linux desktop or for the Windows credential store is a second implementation of this interface and needs no change to the flows.
- A stored key is never replaced. Storing a key for a server that has one fails.
- A key is generated from the random source of the operating system and stored before its public key is sent to a server.

A key belongs to one origin:

- A key is created for one server identifier and one origin: the address of the server as the client verified it when the key was created (ADR 0714). The origin is stored with the key, in the key file, and not only in the server book.
- The store gives a key out only for the origin that it was created for. A request for another origin gets the error "other origin" and no key, whatever the server book says.
- The flow that adds a server reads the origin of a stored key of the reported identifier before it saves the entry. When the origin is another address, the server is not added: the flow shows the state "key of another address" with that address. The only action that it offers there is to delete the stored key. After that the server can be added and gets a new key, and the device is a new device for the server.
- The client offers no action that moves a key to another address. A server that changes its address is reached again through a device link or a recovery.

Key file:

- The path is `keys/<server identifier>.key` in the data directory (ADR 0710).
- The content is: the text `junto-device-key-v1` and a line feed, the 32 secret bytes of the Ed25519 key, the 32 bytes of its public key, the length of the origin as two bytes, and the origin as text of at most 300 bytes.
- At a load the client derives the public key from the secret bytes and compares it with the stored one, and reads the origin as a server address. A file of another length, with another first line, with a public key that does not match, or with an origin that is not a server address in its stored form is reported as damaged and is not used.
- A damaged or missing key is never replaced by a new key without an action of the user. A new key would look to the server like an unknown device.
- A key file is created in one step under its final name and never replaces a file: the content is written to a temporary file, flushed, and given the final name only when that name is free.

Opened objects, not paths:

- The client opens the data directory once, without following a link at its path, and keeps it open. It opens the key directory relative to the opened data directory, again without following a link, and keeps it open.
- Each check is made on the opened object. The data directory and the key directory must be directories, belong to the user of the process, and give nobody else access. A key file must be a regular file, belong to the user, and give nobody else access.
- Every file of the data directory is opened relative to an opened directory and without following a link. A link in place of a file is refused, also when it appears between two operations.
- The key directory is checked again before each read and each creation of a key, because its mode or its access list can change while it is open.
- A file or directory that fails a check is refused with an error that names the path. The client does not repair it.

Unix (macOS and Linux):

- The directories have mode 0700 and the files mode 0600, set at creation.
- "Belongs to the user" is the owner of the opened object equal to the effective user of the process. "Nobody else" is no permission bit for the group or for others.
- The opens use the directory descriptor and the flag that refuses a link.

Windows:

- The data directory and the key directory are created with the user of the process as owner and a protected access list that has one entry: full access for that user, inherited by the files and directories inside. A protected list takes nothing from the parent directory.
- A directory is opened without following a reparse point and is held open with a sharing mode that excludes deletion and renaming, so its path cannot be given to another object while the client runs. A file is opened inside such a directory without following a reparse point.
- The owner and the access list are read from the opened handle, not from the path. The owner must be the user of the process, the administrators group or the system account; an owner can always change the access list, and the latter two can take any file anyway. Each entry of the access list of a private object must allow access to the user of the process; a list with any other entry, or no list, is refused. A reparse point is refused.
- This code calls the Windows API through the `windows-sys` crate. It is the only unsafe code of the library: the crate denies unsafe code and allows it in this one module, which is compiled on Windows only. The command-line tool has one more such module, which turns the echo of the console off (ADR 0715).
- **No test has executed the Windows code yet.** It is compiled and type-checked for the Windows target, and the rule that decides from an owner and an access list is tested on the other systems as a function without system calls. The system calls, the creation of the access list and the behaviour of held directory handles are untested until a Windows system runs the tests (ADR 0902).

Memory and logs:

- The type that holds a key overwrites the secret bytes when it is dropped. The buffers that carry the bytes between the file and the key have a fixed size and are overwritten too.
- The key type has no formatting that shows the secret bytes. Its debug output shows the public key.
- The only operation that signs takes a challenge that the client has compared with its request (ADR 0714). There is no public operation that signs arbitrary bytes.
- An error of the key store names the server identifier, the path and the stored origin, never the secret content.

## Consequences

- The key file has the protection of an SSH private key without a passphrase: another user account cannot read it, and a program that runs as the same user can.
- A key file is not tied to the identity of the application, so an update of the client reads it without a prompt.
- A key without a book entry is used again only at the address that it was created for. An address that copies the identifier of a saved or formerly saved server gets no signature.
- A server that moves to another address loses the devices that are not linked or recovered again. This is the price of never moving a key between addresses.
- A backup of the data directory contains the keys.
- A restore that changes the modes or the owner makes the client refuse the files until the user corrects them. The error names the path.
- A data directory that is a link to another place is refused. A user who wants the data elsewhere sets `JUNTO_DATA_DIR`.
- On Windows a member of the administrators group and the system account can read a key, as root can on Unix.
- ADR 0103 names other stores per system. For the first version this record applies, and the other stores can be added behind the interface.
