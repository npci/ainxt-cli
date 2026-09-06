//! In-TUI `[model.*]` add/edit/remove screen.
//!
//! Lets the user manage model provider entries without hand-editing
//! `config.toml`. See `crates/codegen/ainxt-shell/src/agent/config_model_override_write.rs`
//! for the surgical writer this screen drives, and `views/settings_modal`
//! for the sibling pattern this module's shape is adapted from.

mod input;
mod render;
mod state;

pub use input::{ModelEditorKeyOutcome, handle_model_editor_key, handle_model_editor_paste};
pub use render::{
    SHORTCUT_CONFIRM_NO_ID, SHORTCUT_CONFIRM_YES_ID, SHORTCUT_DELETE_ID, SHORTCUT_ENTER_ID,
    SHORTCUT_ESC_ID, SHORTCUT_SAVE_ID, render_model_editor,
};
pub use state::{
    CredentialMode, FieldEdit, ModelEditorField, ModelEditorForm, ModelEditorMode,
    ModelEditorState, FIELD_ORDER,
};
