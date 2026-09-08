//! Shared utilities used by both `ainxt-shell` and its downstream clients
//! (e.g. `ainxt-pager-render`). This crate sits upstream of `ainxt-shell`
//! so it must never depend on it.

pub mod clipboard;
pub mod placeholder_images;
pub mod session;
pub mod stderr;
pub mod ui_config;
