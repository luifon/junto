# 0753. The C runtime of the Windows build

## Status

Accepted

The Windows build with this setting is checked by CI only; no Windows machine builds the workspace outside CI.

## Context

The call library of the client (the LiveKit Rust SDK, ADR 0701) links a prebuilt WebRTC library. Its build script downloads `webrtc-89d790b` for `win-x64-release`, which is compiled with the static C runtime of MSVC (`/MT`, `MT_StaticRelease`). Rust on `x86_64-pc-windows-msvc` links the dynamic C runtime (`/MD`) by default, and the `cc` crate compiles the C and C++ code of build scripts with the runtime that Rust uses: the C++ part of `webrtc-sys` was compiled with `/MD`. The MSVC linker refuses objects of the two runtimes in one program (`LNK2038` "mismatch detected for 'RuntimeLibrary'"), and their two C++ standard libraries define the same symbols (`LNK2005`, then `LNK1169`). The first CI build of the client on Windows ended with these errors.

The repository of the SDK, at the commit of the published versions that the workspace locks (`livekit` 0.9.4, `webrtc-sys` 0.3.48), sets `rustflags = ["-C", "target-feature=+crt-static"]` for `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` in its `.cargo/config.toml`. A setting in the configuration of a dependency does not apply to a workspace that uses it.

## Decision

- `.cargo/config.toml` of the workspace sets `-C target-feature=+crt-static` in `rustflags` and in `rustdocflags` for `x86_64-pc-windows-msvc`, the Windows target of ADR 0705. Every crate of a Windows build, its build scripts, its tests and its documentation tests then use the static C runtime.
- The `cc` crate reads the target feature and compiles with `/MT`; this covers the C code of `ring` and the C++ code of `cxx` and `webrtc-sys`. The resource file of gpui has no C runtime. No other dependency links a prebuilt library that uses a C runtime.
- The setting is in the configuration of the workspace and not in a variable of CI, so a developer's build and CI build the same program.

## Consequences

- `RUSTFLAGS` and `RUSTDOCFLAGS` in the environment replace the flags of the configuration file. A Windows build that sets either variable must add `-C target-feature=+crt-static` itself.
- The client program contains the C runtime and the C++ standard library. A Windows package needs no Visual C++ redistributable package, and the program does not use a runtime update that Windows installs; a new runtime reaches users with a new release of the client.
- Each program and each test binary carries its own copy of the runtime, with its own heap. Memory allocated by one module and released by another fails; the client is one program without dynamic libraries of its own, so this case does not occur.
- The setting is checked by the Windows job of CI (ADR 0905). Another Windows target, or a new version of the SDK whose prebuilt library uses another runtime, needs this decision again.

## References

- [`.cargo/config.toml` of the LiveKit Rust SDK at the locked commit](https://github.com/livekit/rust-sdks/blob/a6f93a16fa71fe811d040de175e070e233d9b1fe/.cargo/config.toml).
- [The C runtime options of MSVC](https://learn.microsoft.com/en-us/cpp/build/reference/md-mt-ld-use-run-time-library).
