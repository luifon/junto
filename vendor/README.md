# Call-library dependencies

## `webrtc-sys` 0.3.48

`webrtc-sys-0.3.48` contains the build inputs of the published `webrtc-sys` 0.3.48 crate. The source archive has SHA-256 `9d87fa13acfe63505a0e8c1311b5dc60323a429196c4a7ba18b63de34e5cba67`. Cargo uses this copy through the workspace patch and the standalone test participant's patch.

The retained inputs are `Cargo.toml`, `build.rs`, `src/`, `include/`, `README.md` and `NOTICE.md`. The archive's development instructions, changelog, nested lockfile, original unnormalized manifest, native-library build scripts and unused native tests are not required by this build and are omitted. Copyright and licence notices in the retained inputs are unchanged.

The peer-connection destructor in `src/peer_connection.cpp` calls `Close` while both its native interface and observer still exist. Linux runtime initialization also registers a system-root TLS verifier from `src/system_certificate_verifier.{rs,cpp}` through the pinned binary's platform-verifier API. It uses `rustls-native-certs` to load roots and Rustls to verify chains with SSL server purpose. The build adds that bridge and its Rust dependencies only on Linux. ADR 0733 explains both corrections, their regression checks and the combined compiled-in/system trust policy. The WebRTC binary revision and every other dependency version are unchanged.

Remove the destructor correction when the pinned upstream crate closes its native peer connection before releasing these members. Remove the Linux trust correction when a pinned upstream version provides system-root verification for every default TLS adapter. Run the native destruction tests, window live tier and media live tier when replacing this dependency; the Linux media live tier includes the TURN TLS trust regression.

## `livekit` 0.9.4

`livekit-0.9.4` contains the build inputs of the published `livekit` 0.9.4 crate. The source archive has SHA-256 `4dc818741891a8179eafd4665156482761d5084fb15238c1de9c211fdb974e3e`, the checksum that the lockfile held for the registry version. Cargo uses this copy through the workspace patch and the standalone test participant's patch.

The retained inputs are `Cargo.toml`, `build.rs`, `src/` and `README.md`. The archive's changelog, nested lockfile, original unnormalized manifest, examples, specifications and tests are not required by this build and are omitted. The archive has no separate licence file; the crate is licensed under Apache-2.0 (its manifest), and the copyright and licence notice at the top of each retained source file is unchanged.

Every file and every dependency version not named below are unchanged. ADR 0733 explains each correction and its regression checks. Remove a correction when a pinned upstream version makes it unnecessary, and remove this copy when no correction remains. Run the tests of each remaining correction against a new version first, then the window live tier and the media live tier.

Corrections:

- Publisher negotiation queue. `src/rtc_engine/negotiation_queue.rs` is new; `src/rtc_engine/mod.rs` and `src/rtc_engine/rtc_session.rs` use it. The queue of the fast publish mode decides with one state under one lock whether a task negotiates. The published version also kept a separate flag for a running task; a request that came after the task had set the state to idle and before it cleared that flag was dropped, and every later request with it. `crates/media/tests/negotiation_queue.rs` compiles the new file and runs its tests in the workspace suite. Removed when a pinned upstream version decides the queue under one state, so that a request after the end of a negotiation always starts another one.
