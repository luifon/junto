# Native call-library dependency

`webrtc-sys-0.3.48` contains the build inputs of the published `webrtc-sys` 0.3.48 crate. The source archive has SHA-256 `9d87fa13acfe63505a0e8c1311b5dc60323a429196c4a7ba18b63de34e5cba67`. Cargo uses this copy through the workspace patch and the standalone test participant's patch.

The retained inputs are `Cargo.toml`, `build.rs`, `src/`, `include/`, `README.md` and `NOTICE.md`. The archive's development instructions, changelog, nested lockfile, original unnormalized manifest, native-library build scripts and unused native tests are not required by this build and are omitted. Copyright and licence notices in the retained inputs are unchanged.

The peer-connection destructor in `src/peer_connection.cpp` calls `Close` while both its native interface and observer still exist. Linux runtime initialization also registers a system-root TLS verifier from `src/system_certificate_verifier.{rs,cpp}` through the pinned binary's platform-verifier API. It uses `rustls-native-certs` to load roots and Rustls to verify chains with SSL server purpose. The build adds that bridge and its Rust dependencies only on Linux. ADR 0733 explains both corrections, their regression checks and the combined compiled-in/system trust policy. The WebRTC binary revision and every other dependency version are unchanged.

Remove the destructor correction when the pinned upstream crate closes its native peer connection before releasing these members. Remove the Linux trust correction when a pinned upstream version provides system-root verification for every default TLS adapter. Run the native destruction tests, window live tier and media live tier when replacing this dependency; the Linux media live tier includes the TURN TLS trust regression.
