//! The Junto desktop application: a gpui window on the client core (ADR 0701, ADR 0730).
//!
//! The crate has no rule of joining, of login or of the event stream; those are in
//! `junto-client-core`. It holds the theme, the catalog of the interface text, the
//! components, the state that follows the client core, and the main window.

pub mod app;
pub mod assets;
pub mod compose;
pub mod emoji;
pub mod highlight;
pub mod images;
pub mod logs;
pub mod moment;
pub mod notify;
pub mod opening;
pub mod panics;
#[cfg(target_os = "linux")]
pub mod renderer;
pub mod shell;
pub mod state;
#[cfg(windows)]
pub mod taskbar;
pub mod text;
pub mod theme;
pub mod ui;
pub mod window_icon;
pub mod window_level;
pub mod zones;
