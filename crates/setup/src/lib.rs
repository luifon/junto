//! The Junto setup tool (ADR 0204, ADR 0212): `export` writes the structure of a server to
//! a file, `plan` shows what applying a file would change, and `apply` makes the changes.
//! It uses the public API with an access token and has no other access.

pub mod api;
pub mod cli;
pub mod plan;
