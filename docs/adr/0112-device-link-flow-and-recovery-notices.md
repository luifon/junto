# 0112. Device link flow, device removal and recovery notices

## Status

Accepted

## Context

ADR 0104 states that a second device is added with a short-lived one-time code, that the approval names the public key of the new device, and that the account sees each recovery. It does not state which device creates the code, how the server stores the notice of a recovery, or whether the only device of an account can be removed.

## Decision

Link flow on one server:

1. The new device proves that it holds its key with a `link_device` challenge (ADR 0108) and sends a device name. The server stores the public key and returns a link code (ADR 0110) that lives 10 minutes.
2. The user types the code into a device that is logged in. That device asks the server for the public key and the device name behind the code and shows them.
3. The logged-in device approves with the code and the public key. The server adds the device to the account of the approving session only when the public key equals the stored one.

- A new request of the same key replaces its earlier code.
- The server holds at most 500 waiting device links and refuses further ones with the error `device_links_full`.
- An approval with another public key gets the error `public_key_mismatch` and leaves the code usable.
- An approval uses up the code.
- A key that already belongs to an account cannot ask for a link.

Devices:

- Every device has a name that its account can change. The name can be empty; a device that an approval of a join request creates starts with an empty name, because a join request carries a name only (ADR 0105).
- Each device records how its key came to the account: owner code, invite, join request, link or recovery.
- An account can remove any of its devices, the one in use included, but not its last device.

Recovery:

- The server owner sends the account and the public key of the new device. The server adds the key as a device of the account.
- The recovery deletes every session of the account in the same transaction. A copied session token stops working at the recovery.
- The other devices of the account keep their keys. They log in again and then receive the notice.
- The server stores one row per recovery, and one notice row for each device that the account has at that moment, the new device included.
- The response to a login carries the recoveries whose notice the device has not acknowledged. A login removes no notice. The device reads its notices and acknowledges each one after it has shown it (ADR 0118). The account can also list all of its recoveries.

## Consequences

- A device link and a link code that waited too long are deleted by the maintenance task (ADR 0009).
- A link code alone gives no access. A person who creates a link request needs a logged-in user who types that code and approves that key.
- The approving client must show the public key or compare it for the user; this is the step that names the key.
- Linking all servers of a client (ADR 0104) repeats this flow once per server. The client carries the codes between the two devices.
- An account always has a device that can log in, unless the owner removes or bans the account.
- Each device is told about a recovery at every login until it acknowledges the notice, whatever the time of that login. Every device has to log in again after a recovery, so the notice reaches it then.
- A recovery interrupts the work on the other devices of the account until their clients have logged in again.
- The owner chooses the public key of a recovery, so the person must give the owner the public key of the new device over another channel.
