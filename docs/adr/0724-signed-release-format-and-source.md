# 0724. Signed release format and source

## Status

Accepted

## Context

ADR 0704 requires one source, authenticated releases and replacement of signing keys. Release authorization and periodic freshness have different custody requirements. Format 1 is settled before the first release.

## Decision

- `JUNTO_UPDATE_SOURCE` is one build-time HTTPS base address; HTTP is allowed only for literal loopback addresses. Credentials, queries, fragments, whitespace and control characters are refused by shared build/runtime validation. Without a source or both trust sets, no update request is made. The existing startup and six-hour scheduler applies.
- Release public keys are in `packaging/update-keys.json`; freshness public keys are in `packaging/freshness-keys.json`. Each is a reviewed array of up to 32 base64 Ed25519 keys, initially empty. Release keys authorize code and remain offline between releases. Freshness keys are used before each 30-day expiry, without using release keys. The sets and their private keys are distinct in production.
- `latest-<system>-<architecture>.json` serves format 1 permanently. A later incompatible format uses a different filename; changing this file's format cannot migrate existing clients. Optional unknown fields are ignored; duplicate known fields are refused. Every used field is inside authenticated bytes.
- The envelope contains base64 `payload`, named `signatures`, and `freshness`. Release signatures cover exactly the decoded payload bytes, prefixed by `junto-release-metadata-v1` and a zero byte. Parse only those verified bytes. The release payload holds `format` (1), `version`, `system`, `architecture`, safe `archive` filename, `size`, lower-case hexadecimal `sha256` and named `archive_signatures`. Archive signatures cover `junto-release-archive-sha256-v1`, a zero byte and the binary digest. These signatures do not expire.
- `freshness` contains its own base64 payload and named signatures. Its payload holds `format` (1), `version`, `release_sha256` (the digest of the exact release payload), and `expires` (Unix seconds). Freshness signatures cover `junto-release-freshness-v1`, a zero byte and those exact bytes. Verify with the freshness trust set, then require the stated version and digest to equal the authenticated release information. Missing or expired freshness is refused before even an equal release is described as current. The message mentions freshness and the system clock rather than assigning fault to the source.
- At least one valid signature from the appropriate trusted set authenticates each object. Each list has at most 32 entries. Sign release information with every release key still needed by supported clients; refresh can likewise carry several freshness signatures. Either role can rotate independently. Shipping an overlap build alone does not reach clients that skipped that build.
- `refresh METADATA ARCHIVE VERSION FRESHNESS-SECRET...` first verifies release authorization against trusted release keys, the operator-named version, and the existing archive's size, digest and signature. Only then does it replace the freshness statement, with signatures from trusted freshness keys. It preserves the exact release payload and signatures, writes a new private file and renames it over the old metadata. Operators refresh their retained release files, never files fetched back from the release host.
- A release key alone can authorize arbitrary code, but cannot supply valid freshness for changed release information. A freshness key alone can extend or select an authentic release, but cannot authorize changed release bytes, a different target or unsigned code; clients still refuse versions not newer than themselves. Holding both roles authorizes a new release, subject on macOS to the permanent certificate check. A source alone can withhold files, replay still-fresh authentic releases, or cause verification failures; it cannot change authenticated code or extend freshness. No mechanism proves that a still-fresh release is the newest release.
- Unrecognized signatures produce only a Version-settings message: the information is not signed by a trusted key, automatic updating is unavailable, and any newer version must come from a source independently verified with whoever runs the person's server. This state removes the update notice, download address and install control. It does not direct the person to the unauthenticated source.
- Metadata is limited to 64 KiB and a 60-second complete deadline. Archives are limited to 512 MiB, a 60-second idle deadline reset by received bytes, and a total of 60 seconds plus signed size divided by 16 KiB/second, rounded upward. Redirects and proxies are disabled; requests contain no account/device identifier. A failed attempt restarts from zero.
- Downloads create a private file, enforce the signed size while streaming, flush and sync, then verify the held file on a blocking worker. Verification runs once more immediately before extraction. Failure or cancellation removes the temporary file. Fixed public fixtures cover all three signature domains; their private keys are not retained.
- External test key, certificate and version inputs never rewrite tracked files. Any build override records a test-configuration marker, visible in Version and `--version`; publication requires an explicit test flag. The public release wrapper refuses overrides.

## Consequences

- Timely freshness renewal and a reasonable local clock are required, even when no new code is released. Release-authorizing secrets need not be brought online for renewal.
- An old trusted key remains authoritative for clients that still contain it; rotation does not remotely revoke that authority.
- macOS additionally checks its permanent code-signing certificate. Release commands do not upload files.
- Validation reuses existing URL, JSON, base64 and Ed25519 packages.
