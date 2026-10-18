# 0801. Licence and the reference-code process

## Status

Accepted

## Context

The project is public and depends on gpui and LiveKit, which are licensed under Apache-2.0. The Zed editor is built with gpui, and its application crates are licensed under GPL-3.0-or-later. Code derived from GPL code cannot be distributed under Apache-2.0. Code that is written with a language model may reproduce code that the model saw in training, whatever the model was given to read.

## Decision

Licence:

- The project is public under Apache-2.0, the licence of gpui and of LiveKit.
- CI checks the licence of every dependency.
- A copy of a dependency in `vendor/` keeps the copyright and licence notices of its files. Each changed source file states in its header that it differs from the published version, each change carries a `Modified:` comment (in a manifest, at the changed dependency), and a new file states that it is not in the published crate. Apache-2.0 section 4(b) requires a prominent notice of change in each changed file of a redistributed work.

The licence file of a client package:

- A client package carries the licence and notice texts of the third-party code in its programs. The MIT, BSD, ISC and Zlib licences require their copyright and permission notices in binary copies, Apache-2.0 section 4 requires the licence and the attribution notices of a `NOTICE` file, and the SIL Open Font License requires its text with the fonts.
- Each package (the macOS bundle, the Windows ZIP, the Linux tarball and the AppImage) has a directory `licences` (`Contents/Resources/licences` in the bundle) with `Junto-LICENSE.txt`, the licence of Junto, and `third-party-licences.txt`. `packaging/licences.py` writes the second file when `bundle.py`, `windows.py` or `linux.py` assembles the package, from the checkout and from the directory of the programs of the build.
- The crates are those that `cargo tree` gives for the packages of the two programs, `junto-client` (`junto`) and `junto-client-core` (`junto-recover`), for the Rust target of the package, with normal and build dependencies. This is the feature resolution of Cargo for these two packages, so crates that only the server or the tests use are not listed. Build dependencies and procedural macros are listed: their code is not in the programs, but they produce parts of the programs, such as the generated protocol code and the download of the WebRTC library. Cargo resolves build dependencies for the machine that runs it; each package is assembled on its own system. `cargo metadata` gives the licence, the authors and the directory of each crate.
- The texts of a crate are the files of its package whose names start with `LICENSE`, `LICENCE`, `COPYING` or `UNLICENSE`, the files of a directory `LICENSES`, the file that its manifest names in `license-file`, and its notice files (`COPYRIGHT`, `NOTICE`). A crate whose package holds no licence file gets the standard text of each licence identifier of its manifest: Apache-2.0 is the `LICENSE` of the repository, the others are in `packaging/licence-texts/`, from the SPDX License List 3.27.0. Its authors are named as the copyright holders. A crate that has neither a licence file nor a standard text for one of its identifiers stops the assembly of the package.
- The file states for each crate its version, its licence expression, its source (crates.io, a git repository, Junto, or a changed copy in `vendor/`) and the numbers of its texts. Each distinct text is printed once, after the list, with every component that it applies to; two texts that differ only in line endings and trailing spaces are one text.
- The crates of the workspace are listed as part of Junto. The copies in `vendor/` are listed as changed copies, with the notice file of `webrtc-sys`.
- The WebRTC library: the file `LICENSE.md` of the prebuilt library that `webrtc-sys-build` downloaded into the build directory, for the release named by `WEBRTC_TAG` in the vendored `webrtc-sys-build`, or of the library that `LK_CUSTOM_WEBRTC` names. It names WebRTC and each project that it contains, with its licence. A build directory without it stops the assembly.
- The Rust standard library of the version of `rust-toolchain.toml`, which is compiled into each program, with the MIT and Apache-2.0 texts.
- The fonts compiled into the client, with the OFL text beside each family in `crates/client/assets/fonts/`. The icons and the other pictures of the client are part of Junto.
- The Linux packages add the copyright files of the Debian packages of the libraries in `lib`, and the AppImage adds the statement of its runtime.
- No other tool is installed for this. A licence tool such as `cargo-about` identifies a licence text by its content; it would be one more pinned tool on the three CI machines and in the Linux build image. The file takes the licence files of a crate by their names and does not compare their content with the manifest; `cargo deny` checks the licence of every crate in CI.

Checks:

- The packaging unit tests generate the file for the three targets of the packages and require that its list equals the crates of `cargo tree` for the two programs. A small workspace of path crates checks normal, build, target and test dependencies, files and standard texts, texts printed once, and the refusal of a crate without a text and of a path crate outside the workspace and `vendor/`. A build directory without the WebRTC licence and two different WebRTC licences are refused; the WebRTC licence of the build of the checkout is found.
- The bundle and ZIP tests require both files in the package and the list of every crate. `packaging/check_linux.py` and `packaging/check_windows.py` check the same in the packages of a real build, with the WebRTC licence of that build, and on Linux the copyright files of the bundled libraries and the AppImage statement.

Reference code:

- The default is not to read GPL code. The Apache-2.0 sources (gpui, the LiveKit SDK, the component libraries) cover most needs.
- Where they do not, the GPL-3.0-or-later application crates of the Zed editor are a reference only, through three separate sessions:
  1. A reader reads the code and writes a description of its behaviour in plain language, with no code and no copied identifiers.
  2. A writer implements from that description and from the gpui and LiveKit documentation, without the reference source.
  3. A checker reads both and passes or fails each file. The checker's feedback names only the project's file and the kind of similarity, and never describes the reference code. Similarity that a library API forces is not a failure. After three failed rounds on one file, the loop stops and the description is rewritten.
- A record is kept of each use of the three-session process.

Similarity check:

- The similarity check covers more than the three-session process. A code-similarity tool compares every client file that implements text editing, lists and windows against a local copy of the GPL crates.

## Consequences

- A dependency with a licence that is not compatible fails CI.
- A correction of a vendored file adds its notice in the same change; `vendor/README.md` names each changed and new file.
- A new dependency whose package holds no licence file and whose licence has no standard text stops the assembly of a package until its text is added to `packaging/licence-texts/`.
- The assembly of a package needs Cargo, the sources of the crates and the build directory of the programs. The file is about 1 MB.
- The AppImage statement names the components of its runtime (libfuse, squashfuse, zstd, zlib) and a link to them; their texts are not in the file. The server image is not covered by this file.
- A function that only the GPL crates show how to build costs three sessions and at least one comparison round.
- The writer never has the reference source, and the checker's feedback cannot carry the reference code to the writer.
- Client files for text editing, lists and windows are compared against the GPL crates even when no reference was read for them.
- A local copy of the GPL crates is kept for the comparison only and is not part of the repository.
- The process reduces the risk of copying. It is not a legal guarantee.
