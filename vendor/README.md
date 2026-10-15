# Native call-library dependency

`webrtc-sys-0.3.48` contains the build inputs of the published `webrtc-sys` 0.3.48 crate. The source archive has SHA-256 `9d87fa13acfe63505a0e8c1311b5dc60323a429196c4a7ba18b63de34e5cba67`. Cargo uses this copy through the workspace patch.

The retained inputs are `Cargo.toml`, `build.rs`, `src/`, `include/`, `README.md` and `NOTICE.md`. The archive's development instructions, changelog, nested lockfile, original unnormalized manifest, native-library build scripts and unused native tests are not required by this build and are omitted. Copyright and licence notices in the retained inputs are unchanged.

The only source correction is in `src/peer_connection.cpp`: the peer-connection destructor calls `Close` while both its native interface and observer still exist. ADR 0733 explains the shutdown order and its regression checks. The WebRTC binary revision and every other dependency version are unchanged.

Remove the patch when the pinned upstream crate closes its native peer connection before releasing these members. Run the native destruction tests, window live tier and media live tier when replacing it.
