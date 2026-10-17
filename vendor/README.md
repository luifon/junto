# Call-library dependencies

## `webrtc-sys` 0.3.48

`webrtc-sys-0.3.48` contains the build inputs of the published `webrtc-sys` 0.3.48 crate. The source archive has SHA-256 `9d87fa13acfe63505a0e8c1311b5dc60323a429196c4a7ba18b63de34e5cba67`. Cargo uses this copy through the workspace patch and the standalone test participant's patch.

The retained inputs are `Cargo.toml`, `build.rs`, `src/`, `include/`, `README.md` and `NOTICE.md`. The archive's development instructions, changelog, nested lockfile, original unnormalized manifest, native-library build scripts and unused native tests are not required by this build and are omitted. Copyright and licence notices in the retained inputs are unchanged.

The peer-connection destructor in `src/peer_connection.cpp` calls `Close` while both its native interface and observer still exist. Linux runtime initialization also registers a system-root TLS verifier from `src/system_certificate_verifier.{rs,cpp}` through the pinned binary's platform-verifier API. It uses `rustls-native-certs` to load roots and Rustls to verify chains with SSL server purpose. The build adds that bridge and its Rust dependencies only on Linux. ADR 0733 explains both corrections, their regression checks and the combined compiled-in/system trust policy. The WebRTC binary revision and every other dependency version are unchanged.

Remove the destructor correction when the pinned upstream crate closes its native peer connection before releasing these members. Remove the Linux trust correction when a pinned upstream version provides system-root verification for every default TLS adapter. Run the native destruction tests, window live tier and media live tier when replacing this dependency; the Linux media live tier includes the TURN TLS trust regression.

## `webrtc-sys-build` 0.3.19

`webrtc-sys-build-0.3.19` contains the build inputs of the published `webrtc-sys-build` 0.3.19 crate, the build-script helper of `webrtc-sys` that downloads and extracts the WebRTC library. The source archive has SHA-256 `0cd4f418e16f4b7ba028e6b5c70e4f532c7d2b5ca39726d42b699f3e4591ed5c`, the checksum that the lockfile held for the registry version. The archive was made from commit `2d9f01ab1e933a86a8a5c53805ee29ee58b9be1b` of the upstream repository, directory `webrtc-sys/build` (its `.cargo_vcs_info.json`). Cargo uses this copy through the workspace patch and the standalone test participant's patch.

The retained inputs are `Cargo.toml` and `src/`. The archive's changelog, nested lockfile, original unnormalized manifest and `.cargo_vcs_info.json` are not required by this build and are omitted. The archive has no separate licence file; the crate is licensed under Apache-2.0 (its manifest), and the copyright and licence notice at the top of `src/lib.rs` is unchanged.

`src/lib.rs` is unchanged. `Cargo.toml` differs in one dependency, marked with a `Modified:` comment: `zip` 9.0.4 with its deflate decompressor (`deflate-flate2-zlib-rs`) replaces `zip` 0.6 with the feature `deflate`. The `ZipArchive::extract` of `zip` 0.6 keeps the setuid, setgid and sticky bits of an entry (RUSTSEC-2026-0367) and applies permissions through extracted symbolic links (RUSTSEC-2026-0369). ADR 0733 explains the correction and its checks. Every other dependency version is unchanged.

Remove this copy when a pinned upstream version depends on `zip` 9.0.0 or later. Build `junto-media` from an empty build directory on macOS and Linux when replacing it, so that the WebRTC archive is downloaded and extracted again, then run `cargo deny check` and the media live tier.

## `livekit` 0.9.4

`livekit-0.9.4` contains the build inputs of the published `livekit` 0.9.4 crate. The source archive has SHA-256 `4dc818741891a8179eafd4665156482761d5084fb15238c1de9c211fdb974e3e`, the checksum that the lockfile held for the registry version. The archive was made from commit `a6f93a16fa71fe811d040de175e070e233d9b1fe` of the upstream repository, directory `livekit` (its `.cargo_vcs_info.json`). Cargo uses this copy through the workspace patch and the standalone test participant's patch.

The retained inputs are `Cargo.toml`, `build.rs`, `src/` and `README.md`. The archive's changelog, nested lockfile, original unnormalized manifest, `.cargo_vcs_info.json`, examples, specifications and tests are not required by this build and are omitted. The archive has no separate licence file; the crate is licensed under Apache-2.0 (its manifest), and the copyright and licence notice at the top of each retained source file is unchanged.

Every file and every dependency version not named below are unchanged. Each changed file states in its header that it differs from the published version, and each change carries a `Modified:` comment; a new file states that it is not in the published crate. ADR 0733 explains each correction and its regression checks. Remove a correction when a pinned upstream version makes it unnecessary, and remove this copy when no correction remains. Run the tests of each remaining correction against a new version first, then the window live tier and the media live tier.

Corrections:

- Publisher negotiation queue. `src/rtc_engine/negotiation_queue.rs` is new; `src/rtc_engine/mod.rs` and `src/rtc_engine/rtc_session.rs` use it. The queue of the fast publish mode decides with one state under one lock whether a task negotiates. The published version also kept a separate flag for a running task; a request that came after the task had set the state to idle and before it cleared that flag was dropped, and every later request with it. `crates/media/tests/negotiation_queue.rs` compiles the new file and runs its tests in the workspace suite. Removed when a pinned upstream version decides the queue under one state, so that a request after the end of a negotiation always starts another one.
- Order of mute requests. `src/room/participant/in_order.rs` is new; `src/room/participant/mod.rs` uses it. The mute and unmute requests of the local tracks of a participant are sent one after the other, in the order of the changes. The published version started a task for each request; tasks that waited together for a restored connection sent in any order, and a mute that reached the media server after the unmute that followed it left the track muted there. `crates/media/tests/mute_order.rs` compiles the new file and runs its tests in the workspace suite. Removed when a pinned upstream version sends the mute requests of a track in their order.
