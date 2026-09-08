//! Per-model capability profile: which optional Anthropic-Messages features a
//! model's endpoint actually accepts.
//!
//! The Messages wire format has a first-party core (model, messages, tools,
//! tool_choice, max_tokens, temperature) that every Messages-compatible
//! endpoint implements, plus newer Anthropic-specific extensions that a
//! self-hosted model behind a translating gateway generally does not:
//! `thinking`, `output_config`, `cache_control` breakpoints, and a block-array
//! `system`. Sending an extension to an endpoint that does not implement it
//! either 400s the whole request or silently degrades the response — and
//! because several extensions only appear once a conversation has history
//! (replayed `thinking` blocks) the failure shows up on the second turn and in
//! subagents rather than on the first message.
//!
//! A profile travels with the sampler config, so every request the CLI builds
//! for that model is shaped to what the endpoint can take. Defaults are
//! permissive (everything on) so first-party models are unaffected; the profile
//! is narrowed either by [`ModelCapabilities::for_model`] auto-detection or by
//! an explicit `[model.<id>.capabilities]` block in `config.toml`.

use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Which optional Messages features an endpoint accepts for one model.
///
/// Every field defaults to `true`: an unknown model is assumed fully capable,
/// which keeps first-party behavior untouched. Narrow it explicitly.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ModelCapabilities {
    /// Extended thinking: the top-level `thinking` request field and replaying
    /// signed `thinking` blocks in assistant history.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub thinking: bool,

    /// Prompt-cache breakpoints (`cache_control: {type: "ephemeral"}`).
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub prompt_caching: bool,

    /// The `output_config` request field, carrying reasoning `effort` and
    /// native structured-output `format`.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub output_config: bool,

    /// A block-array `system` prompt. When false the system blocks are joined
    /// into a single plain string, which every endpoint accepts.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub system_blocks: bool,

    /// Image content blocks. Unlike the other flags this is not an Anthropic
    /// extension but a property of the model: a text-only model errors on an
    /// image block rather than ignoring it. When false, images are replaced
    /// with a placeholder naming the flag, so a dropped image is visible in the
    /// transcript instead of silently missing.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub images: bool,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self::full()
    }
}

impl ModelCapabilities {
    /// Every optional feature enabled — the first-party assumption.
    pub const fn full() -> Self {
        Self {
            thinking: true,
            prompt_caching: true,
            output_config: true,
            system_blocks: true,
            images: true,
        }
    }

    /// Only the Messages core: no thinking, no prompt-cache breakpoints, no
    /// `output_config`, a plain-string `system`, and no image blocks. The safe
    /// profile for a self-hosted model reached through a translating gateway.
    ///
    /// `images` is included because the common self-hosted coding models are
    /// text-only; a local *vision* model needs `images = true` set explicitly.
    pub const fn core_only() -> Self {
        Self {
            thinking: false,
            prompt_caching: false,
            output_config: false,
            system_blocks: false,
            images: false,
        }
    }

    /// The profile to assume for `model_id` when none was configured:
    /// [`Self::core_only`] for an in-house / self-hosted model, otherwise
    /// [`Self::full`].
    pub fn for_model(model_id: &str) -> Self {
        if is_in_house_model(model_id) {
            Self::core_only()
        } else {
            Self::full()
        }
    }

    /// True when this profile withholds at least one feature, i.e. it is worth
    /// logging that requests for the model are being narrowed.
    pub fn is_narrowed(&self) -> bool {
        *self != Self::full()
    }
}

/// True when a model id names an in-house / self-hosted model.
///
/// The gateway prefixes its self-hosted models with `local:`; the open-weight
/// family names are also recognized unprefixed, since a deployment may list
/// them under their upstream ids (`moonshotai/kimi-k2.7`, `qwen-3.6-35B`, …).
///
/// Kept deliberately conservative in the other direction: a first-party id must
/// never match, because that would silently strip thinking and prompt caching
/// from a model that supports both.
pub fn is_in_house_model(model_id: &str) -> bool {
    let id = model_id.trim().to_ascii_lowercase();

    // Gateway-assigned and provider-route prefixes.
    const PREFIXES: [&str; 9] = [
        "local:", "local-", "local/", "ollama:", "ollama/", "vllm:", "vllm/", "sglang:", "sglang/",
    ];
    if PREFIXES.iter().any(|p| id.starts_with(p)) {
        return true;
    }

    // Open-weight families, with or without an org prefix
    // (`moonshotai/kimi-k2.7`, `Qwen/Qwen3-…`).
    const FAMILIES: [&str; 9] = [
        "kimi", "qwen", "glm", "gemma", "llama", "mistral", "deepseek", "phi", "gpt-oss",
    ];
    let last_segment = id.rsplit('/').next().unwrap_or(&id);
    if FAMILIES
        .iter()
        .any(|f| last_segment.starts_with(f) || id.starts_with(f))
    {
        return true;
    }

    id.ends_with("-inhouse") || id.ends_with("-in-house") || id.ends_with("-internal")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_is_the_default_so_first_party_models_are_untouched() {
        let caps = ModelCapabilities::default();
        assert_eq!(caps, ModelCapabilities::full());
        assert!(
            caps.thinking
                && caps.prompt_caching
                && caps.output_config
                && caps.system_blocks
                && caps.images
        );
        assert!(!caps.is_narrowed());
    }

    #[test]
    fn core_only_withholds_every_extension() {
        let caps = ModelCapabilities::core_only();
        assert!(!caps.thinking);
        assert!(!caps.prompt_caching);
        assert!(!caps.output_config);
        assert!(!caps.system_blocks);
        assert!(!caps.images);
        assert!(caps.is_narrowed());
    }

    #[test]
    fn in_house_ids_are_detected() {
        for id in [
            "local:kimi-k2.7-code",
            "local:gemma-4-31B-it",
            "LOCAL:Qwen-3.6",
            "ollama:llama3.1",
            "ollama/llama3.1",
            "vllm:kimi",
            "moonshotai/kimi-k2.7",
            "moonshotai/Kimi-K2.7-Code",
            "Qwen/Qwen3-35B-A3B",
            "kimi-k2.7",
            "gpt-oss-120b",
            "deepseek-v3",
            "mistral-large",
            "phi-4",
            "acme-model-inhouse",
            "acme-model-internal",
        ] {
            assert!(is_in_house_model(id), "{id} must be detected as in-house");
        }
    }

    /// A first-party id matching here would silently strip thinking and prompt
    /// caching from a model that supports both — the expensive direction to get
    /// wrong.
    #[test]
    fn first_party_ids_are_never_in_house() {
        for id in [
            "claude-sonnet-4-6",
            "claude-opus-4-7",
            "claude-haiku-4-5-20251001",
            "gpt-5.4",
            "gpt-5-mini",
            "gpt-5-5",
            "gemini-2.5-flash",
            "gemini-3.5-flash",
            "ainxt-build",
            "ainxt-build-0.1",
            "grok-4",
        ] {
            assert!(
                !is_in_house_model(id),
                "{id} must NOT be treated as in-house"
            );
        }
    }

    #[test]
    fn for_model_picks_the_profile_by_id() {
        assert_eq!(
            ModelCapabilities::for_model("local:kimi-k2.7-code"),
            ModelCapabilities::core_only()
        );
        assert_eq!(
            ModelCapabilities::for_model("claude-sonnet-4-6"),
            ModelCapabilities::full()
        );
    }

    /// `[model.<id>.capabilities]` sets only what it wants; the rest stay on.
    /// Asserted through serde (shared by the TOML and JSON catalog paths).
    #[test]
    fn partial_table_leaves_unlisted_features_enabled() {
        let caps: ModelCapabilities =
            serde_json::from_str(r#"{"thinking": false, "prompt_caching": false}"#)
                .expect("parses");
        assert!(!caps.thinking);
        assert!(!caps.prompt_caching);
        assert!(caps.output_config, "unlisted features stay enabled");
        assert!(caps.system_blocks, "unlisted features stay enabled");
        assert!(caps.images, "unlisted features stay enabled");
    }

    /// An empty table is a fully-capable profile, not an empty one.
    #[test]
    fn empty_table_is_full() {
        let caps: ModelCapabilities = serde_json::from_str("{}").expect("parses");
        assert_eq!(caps, ModelCapabilities::full());
    }

    /// Only the withheld features serialize, so writing a config back does not
    /// spray `= true` for every model.
    #[test]
    fn serialization_only_emits_withheld_features() {
        let json = serde_json::to_value(ModelCapabilities::full()).unwrap();
        assert_eq!(json, serde_json::json!({}));

        let json = serde_json::to_value(ModelCapabilities {
            thinking: false,
            ..ModelCapabilities::full()
        })
        .unwrap();
        assert_eq!(json, serde_json::json!({"thinking": false}));
    }
}
