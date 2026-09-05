//! Foundation modules shared by the ainxt shell crate family. Extracted from
//! `ainxt-shell` (which re-exports them at their original paths) so they
//! build in parallel and stop rebuilding on shell edits.

pub mod cpu_profile;
pub mod env;
pub mod util;
