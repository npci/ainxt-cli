//! Surgical writer for `[model.<id>]` entries in `config.toml`.
//!
//! Companion to [`super::config_model_override_parse`]: writes only the
//! curated fields the in-TUI model editor exposes (`name`, `model`,
//! `base_url`, `api_backend`, `context_window`, and one of `api_key`/
//! `env_key`), leaving every other key in the table — known-but-uncurated or
//! genuinely unknown to this crate — byte-for-byte untouched, and preserving
//! comments/formatting elsewhere in the file via `toml_edit`.
//!
//! Deliberately never serializes the full `ConfigModelOverride` struct: it
//! has no `skip_serializing_if` on most fields, so a whole-struct serialize
//! would splat every field (including ones the editor never touched) into
//! the file. Mirrors the pattern in `extensions/marketplace.rs`'s
//! `add_marketplace_source`/`remove_source_locked` — lock, `spawn_blocking`,
//! read-modify-write via `toml_edit::DocumentMut`, atomic rename-on-write.

use std::path::Path;

use indexmap::IndexMap;

use super::config::ConfigModelOverride;
use crate::sampling::ApiBackend;
use crate::util::config::{atomic_write_string, lock_config_writes, read_to_string_or_empty};

/// Read-side companion for the in-TUI model editor: the raw, un-merged
/// `[model.<id>]` overrides as currently written to disk (effective config,
/// i.e. including managed-config layering) — NOT the resolved runtime
/// catalog (`ModelState`/`ModelInfo`), which has already applied overrides
/// onto built-ins and lost the distinction between "explicitly set" and
/// "inherited". The editor needs the raw overrides so editing one field
/// doesn't silently freeze in an inherited value for every other field.
pub fn model_overrides_from_effective_config() -> std::io::Result<IndexMap<String, ConfigModelOverride>>
{
    let raw = crate::util::config::load_effective_config()?;
    Ok(super::config_model_override_parse::parse_model_overrides(&raw).models)
}

/// Curated fields the in-TUI model editor can set on one `[model.<id>]`
/// entry. `None` means "leave whatever is already on disk for this key
/// untouched" (or, for a brand-new entry, "don't write this key at all").
#[derive(Clone, Debug, Default)]
pub struct ModelOverrideFormFields {
    pub name: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_backend: Option<ApiBackend>,
    pub context_window: Option<u64>,
    pub credential: CredentialInput,
}

/// Credential fields are mutually exclusive by construction: exactly one of
/// `api_key`/`env_key` is ever written, and choosing one removes the other
/// from the file (so a stale secret never lingers if the user switches
/// modes, and a hand-edited file that has both self-heals on next save).
#[derive(Clone, Debug, Default)]
pub enum CredentialInput {
    ApiKey(String),
    EnvKey(String),
    /// Remove both `api_key` and `env_key` if present; don't write either.
    #[default]
    None,
}

/// Error from a model-override write. Wraps the underlying I/O/parse error
/// (surfaced verbatim to the user — see the in-TUI editor's save-failure
/// path) or a `spawn_blocking` join failure.
#[derive(Debug)]
pub enum ModelOverrideWriteError {
    Io(std::io::Error),
    TaskJoin(String),
}

impl std::fmt::Display for ModelOverrideWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::TaskJoin(e) => write!(f, "config write task failed: {e}"),
        }
    }
}

impl std::error::Error for ModelOverrideWriteError {}

/// Insert or update `[model.<model_key>]` with `fields`'s curated values.
///
/// `original_key`, when `Some` and different from `model_key`, means the
/// user renamed the model id during this edit: the old table is deleted and
/// the new one inserted inside the same locked write, so a crash mid-save
/// can't leave both a stale and a fresh entry.
///
/// Refuses (returns `Err`) if the file exists and doesn't parse as TOML —
/// never clobbers a malformed file.
pub async fn save_model_override(
    original_key: Option<String>,
    model_key: String,
    fields: ModelOverrideFormFields,
) -> Result<(), ModelOverrideWriteError> {
    let _guard = lock_config_writes().await;
    let path = crate::util::config::user_config_path();
    tokio::task::spawn_blocking(move || {
        write_model_override(&path, original_key.as_deref(), &model_key, &fields)
    })
    .await
    .map_err(|e| ModelOverrideWriteError::TaskJoin(e.to_string()))?
    .map_err(ModelOverrideWriteError::Io)
}

/// Remove `[model.<model_key>]` entirely. No-ops (`Ok`) if the key, the
/// `[model]` table, or the file itself doesn't exist.
pub async fn remove_model_override(model_key: String) -> Result<(), ModelOverrideWriteError> {
    let _guard = lock_config_writes().await;
    let path = crate::util::config::user_config_path();
    tokio::task::spawn_blocking(move || delete_model_override(&path, &model_key))
        .await
        .map_err(|e| ModelOverrideWriteError::TaskJoin(e.to_string()))?
        .map_err(ModelOverrideWriteError::Io)
}

fn parse_doc(existing: String) -> std::io::Result<toml_edit::DocumentMut> {
    existing.parse::<toml_edit::DocumentMut>().map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, format!("invalid TOML: {e}"))
    })
}

fn write_model_override(
    path: &Path,
    original_key: Option<&str>,
    model_key: &str,
    fields: &ModelOverrideFormFields,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let existing = read_to_string_or_empty(path)?;
    let mut doc = parse_doc(existing)?;

    if let Some(old_key) = original_key {
        if old_key != model_key {
            remove_model_table(&mut doc, old_key);
        }
    }

    let model_section = doc
        .entry("model")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let model_table = model_section.as_table_mut().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "[model] is not a table")
    })?;
    model_table.set_implicit(true);

    let entry_item = model_table
        .entry(model_key)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let entry = entry_item.as_table_mut().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("[model.{model_key}] is not a table"),
        )
    })?;

    set_or_remove_str(entry, "name", fields.name.as_deref());
    set_or_remove_str(entry, "model", fields.model.as_deref());
    set_or_remove_str(entry, "base_url", fields.base_url.as_deref());

    if let Some(backend) = &fields.api_backend {
        entry["api_backend"] = toml_edit::value(api_backend_str(backend));
        if matches!(backend, ApiBackend::Messages) {
            ensure_anthropic_version_header(entry);
        }
    }

    match fields.context_window {
        Some(cw) => entry["context_window"] = toml_edit::value(cw as i64),
        None => {
            entry.remove("context_window");
        }
    }

    match &fields.credential {
        CredentialInput::ApiKey(key) => {
            entry["api_key"] = toml_edit::value(key.as_str());
            entry.remove("env_key");
        }
        CredentialInput::EnvKey(name) => {
            entry["env_key"] = toml_edit::value(name.as_str());
            entry.remove("api_key");
        }
        CredentialInput::None => {
            entry.remove("api_key");
            entry.remove("env_key");
        }
    }

    atomic_write_string(path, &doc.to_string())
}

fn delete_model_override(path: &Path, model_key: &str) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let existing = read_to_string_or_empty(path)?;
    let mut doc = parse_doc(existing)?;
    remove_model_table(&mut doc, model_key);
    atomic_write_string(path, &doc.to_string())
}

fn remove_model_table(doc: &mut toml_edit::DocumentMut, model_key: &str) {
    if let Some(model_table) = doc.get_mut("model").and_then(|item| item.as_table_mut()) {
        model_table.remove(model_key);
    }
}

fn set_or_remove_str(table: &mut toml_edit::Table, key: &str, value: Option<&str>) {
    match value {
        Some(v) if !v.is_empty() => table[key] = toml_edit::value(v),
        _ => {
            table.remove(key);
        }
    }
}

/// Anthropic's Messages API rejects requests without this header, and
/// nothing in the sampler auto-injects it the way it does `x-api-key` (see
/// `ainxt-sampler/src/client.rs`'s `AuthScheme::XApiKey` handling) — the
/// curated editor form has no `extra_headers` field, so without this the
/// header would be silently missing every time. Only sets it if absent, so
/// a user's own override (e.g. a newer API version) is never clobbered.
fn ensure_anthropic_version_header(entry: &mut toml_edit::Table) {
    const ANTHROPIC_VERSION: &str = "2023-06-01";
    let headers_item = entry
        .entry("extra_headers")
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    if let Some(headers) = headers_item.as_table_mut()
        && !headers.contains_key("anthropic-version")
    {
        headers["anthropic-version"] = toml_edit::value(ANTHROPIC_VERSION);
    }
}

fn api_backend_str(backend: &ApiBackend) -> &'static str {
    match backend {
        ApiBackend::ChatCompletions => "chat_completions",
        ApiBackend::Responses => "responses",
        ApiBackend::Messages => "messages",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sync(
        path: &Path,
        original_key: Option<&str>,
        model_key: &str,
        fields: &ModelOverrideFormFields,
    ) -> std::io::Result<()> {
        write_model_override(path, original_key, model_key, fields)
    }

    #[test]
    fn creates_model_table_with_curated_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let fields = ModelOverrideFormFields {
            name: Some("My Model".into()),
            model: Some("gpt-4o".into()),
            base_url: Some("https://api.example.com/v1".into()),
            api_backend: Some(ApiBackend::ChatCompletions),
            context_window: Some(128_000),
            credential: CredentialInput::EnvKey("MY_MODEL_KEY".into()),
        };
        write_sync(&path, None, "my-model", &fields).unwrap();

        let doc = parse_doc(std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = doc["model"]["my-model"].as_table().unwrap();
        assert_eq!(entry["name"].as_str(), Some("My Model"));
        assert_eq!(entry["model"].as_str(), Some("gpt-4o"));
        assert_eq!(entry["api_backend"].as_str(), Some("chat_completions"));
        assert_eq!(entry["context_window"].as_integer(), Some(128_000));
        assert_eq!(entry["env_key"].as_str(), Some("MY_MODEL_KEY"));
        assert!(entry.get("api_key").is_none());
    }

    #[test]
    fn messages_backend_auto_sets_anthropic_version_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let fields = ModelOverrideFormFields {
            model: Some("claude-opus-4-6".into()),
            api_backend: Some(ApiBackend::Messages),
            credential: CredentialInput::EnvKey("ANTHROPIC_API_KEY".into()),
            ..Default::default()
        };
        write_sync(&path, None, "claude-opus", &fields).unwrap();

        let doc = parse_doc(std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = doc["model"]["claude-opus"].as_table().unwrap();
        assert_eq!(
            entry["extra_headers"]["anthropic-version"].as_str(),
            Some("2023-06-01"),
            "the Messages API rejects requests without this header, and nothing \
             auto-injects it the way x-api-key is derived from api_key/env_key"
        );
    }

    #[test]
    fn messages_backend_does_not_clobber_a_customized_anthropic_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[model.claude-opus]\nmodel = \"claude-opus-4-6\"\n\
             [model.claude-opus.extra_headers]\nanthropic-version = \"2099-01-01\"\n",
        )
        .unwrap();

        let fields = ModelOverrideFormFields {
            model: Some("claude-opus-4-6".into()),
            api_backend: Some(ApiBackend::Messages),
            ..Default::default()
        };
        write_sync(&path, Some("claude-opus"), "claude-opus", &fields).unwrap();

        let doc = parse_doc(std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = doc["model"]["claude-opus"].as_table().unwrap();
        assert_eq!(
            entry["extra_headers"]["anthropic-version"].as_str(),
            Some("2099-01-01"),
            "a user's own override must survive a later save, not get reset to the default"
        );
    }

    #[test]
    fn non_messages_backend_does_not_add_anthropic_version_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let fields = ModelOverrideFormFields {
            model: Some("gpt-4o".into()),
            api_backend: Some(ApiBackend::ChatCompletions),
            ..Default::default()
        };
        write_sync(&path, None, "gpt", &fields).unwrap();

        let doc = parse_doc(std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = doc["model"]["gpt"].as_table().unwrap();
        assert!(entry.get("extra_headers").is_none());
    }

    #[test]
    fn preserves_sibling_sections_and_uncurated_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[ui]\ncompact_mode = true\n\n\
             [model.existing]\nmodel = \"old-id\"\ntemperature = 0.4\ncapabilities = { thinking = true }\n",
        )
        .unwrap();

        let fields = ModelOverrideFormFields {
            model: Some("new-id".into()),
            ..Default::default()
        };
        write_sync(&path, None, "existing", &fields).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let doc = parse_doc(body.clone()).unwrap();
        assert_eq!(doc["model"]["existing"]["model"].as_str(), Some("new-id"));
        assert!(
            body.contains("temperature"),
            "uncurated field must survive: {body}"
        );
        assert!(
            body.contains("compact_mode"),
            "sibling [ui] section must survive: {body}"
        );
    }

    #[test]
    fn switching_credential_mode_removes_the_other_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[model.x]\nmodel = \"m\"\napi_key = \"sk-old\"\n",
        )
        .unwrap();

        let fields = ModelOverrideFormFields {
            credential: CredentialInput::EnvKey("X_KEY".into()),
            ..Default::default()
        };
        write_sync(&path, None, "x", &fields).unwrap();

        let doc = parse_doc(std::fs::read_to_string(&path).unwrap()).unwrap();
        let entry = doc["model"]["x"].as_table().unwrap();
        assert_eq!(entry["env_key"].as_str(), Some("X_KEY"));
        assert!(entry.get("api_key").is_none(), "stale api_key must be removed");
    }

    #[test]
    fn rename_deletes_old_key_and_inserts_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[model.old-id]\nmodel = \"m\"\n").unwrap();

        let fields = ModelOverrideFormFields {
            model: Some("m".into()),
            ..Default::default()
        };
        write_sync(&path, Some("old-id"), "new-id", &fields).unwrap();

        let doc = parse_doc(std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(doc["model"].get("old-id").is_none());
        assert!(doc["model"].get("new-id").is_some());
    }

    #[test]
    fn refuses_to_clobber_unparseable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let bad = "this is [not valid toml\n";
        std::fs::write(&path, bad).unwrap();

        let err = write_sync(&path, None, "x", &ModelOverrideFormFields::default());
        assert!(err.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad);
    }

    #[test]
    fn delete_removes_only_the_named_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[model.keep]\nmodel = \"a\"\n\n[model.drop]\nmodel = \"b\"\n",
        )
        .unwrap();

        delete_model_override(&path, "drop").unwrap();

        let doc = parse_doc(std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(doc["model"].get("drop").is_none());
        assert!(doc["model"].get("keep").is_some());
    }

    #[test]
    fn delete_on_missing_file_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(delete_model_override(&path, "anything").is_ok());
        assert!(!path.exists());
    }
}
