# Spike: macOS application identity across updates

Step 0, test 4 (ADR 0103, ADR 0704). Run on 2026-10-09, macOS 26.6.1,
Apple Silicon, rustc 1.98.1, OpenSSL 3.6.4.

## Question

Junto is distributed without a paid Apple Developer ID and updates itself by
replacing its application bundle. Hypothesis: signing every build with the
project's own self-signed code-signing certificate gives a stable identity, so
that after an update the Keychain item is still readable without a prompt and
the Microphone and Screen Recording permissions remain granted.

## Result

The hypothesis holds for the permissions and does not hold for the Keychain.

Variant A is ad-hoc signed (`codesign --sign -`). Variant B is signed with the
self-signed certificate. PASS means the wanted property was observed.

| Gate | A: ad-hoc | B: self-signed certificate |
|---|---|---|
| I1 designated requirement stable v1→v2→v3 | FAIL: it is the code hash of each build | PASS: bundle identifier plus hash of the certificate |
| I2 Keychain item of v1 readable by v2 without a prompt | FAIL: prompt, needs the user and the login keychain password | FAIL: prompt, needs the user and the login keychain password |
| I3 Microphone still granted after the update | FAIL: status back to "not determined", new prompt | PASS: authorized in v2 and v3, no prompt |
| I4 Screen & System Audio Recording still granted after the update | FAIL: preflight false in v2 and v3, new prompt | PASS: preflight true in v2 and v3, no prompt |
| I5 Data Protection keychain without a provisioning profile | FAIL: OSStatus -34018 | FAIL: OSStatus -34018 |
| I6 Key file with mode 0600 readable by v2 and v3 | PASS | PASS |
| I7 First launch of a quarantined copy | FAIL: launch blocked, user must allow it in System Settings | FAIL: the same dialog and the same steps |
| I8 Certificate needed on the user's machine | not applicable | PASS: not needed. I1, I3 and I4 held with the certificate absent from the machine |

Every row is observed. No step is left for a person: on this machine the
Screen Recording switch changed without a password or Touch ID.

## Conclusion for the product

1. **Device key: store it in a key file, not in the Keychain.** Use a file
   under the user's Application Support directory with mode 0600 in a
   directory with mode 0700 (the fallback of ADR 0103). Neither signing variant
   lets an updated build read the Keychain item of the previous build without a
   prompt that asks for the login keychain password. The Data Protection
   keychain refused both variants for a missing entitlement. That entitlement
   has to be authorized by a provisioning profile, which needs a paid Apple
   developer account (not verified here).
2. **Builds: sign every build with the project's self-signed code-signing
   certificate.** It keeps the Microphone and the Screen & System Audio
   Recording permissions across updates. An ad-hoc signed build loses both at
   every update.
3. **The identity is the pair (bundle identifier, certificate).** macOS stores
   `identifier "<bundle id>" and certificate leaf = H"<SHA-1 of the
   certificate>"` with each permission. A different bundle identifier or a
   different certificate (a new certificate for the same key included) is a
   different application for macOS, and the user is asked again. The
   certificate and the bundle identifier must stay the same for the life of the
   product.
4. **The certificate must be valid when a build is signed.** `codesign`
   refused a certificate whose validity period had ended ("no identity
   found"). Issue the certificate with a long validity period. What happens to
   an already installed build after its certificate expires is not verified.
5. **The certificate's private key decides who can receive the permissions.**
   A program signed with that key and the same bundle identifier satisfies the
   stored requirement. The key needs the same care as the update signing keys
   of ADR 0704.
6. **The user's machine needs nothing installed.** The certificate does not
   have to be present or trusted there.
7. **The first launch of a downloaded copy is blocked for both variants** and
   needs the steps in I7. The install instructions of ADR 0704 stay as they
   are. The self-signed certificate does not change this.

## Method

- `src/main.rs` is a Rust program with one mode per run. It prints one JSON
  line and writes the same line to the file given with `--out`. Modes:
  `version`, `keychain-store`, `keychain-read`, `keychain-read-quiet` (read
  with user interaction disabled), `dp-store`, `dp-read` (Data Protection
  keychain), `mic-status`, `mic-request`, `screen-status`
  (`CGPreflightScreenCaptureAccess`), `screen-request`
  (`CGRequestScreenCaptureAccess`), `file-store`, `file-read`.
  Crates: `security-framework` 3.7 (MIT or Apache-2.0), `objc2`,
  `objc2-foundation`, `objc2-av-foundation`, `block2` (MIT).
- `spike.sh` builds the binary three times with `SPIKE_VERSION` 1.0.0, 2.0.0
  and 3.0.0 (three different binaries, SHA-256 differs), assembles
  `JuntoSpikeAdhoc.app` (`junto.spike.identity.adhoc`) and
  `JuntoSpikeSelfSigned.app` (`junto.spike.identity.selfsigned`) for each
  version, and signs them. The bundles have `LSUIElement` set, a
  `NSMicrophoneUsageDescription` and a `NSAudioCaptureUsageDescription`. No
  hardened runtime, no entitlements.
- Install path: `~/Applications/JuntoSpike/<name>.app`. An update copies the
  new bundle next to the installed one, renames the installed one away,
  renames the new one into the path and deletes the old one.
- Every run starts the installed bundle with `open -n <bundle> --args <mode>
  --out <file>`, never the bare binary.
- Attribution (observed): each result line reports the bundle identifier read
  from `NSBundle`, the executable path inside the installed bundle and parent
  process 1. The prompts name the bundle (`“JuntoSpikeAdhoc” would like to
  access the Microphone.`). The permission records have the bundle identifier
  as client. For comparison, the bare binary started from the terminal reported
  `screen_recording: true`, which is the terminal's permission, while the
  bundle reported `false` at that time.
- The screen was read and clicked with a desktop automation tool. Its command
  is read from the environment variable `JUNTO_TEST_AUTOMATION_CLI`.
  `click_prompt.py` reads a prompt and clicks one button only when the prompt
  contains the expected text. The tool reported an error for most of these
  clicks because the prompt closed before the tool could read it back; the
  effect of each click was checked from the program's result. No password was
  typed. The microphone was not opened for capture.
- Permission records were read from the two TCC databases with `sqlite3`
  (`./spike.sh tcc`); the stored requirement was decoded with `csreq -r- -t`.
- `item_acl.swift` prints the access entries and the partition list of one
  test item. It reads no secret.

Order of the run:

1. Build, create the signing identity, sign the six bundles.
2. Variant A: v1 (store the secret, store the key file, grant Microphone,
   grant Screen Recording), update to v2, check, update to v3, check.
3. Variant B: v1 with the temporary keychain in the search list (same steps
   as A). Remove the temporary keychain from the search list. Update to v2,
   check. Put the keychain back for one Keychain read, remove it again. Delete
   the temporary keychain and the key material. Update to v3, check.
4. Variant B again with no certificate on the machine at any step: reset the
   Microphone record, install v1, store a second Keychain item, grant
   Microphone, update to v2, check.
5. Quarantined copies of both variants.
6. Extra: try to sign with a certificate that expired in 2021.
7. Clean up.

## Signing commands

Placeholders: `<keychain>` is the path of a new keychain file, `<password>` a
password chosen for it, `<name>` the certificate's common name, `<login>` the
path of the login keychain, `<app>` the bundle.

```sh
openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
    -keyout key.pem -out certificate.pem -subj "/CN=<name>" \
    -addext "basicConstraints=critical,CA:FALSE" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning"

security create-keychain -p "<password>" "<keychain>"
security set-keychain-settings "<keychain>"
security unlock-keychain -p "<password>" "<keychain>"
security import key.pem -k "<keychain>" -T /usr/bin/codesign
security import certificate.pem -k "<keychain>"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "<password>" "<keychain>"
security list-keychains -d user -s "<login>" "<keychain>"

codesign --force --sign - "<app>"                                                   # variant A
codesign --force --timestamp=none --keychain "<keychain>" --sign "<name>" "<app>"   # variant B

security list-keychains -d user -s "<login>"
security delete-keychain "<keychain>"
```

Observed: none of these commands showed a prompt. The certificate was not
imported into the login or System keychain and was not marked as trusted.
`security find-identity -p codesigning <keychain>` listed the identity as
`(CSSMERR_TP_NOT_TRUSTED)` with `0 valid identities found`, and `codesign`
signed with it. `codesign --verify --strict` reported `valid on disk` and
`satisfies its Designated Requirement` for all six bundles.

## Evidence

### I1 Designated requirement

`codesign -d -r- <app>` (observed):

```
adhoc v1       # designated => cdhash H"362ab5835d03a30b7b9eb6df686ebbe4ed96609d"
adhoc v2       # designated => cdhash H"392b9f3114e7bce5b5e78d1e5f6cf324fc0e4c02"
adhoc v3       # designated => cdhash H"1c491b9629ebfd773d9452ef6f4b4ce79d9fa78a"
selfsigned v1  designated => identifier "junto.spike.identity.selfsigned" and certificate leaf = H"fb3af1b3b6a37c02c119fdc98d0c0dc96b2a93d6"
selfsigned v2  designated => identifier "junto.spike.identity.selfsigned" and certificate leaf = H"fb3af1b3b6a37c02c119fdc98d0c0dc96b2a93d6"
selfsigned v3  designated => identifier "junto.spike.identity.selfsigned" and certificate leaf = H"fb3af1b3b6a37c02c119fdc98d0c0dc96b2a93d6"
```

- A: the requirement is the hash of the build's code directory
  (`Signature=adhoc`, `flags=0x2(adhoc)`). It changes with every build.
- B: the requirement is the bundle identifier and the SHA-1 hash of the
  signing certificate (`Authority=Junto Spike Code Signing`,
  `TeamIdentifier=not set`). `fb3af1b3…` is the SHA-1 fingerprint that
  `openssl x509 -fingerprint -sha1` printed for the certificate. The code
  hashes of the three builds differ (`9557cdb3…`, `839f6bd0…`, `53e6c67f…`).

### I2 Keychain (login keychain, generic-password item created by v1)

v1 stored and read its item without a prompt in both variants (`os_status 0`).
`security find-generic-password` confirmed that both items were in the login
keychain.

A, after the update (observed in v2 and in v3):

- `keychain-read-quiet`: `os_status -25293` (`errSecAuthFailed`).
- `keychain-read`: a prompt. Text: `JuntoSpikeAdhoc wants to use your
  confidential information stored in “junto.spike.identity.adhoc” in your
  keychain.` Second line: `To allow this, enter the “login” keychain
  password.` A `Password:` field. Buttons: `?` (Help), `Always Allow`, `Deny`,
  `Allow`. It demands the login keychain password. After `Deny`:
  `os_status -128` (`User canceled the operation.`). Result: prompt, needs the
  user.

B, after the update (observed in v2 with the keychain out of the search list,
in v2 with it back in the search list, and in v3 with it deleted):

- `keychain-read-quiet`: `os_status -25293` in all three states.
- `keychain-read` (v2): a prompt with a different first line:
  `JuntoSpikeSelfSigned wants to access key “junto.spike.identity.selfsigned”
  in your keychain.` Second line: `To allow this, enter the “login” keychain
  password.` A `Password:` field. Buttons: `?` (Help), `Always Allow`, `Deny`,
  `Allow`. After `Deny`: `os_status -128`. Result: prompt, needs the user.
- Putting v1 back at the install path made the item readable again without a
  prompt (`os_status 0`).

Cause (observed with `item_acl.swift`): each item has two checks.

```
junto.spike.identity.selfsigned
entry: ACLAuthorizationDecrypt, …   application: ~/Applications/JuntoSpike/JuntoSpikeSelfSigned.app
entry: ACLAuthorizationPartitionID  partitions: ["cdhash:9557cdb3d96df57e9e5bca076db3daad26688fae"]

junto.spike.identity.adhoc
entry: ACLAuthorizationDecrypt, …   application: ~/Applications/JuntoSpike/JuntoSpikeAdhoc.app
entry: ACLAuthorizationPartitionID  partitions: ["cdhash:362ab5835d03a30b7b9eb6df686ebbe4ed96609d"]
```

The partition list of both items holds the code hash of v1 (observed), so an
updated build is in another partition whatever its designated requirement is.
The two prompt texts fit this: in B the application entry accepts v2 (stable
requirement) and the partition check refuses it ("wants to access key"); in A
the application entry already refuses v2 ("wants to use your confidential
information"). The rule behind it, that macOS assigns the partition
`teamid:<Team ID>` only to signatures with an Apple-issued Team ID and
`cdhash:<code hash>` to all others, comes from the published Security
framework source and was not tested beyond these two items.

### I3 Microphone

A (observed): v1 `not_determined` → prompt `“JuntoSpikeAdhoc” would like to
access the Microphone.` with the usage text, buttons `Don’t Allow` and `Allow`
→ `Allow` → `authorized`. Record:
`kTCCServiceMicrophone junto.spike.identity.adhoc auth_value=2 requirement:
cdhash H"362ab583…"` (the code hash of v1). After the update to v2:
`not_determined`, and `mic-request` showed the prompt again. After `Allow` the
record held the code hash of v2. After the update to v3: `not_determined`
again.

B (observed): v1 `not_determined` → the same prompt for
`“JuntoSpikeSelfSigned”` → `Allow` → `authorized`. Record:
`kTCCServiceMicrophone junto.spike.identity.selfsigned auth_value=2
requirement: identifier "junto.spike.identity.selfsigned" and certificate leaf
= H"fb3af1b3…"`. v2: `authorized`. v3: `authorized`, and `mic-request`
returned `granted` in 15 ms with no prompt.

### I4 Screen & System Audio Recording

First grant, both variants (observed): `screen-request` showed a window titled
`Screen Recording` with the text `“<name>” would like to record this
computer’s screen and audio.` and `Grant access to this application in Privacy
& Security settings, located in System Settings.` Buttons: `Open System
Settings`, `Deny`. `Open System Settings` opened the Screen & System Audio
Recording list with a row for the bundle, switch off. Clicking the switch
turned it on. macOS asked for no password and no Touch ID. The account is an
administrator account. Whether a standard account is asked is not verified.

A (observed): v1 preflight `true` after the switch. Record in the system
database: `kTCCServiceScreenCapture junto.spike.identity.adhoc auth_value=2
requirement: cdhash H"362ab583…"`. v2: preflight `false`. v3: preflight
`false`, and `screen-request` showed the prompt again. The row in System
Settings still showed its switch on and the record was unchanged, while the
program was refused.

B (observed): record `kTCCServiceScreenCapture
junto.spike.identity.selfsigned auth_value=2 requirement: identifier
"junto.spike.identity.selfsigned" and certificate leaf = H"fb3af1b3…"`. v2:
preflight `true`. v3: preflight `true`, and `screen-request` returned `true`
in 8 ms with no prompt.

Only the preflight and request calls were used. No screen or audio capture was
started.

### I5 Data Protection keychain

`dp-store` (`SecItemAdd` with `kSecUseDataProtectionKeychain`), both variants,
v1 (observed): `os_status -34018`, message `A required entitlement isn't
present.` (`errSecMissingEntitlement`). `dp-read` then returned `-25300` (item
not found).

### I6 Key file

`file-store` wrote `~/Library/Application Support/<bundle id>/device.key` with
mode 600 in a directory with mode 700. `file-read` in v1, v2 and v3 of both
variants returned the stored value, `file_mode 600`, `directory_mode 700`,
owner equal to the current user (observed).

- What protects it: the file permissions. Another user account on the machine
  cannot read it. When FileVault is on, the disk encryption protects it while
  the machine is off.
- What does not protect it: any program that runs as the same user can read
  it. Observed: `/bin/cat` from the terminal printed the content. The file is
  not tied to the application's identity, there is no prompt, and it is
  included in backups of the user's Library unless excluded. This is the
  protection an SSH private key without a passphrase has.

### I7 First launch of a quarantined copy

A copy of v1 of each variant got
`com.apple.quarantine = 0083;<hex time>;Safari;<UUID>` on every file
(`xattr -r -w`) and was opened with `open`.

Observed, identical for A and B apart from the name:

- `spctl --assess --type execute`: `rejected`.
- A dialog. Title: `“JuntoSpikeAdhoc” Not Opened`. Text: `Apple could not
  verify “JuntoSpikeAdhoc” is free of malware that may harm your Mac or
  compromise your privacy.` Buttons: `?` (Help), `Move to Trash` (the default
  button), `Done`. There is no button that opens the application.
- After `Done` the program had not run (no result file) and the bundle was
  still in place.
- System Settings › Privacy & Security › Security then showed `“JuntoSpikeAdhoc”
  was blocked to protect your Mac.` with a button `Open Anyway`, under `Allow
  applications from: App Store & Known Developers`.

User steps that allow the launch: open the application once, click `Done`,
open System Settings › Privacy & Security, scroll to Security, click `Open
Anyway`, confirm. `Open Anyway` was not clicked. The confirmation and the
authentication that follow it are not verified.

The installed copies had no quarantine attribute and started with no dialog
(observed in every run above). Whether a bundle that the client downloads
itself gets the attribute depends on how the client writes it; this was not
tested.

### I8 Certificate on the user's machine

From the requirement: `certificate leaf = H"…"` compares the hash of the
certificate embedded in the signature. It contains no `anchor` condition, so
no trust evaluation and no lookup in a keychain is part of it.

Observed:

- With the temporary keychain removed from the search list
  (`security find-certificate -a -c "<name>"` found nothing), v2 passed
  `codesign --verify --strict` (`satisfies its Designated Requirement`),
  Microphone was `authorized` and the Screen Recording preflight was `true`.
- With the temporary keychain and the key material deleted, the same for v3.
- With no certificate on the machine at any step (Microphone record reset, v1
  installed again, Microphone granted, update to v2): Microphone `authorized`
  in v2, Screen Recording preflight `true`.
- The Keychain read failed in every state, with the certificate present and
  absent, so the failure of I2 does not depend on the certificate.

### Extra: expired certificate

A certificate valid from 2020-01-01 to 2021-01-01, in a second temporary
keychain (observed): `security find-identity` listed it as
`(CSSMERR_TP_CERT_EXPIRED)`; `codesign --sign` by name and by SHA-1 hash
answered `no identity found`. The second keychain was deleted.

## Not verified

- Real capture after an update (microphone samples, screen frames, system
  audio). Only the authorization status and the preflight were read.
- A build that was signed while the certificate was valid, run after the
  certificate has expired. It needs a changed system clock.
- The Screen Recording switch on a standard (non-administrator) account.
- `Open Anyway`, its confirmation and its authentication (I7).
- What `Always Allow` plus the password does to the item, and how long it
  holds. No password was typed.
- A Keychain item created with an access list that allows every application.
  It would give no more protection against other programs of the same user
  than the key file.
- A build with the hardened runtime. It needs the
  `com.apple.security.device.audio-input` entitlement for the microphone.
- A changed install path, a renamed bundle, and macOS versions other than
  26.6.1.
- Whether marking the certificate as trusted changes I2. It was not allowed
  in this test. The partition is derived from the Team ID, which a
  certificate that Apple did not issue does not have.

## Clean-up

Done and checked (observed):

- Keychain test items `junto.spike.identity.adhoc`,
  `junto.spike.identity.selfsigned` and `junto.spike.identity.selfsigned.cold`
  deleted from the login keychain; `security find-generic-password` finds
  none.
- Both temporary keychains deleted with `security delete-keychain`, key
  material deleted. The user search list is the login keychain only, as before
  the test. The default keychain is unchanged.
- `~/Applications/JuntoSpike/` deleted, including the quarantined copies.
  `lsregister -u` was run for each bundle first.
- `~/Library/Application Support/junto.spike.identity.adhoc` and
  `…selfsigned` deleted.
- `tccutil reset All` for both bundle identifiers. Both TCC databases have no
  row for them, and the Microphone and Screen & System Audio Recording lists
  in System Settings show no row for them.
- `target/` and `out/` (binaries, bundles, result files, screenshots) deleted.

What remains on the machine:

- System Settings › Privacy & Security › Security still shows
  `“JuntoSpikeSelfSigned” was blocked to protect your Mac.` with `Open
  Anyway`. The bundle it refers to is deleted. macOS removes this line by
  itself after some time (not verified).
- System Settings is open on a Privacy & Security page. It was already
  running before the test; the page it showed before is not known.
- Records that macOS keeps and that have no user control: unified log lines
  and LaunchServices and Gatekeeper history for the deleted bundles.
- Screenshots of the prompts and of the System Settings window in the
  temporary directory of the automation tool. The tool marks them to expire
  after 24 hours.

Left in the spike directory on purpose: `src/`, `build.rs`, `Cargo.toml`,
`Cargo.lock`, `spike.sh`, `click_prompt.py`, `click_switch.py`,
`item_acl.swift` and this file, so that the test can be repeated.
