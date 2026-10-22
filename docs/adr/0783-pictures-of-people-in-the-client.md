# 0783. Pictures of people in the client: profiles, avatar images and the picture of the preset

## Status

Accepted

## Context

An account can set an image as its avatar (ADR 0119). The window drew the generated avatar for every person (ADR 0731). The description of an avatar is part of the profile of an account and of the own account in the state of a server; a member list and the people of a call carry the account and the name only. The image is read from a route that needs a session, and the server checked its header and not its data: the client treats it as a file of an unknown sender, reads the format from the bytes and computes the hash itself (ADR 0714). The window read the name of an account outside the member list from the profile route, with a rule for a request without an answer (ADR 0730). The derivation of the view is a function without side effects (ADR 0730). The profile preset holds a picture that a new account starts with, and the server book records per server whether a server refused it (ADR 0717).

## Decision

Cache of profiles:

- The state of the application holds, per server, the profile of each account that the window shows: the name and the hash of the avatar. The cache of the names of ADR 0730 is this cache.
- The derivation names the accounts of the view in the order in which the window shows them: the people in voice channels, the people who share, the authors of the shown messages, of the messages that they answer and of the last reply of a thread, the people of the microphone lines that the card of a recording shows, the authors of the notices and of the search results in the side panel, the people of the microphone lines of the transcript in the side panel, the direct conversations, and then the member list. The state of the application adds the accounts of an open page of the administration. The derivation starts no request.
- A profile is read from `GET /api/v1/accounts/{account_id}/profile`. At most 4 requests of one server are on their way at one time, for profiles and images together, and the people who are named first are read first.
- A server has at most 512 profiles. Only the first 512 accounts of the list get a profile. When a new account needs a place, the profile that was shown longest ago gives way. A profile that is shown now, or whose request is on its way, does not give way: a list of more than 512 people cannot cause requests without an end.
- A refusal is final. A request that got no answer, or that the server limited, is sent again with a later change of the state of the application, at most three times for one account.
- The event `account_profile_updated` replaces the profile of an account that the cache holds. An answer that arrives after such an event is not used.
- When the state of a server was replaced by a snapshot, or the reader missed events, every profile of that server is read again when it is shown. What was read before is shown until the answer arrives. The answer to a request that started before that moment is not used, because it can be older than the new state: the profile is asked for again. The earlier request keeps its place among the 4 requests until it ends.
- The cache of a server belongs to the account of its session. It is dropped when the session belongs to another account and when the server is removed. An answer to a request of the account before is not used.
- Each request has a number that no other request has while the application runs, for any server and any account. A cache holds the numbers of its requests that are on their way, and takes an answer only for one of them. An entry takes an answer only for the number of its newest request. Reason: a server that is removed and added again has a new cache, and a count that starts again with a cache would give a late answer the number of a new request.
- The own account is not read from the profile route. Its name and its avatar are those of the state of the server.
- A name from a profile is cut to 64 characters without control characters. A hash that is not 64 hexadecimal digits in lower case is no avatar.

Images:

- An image is held by its hash. It is read from `GET /api/v1/accounts/{account_id}/avatar` of an account whose profile names that hash, once for the hash, also when several accounts have the same image.
- The answer is decoded on a background thread. The decoder refuses an image with a side of more than 1024 pixels, whatever the server states, and has a bound for its memory. The middle square of the image is kept and reduced to at most 192 by 192 pixels: the largest avatar of the window has 153 pixels of a display.
- Bytes whose computed hash differs from the hash of the profile, bytes that cannot be decoded, and a refusal are final for that hash: the image is not asked for again. The generated avatar is drawn.
- A server has at most 256 decoded images, which are at most 36 MiB of pixels. The image that was shown longest ago gives way, by the rule of the profiles, and its texture is removed from the window.

View and component:

- The view carries the pictures as `faces`: per server, for each account whose profile names an avatar, the hash and the image when the window holds it. A drawing takes the pictures of its server once and asks for each person by the account. The rows of the view carry no picture of their own.
- One component, `Avatar`, draws the image when it is given one, and the generated avatar otherwise, at the same size and with the same corners. It is used wherever the window draws a person: the people of a voice channel, the tiles and the member list of a call, the members panel, the author of a message, of a quoted message and of the last reply of a thread, the mention list, the direct conversations, the profile strip, the menu of a person, the meeting bar, the shares, and the lists of accounts and members of the administration, the notices, the search results, and the microphone lines of a transcript in the card of a recording and in the side panel.
- The image fills the square of the avatar; a part outside the square is not drawn.

Picture of the profile preset:

- The settings show the picture of the preset with "Choose a picture…" and "Remove picture", and state the bounds: PNG, JPEG or WebP, at most 256 KiB, and that a server takes at most 1024 by 1024 pixels.
- The chosen file is read, checked and stored on the queue of the writes (ADR 0730). Only a regular file is read, and at most 256 KiB and one byte of it. One function of the client core reads a chosen file, for the preset and for the profile page of the account layer (ADR 0781). It opens the path without following a link at its end and without waiting for the other side of a pipe or a device, and then reads the kind from the opened file, so the file cannot be exchanged between the check and the read. A system without these flags of an open keeps the check of the opened file. The check is the one of the preset (ADR 0717): the size in bytes and the format of the first bytes. Each refusal has its sentence.
- The settings decode the stored picture with a bound of 4096 pixels per side. A stored picture that cannot be decoded stays stored, and the settings say that the window cannot draw it.
- The profile strip shows the picture of the preset for a person without an account on the selected server.
- For each server whose state of the preset is `refused`, the settings name the server in one line.
- The file dialog is opened by one function, and a second function takes the chosen path. A test calls the second one.

## Consequences

- A person sees the avatar of another account after one request for the profile and one for the image. A workspace with 200 members costs up to 200 profile requests when its member list is shown, four at a time.
- The member list and the people of a call would need no profile request if they carried the description of the avatar. That is a request for the server: the member in the answer of `GET /api/v1/workspaces/{workspace_id}/members` and in the event `workspace_member_added`, and the person of a call in the snapshot and in the events `call_participant_joined` and `call_participant_updated`, each carry the `avatar` of the profile of ADR 0119. The event `account_profile_updated` tells a later change, as it does now.
- An account that the session may not see, such as the author of an old message who left the workspace, has the generated avatar and its name from the message.
- More than 512 people that are shown at one time, or more than 256 different images, leave the later ones with the generated avatar.
- A server cannot make the window hold more than 512 profiles and 256 images of 192 pixels, cannot keep more than 4 requests on their way, and cannot have an image drawn whose bytes do not have the hash that the profile names.
- A state that is replaced while requests for profiles are on their way costs those requests a second time, at most 4 for one replaced state. The notice of a new state reaches the cache after the state itself, so this holds also for the requests that the first drawing of the new state started.
- An account whose requests get no answer is asked for three times after each replaced state.
- An avatar that an account changes twice in a short time is read twice. An image whose bytes changed between the profile and the read is not drawn until the event with the new hash arrives.
- A picture of the preset with more than 1024 pixels per side is stored and shown, and each server that the device joins afterwards refuses it. The settings name those servers.
