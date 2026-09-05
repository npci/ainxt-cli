//! State for the in-TUI `[model.*]` add/edit/remove screen.
//!
//! Mode taxonomy is deliberately flatter than `settings_modal`'s (which is
//! built around a static, fixed-key `SettingsRegistry`): this editor has a
//! dynamic set of user-named entries, so the form lives once inside
//! `ModelEditorMode::Editing` and a field-level sub-state (`FieldEdit`)
//! tracks whichever single field is actively being typed into or picked,
//! instead of duplicating the whole form per field-editing variant.

use ainxt_shell::agent::config::ConfigModelOverride;
use ainxt_shell::agent::config_model_override_write::{CredentialInput, ModelOverrideFormFields};
use ainxt_shell::sampling::ApiBackend;
use indexmap::IndexMap;

use crate::views::modal_window::ModalWindowState;

/// Fixed field order for the editing form. `Id` is the TOML key itself, not
/// a `ConfigModelOverride` field.
pub const FIELD_ORDER: [ModelEditorField; 8] = [
    ModelEditorField::Id,
    ModelEditorField::Name,
    ModelEditorField::Model,
    ModelEditorField::BaseUrl,
    ModelEditorField::ApiBackend,
    ModelEditorField::ContextWindow,
    ModelEditorField::CredentialMode,
    ModelEditorField::CredentialValue,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelEditorField {
    Id,
    Name,
    Model,
    BaseUrl,
    ApiBackend,
    ContextWindow,
    CredentialMode,
    CredentialValue,
}

impl ModelEditorField {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Id => "Model id (config key)",
            Self::Name => "Display name (optional)",
            Self::Model => "Model (provider-side id)",
            Self::BaseUrl => "Base URL",
            Self::ApiBackend => "API backend",
            Self::ContextWindow => "Context window",
            Self::CredentialMode => "Credential",
            Self::CredentialValue => "Credential value",
        }
    }

    /// One-line example/explanation shown under this field while it's
    /// focused. `Id` and `Model` look similar but mean different things —
    /// one selects the entry, the other is sent to the provider — so both
    /// get a concrete example rather than relying on the label alone.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Id => Some(
                "How you'll select this entry: /model <id> or -m <id>. e.g. \"claude-opus\"",
            ),
            Self::Name => Some(
                "Cosmetic label shown in pickers only. Leave blank to just show the id above.",
            ),
            Self::Model => Some(
                "Exact model string your provider's API expects, e.g. \"claude-opus-4-6\", \"gpt-4o\", \"llama3.1:latest\".",
            ),
            Self::BaseUrl => Some("e.g. \"https://api.anthropic.com/v1\". Leave blank to inherit a known model's URL."),
            Self::ContextWindow => Some("Token limit before auto-compaction. Leave blank to default to 200000."),
            Self::ApiBackend | Self::CredentialMode | Self::CredentialValue => None,
        }
    }

    /// Whether this field is a choice-picker rather than free text.
    pub fn is_choice(&self) -> bool {
        matches!(self, Self::ApiBackend | Self::CredentialMode)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CredentialMode {
    #[default]
    EnvKey,
    ApiKey,
    None,
}

impl CredentialMode {
    pub const CHOICES: [CredentialMode; 3] = [Self::EnvKey, Self::ApiKey, Self::None];

    pub fn label(&self) -> &'static str {
        match self {
            Self::EnvKey => "Environment variable (recommended)",
            Self::ApiKey => "API key (stored in config.toml)",
            Self::None => "None (use login / global key)",
        }
    }
}

fn api_backend_label(b: &ApiBackend) -> &'static str {
    match b {
        ApiBackend::ChatCompletions => "chat_completions",
        ApiBackend::Responses => "responses",
        ApiBackend::Messages => "messages",
    }
}

/// In-progress form for one `[model.<id>]` entry.
#[derive(Debug, Clone)]
pub struct ModelEditorForm {
    pub id: String,
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub api_backend: ApiBackend,
    /// Text buffer; parsed/validated to `u64` on save. Blank = omit.
    pub context_window: String,
    pub credential_mode: CredentialMode,
    pub env_key_value: String,
    pub api_key_value: String,
}

impl ModelEditorForm {
    pub fn new_empty() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            model: String::new(),
            base_url: String::new(),
            api_backend: ApiBackend::ChatCompletions,
            context_window: String::new(),
            credential_mode: CredentialMode::EnvKey,
            env_key_value: String::new(),
            api_key_value: String::new(),
        }
    }

    pub fn from_existing(key: &str, over: &ConfigModelOverride) -> Self {
        let (credential_mode, env_key_value, api_key_value) = match (&over.env_key, &over.api_key) {
            (Some(env), _) => (
                CredentialMode::EnvKey,
                env.primary().unwrap_or_default().to_string(),
                String::new(),
            ),
            (None, Some(key)) => (CredentialMode::ApiKey, String::new(), key.clone()),
            (None, None) => (CredentialMode::None, String::new(), String::new()),
        };
        Self {
            id: key.to_string(),
            name: over.name.clone().unwrap_or_default(),
            model: over.model.clone().unwrap_or_default(),
            base_url: over.base_url.clone().unwrap_or_default(),
            api_backend: over.api_backend.clone().unwrap_or_default(),
            context_window: over
                .context_window
                .map(|v| v.to_string())
                .unwrap_or_default(),
            credential_mode,
            env_key_value,
            api_key_value,
        }
    }

    /// Read-only text/label for `field`, for row rendering.
    pub fn display_value(&self, field: ModelEditorField) -> String {
        match field {
            ModelEditorField::Id => self.id.clone(),
            ModelEditorField::Name => self.name.clone(),
            ModelEditorField::Model => self.model.clone(),
            ModelEditorField::BaseUrl => self.base_url.clone(),
            ModelEditorField::ApiBackend => api_backend_label(&self.api_backend).to_string(),
            ModelEditorField::ContextWindow => self.context_window.clone(),
            ModelEditorField::CredentialMode => self.credential_mode.label().to_string(),
            ModelEditorField::CredentialValue => match self.credential_mode {
                CredentialMode::EnvKey => self.env_key_value.clone(),
                CredentialMode::ApiKey => {
                    if self.api_key_value.is_empty() {
                        String::new()
                    } else {
                        "*".repeat(self.api_key_value.chars().count().min(20))
                    }
                }
                CredentialMode::None => "(not set)".to_string(),
            },
        }
    }

    pub fn text_buffer_for(&self, field: ModelEditorField) -> String {
        match field {
            ModelEditorField::Id => self.id.clone(),
            ModelEditorField::Name => self.name.clone(),
            ModelEditorField::Model => self.model.clone(),
            ModelEditorField::BaseUrl => self.base_url.clone(),
            ModelEditorField::ContextWindow => self.context_window.clone(),
            ModelEditorField::CredentialValue => match self.credential_mode {
                CredentialMode::EnvKey => self.env_key_value.clone(),
                CredentialMode::ApiKey => self.api_key_value.clone(),
                CredentialMode::None => String::new(),
            },
            ModelEditorField::ApiBackend | ModelEditorField::CredentialMode => String::new(),
        }
    }

    pub fn apply_text(&mut self, field: ModelEditorField, value: String) {
        match field {
            ModelEditorField::Id => self.id = value,
            ModelEditorField::Name => self.name = value,
            ModelEditorField::Model => self.model = value,
            ModelEditorField::BaseUrl => self.base_url = value,
            ModelEditorField::ContextWindow => self.context_window = value,
            ModelEditorField::CredentialValue => match self.credential_mode {
                CredentialMode::EnvKey => self.env_key_value = value,
                CredentialMode::ApiKey => self.api_key_value = value,
                CredentialMode::None => {}
            },
            ModelEditorField::ApiBackend | ModelEditorField::CredentialMode => {}
        }
    }

    /// Validate a single field's current buffer value; used both at
    /// field-commit time and again at full-form save time. `existing`
    /// (all other configured model ids) is only consulted for `Id`.
    pub fn validate_field(
        &self,
        field: ModelEditorField,
        original_key: Option<&str>,
        existing: &IndexMap<String, ConfigModelOverride>,
    ) -> Option<String> {
        match field {
            ModelEditorField::Id => {
                let id = self.id.trim();
                if id.is_empty() {
                    return Some("Model id is required.".into());
                }
                if id.chars().any(char::is_whitespace) {
                    return Some("Model id can't contain whitespace.".into());
                }
                // A collision is real only if some OTHER entry (not the one
                // being edited, if any) already uses this id.
                if existing.contains_key(id) && original_key != Some(id) {
                    return Some(format!("A model named '{id}' already exists."));
                }
                None
            }
            ModelEditorField::Model => {
                if self.model.trim().is_empty() {
                    Some("Model (provider-side id) is required.".into())
                } else {
                    None
                }
            }
            ModelEditorField::BaseUrl => {
                let url = self.base_url.trim();
                if url.is_empty() {
                    None // optional when overriding a known model
                } else if !(url.starts_with("http://") || url.starts_with("https://")) {
                    Some("Base URL must start with http:// or https://".into())
                } else {
                    None
                }
            }
            ModelEditorField::ContextWindow => {
                let raw = self.context_window.trim();
                if raw.is_empty() {
                    return None;
                }
                match raw.parse::<u64>() {
                    Ok(0) => Some("Context window must be greater than 0.".into()),
                    Ok(_) => None,
                    Err(_) => Some("Context window must be a whole number.".into()),
                }
            }
            ModelEditorField::CredentialValue => match self.credential_mode {
                CredentialMode::EnvKey => {
                    let name = self.env_key_value.trim();
                    if name.is_empty() {
                        Some("Environment variable name is required.".into())
                    } else if !name
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        Some("Must look like an env var name (letters, digits, _).".into())
                    } else {
                        None
                    }
                }
                CredentialMode::ApiKey => {
                    if self.api_key_value.trim().is_empty() {
                        Some("API key is required.".into())
                    } else {
                        None
                    }
                }
                CredentialMode::None => None,
            },
            ModelEditorField::Name | ModelEditorField::ApiBackend | ModelEditorField::CredentialMode => None,
        }
    }

    /// Full-form validation (all fields), used as the save-time gate.
    pub fn validate_all(
        &self,
        original_key: Option<&str>,
        existing: &IndexMap<String, ConfigModelOverride>,
    ) -> Option<String> {
        for field in FIELD_ORDER {
            if let Some(err) = self.validate_field(field, original_key, existing) {
                return Some(format!("{}: {err}", field.label()));
            }
        }
        None
    }

    pub fn to_write_fields(&self) -> ModelOverrideFormFields {
        let context_window = self.context_window.trim().parse::<u64>().ok();
        let credential = match self.credential_mode {
            CredentialMode::EnvKey => CredentialInput::EnvKey(self.env_key_value.trim().to_string()),
            CredentialMode::ApiKey => CredentialInput::ApiKey(self.api_key_value.trim().to_string()),
            CredentialMode::None => CredentialInput::None,
        };
        ModelOverrideFormFields {
            name: non_empty(&self.name),
            model: non_empty(&self.model),
            base_url: non_empty(&self.base_url),
            api_backend: Some(self.api_backend.clone()),
            context_window,
            credential,
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// One in-flight field-level edit, layered on top of `Editing`.
#[derive(Debug, Clone)]
pub enum FieldEdit {
    Text {
        buffer: String,
        cursor_byte: usize,
        error: Option<String>,
    },
    Choice {
        choices: Vec<String>,
        index: usize,
    },
}

#[derive(Debug)]
pub enum ModelEditorMode {
    /// Flat list: configured models + trailing "+ Add new model...".
    Browse,
    Editing {
        /// `None` for a brand-new model.
        original_key: Option<String>,
        form: ModelEditorForm,
        /// Index into `FIELD_ORDER`.
        field_focus: usize,
        /// `Some` while actively typing into / picking `field_focus`.
        field_edit: Option<FieldEdit>,
    },
    ConfirmDelete {
        model_key: String,
    },
    /// Write in flight; blocks further input until the task result lands.
    Saving {
        model_key: String,
    },
    Deleting {
        model_key: String,
    },
}

impl ModelEditorMode {
    pub fn title(&self) -> &'static str {
        match self {
            Self::Browse => "Manage Models",
            Self::Editing { original_key, .. } => {
                if original_key.is_some() {
                    "Edit Model"
                } else {
                    "Add Model"
                }
            }
            Self::ConfirmDelete { .. } => "Remove model?",
            Self::Saving { .. } => "Saving model...",
            Self::Deleting { .. } => "Removing model...",
        }
    }
}

#[derive(Debug)]
pub struct ModelEditorState {
    pub window: ModalWindowState,
    /// Snapshot of configured `[model.*]` entries, refreshed optimistically
    /// after a successful save/delete (not re-read from disk on every
    /// keystroke).
    pub entries: IndexMap<String, ConfigModelOverride>,
    /// Selected row in Browse mode: `0..entries.len()` picks an entry, and
    /// `entries.len()` is the trailing "+ Add new model..." row.
    pub selected: usize,
    pub mode: ModelEditorMode,
    /// Inline, non-fatal error banner (e.g. a failed save). Cleared on the
    /// next successful action.
    pub error: Option<String>,
}

impl ModelEditorState {
    pub fn new(entries: IndexMap<String, ConfigModelOverride>) -> Self {
        Self {
            window: ModalWindowState::new(),
            entries,
            selected: 0,
            mode: ModelEditorMode::Browse,
            error: None,
        }
    }

    pub fn add_row_index(&self) -> usize {
        self.entries.len()
    }

    pub fn start_add(&mut self) {
        self.mode = ModelEditorMode::Editing {
            original_key: None,
            form: ModelEditorForm::new_empty(),
            field_focus: 0,
            field_edit: None,
        };
        self.error = None;
    }

    pub fn start_edit(&mut self, key: &str) {
        let Some(over) = self.entries.get(key) else {
            return;
        };
        self.mode = ModelEditorMode::Editing {
            original_key: Some(key.to_string()),
            form: ModelEditorForm::from_existing(key, over),
            field_focus: 0,
            field_edit: None,
        };
        self.error = None;
    }

    pub fn handle_save_success(&mut self, model_key: &str) {
        self.error = None;
        self.mode = ModelEditorMode::Browse;
        // Optimistic refresh: the caller (dispatch) already has the write's
        // own fields available at the call site, but simplest correct
        // option here is to just drop back to Browse and let the next
        // modal-open re-read from disk. Selection resets to the top.
        let _ = model_key;
        self.selected = 0;
    }

    pub fn handle_save_failure(&mut self, error: String) {
        // Stay wherever the user was (Saving carries no form; the caller
        // is expected to have kept `Editing`'s form intact by never
        // clearing it — see dispatch wiring) — fall back to Browse only if
        // we've lost the form context.
        self.error = Some(error);
        if matches!(self.mode, ModelEditorMode::Saving { .. }) {
            self.mode = ModelEditorMode::Browse;
        }
    }

    pub fn handle_delete_success(&mut self, model_key: &str) {
        self.entries.shift_remove(model_key);
        self.mode = ModelEditorMode::Browse;
        self.error = None;
        if self.selected > self.add_row_index() {
            self.selected = self.add_row_index();
        }
    }

    pub fn handle_delete_failure(&mut self, error: String) {
        self.error = Some(error);
        self.mode = ModelEditorMode::Browse;
    }
}
