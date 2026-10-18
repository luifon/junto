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
- A function that only the GPL crates show how to build costs three sessions and at least one comparison round.
- The writer never has the reference source, and the checker's feedback cannot carry the reference code to the writer.
- Client files for text editing, lists and windows are compared against the GPL crates even when no reference was read for them.
- A local copy of the GPL crates is kept for the comparison only and is not part of the repository.
- The process reduces the risk of copying. It is not a legal guarantee.
