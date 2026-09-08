# Custom Models

ainxt connects to custom model endpoints for alternative providers, self-hosted models, and overriding built-in settings. This guide explains how to select models, configure endpoints, and integrate third-party providers.

---

## Default Models

There are **no** built-in or bundled models. This build compiles in no default
gateway, so an unconfigured CLI starts with an empty model list and tells you so.
Every model you can select comes from one of two places:

- the catalogue served by the gateway you set `AINXT_GATEWAY_URL` to, or
- a `[model.*]` entry you define yourself, as described in this chapter.

New sessions start with `ainxt-build` **if your gateway offers a model by that
name**. Authenticate with `ainxt login` or an API key, then start a session.

List all available models:

```bash
ainxt models
```

---

## Selecting a Model

### CLI Flag

```bash
ainxt -p "Hello" -m ainxt-build
```

### Slash Command

In the TUI, switch models during a session:

```
/model ainxt-build
```

Or use the alias:

```
/m ainxt-build
```

### Model Picker (Ctrl+M)

Press `Ctrl+M` from the scrollback pane to open the model picker. It lists all available models, both built-in and custom, and lets you switch with a single keystroke. With the prompt focused, `Ctrl+M` toggles multiline input instead -- use `/model` to switch without leaving the prompt.

### Config Default

Set a persistent default in `~/.ainxt/config.toml`:

```toml
[models]
default = "ainxt-build"
```

---

## Supported API Backends

ainxt supports three API backends. Set `api_backend` in your `[model.*]` config to choose which protocol the model uses:

| Value | API | Default |
|-------|-----|---------|
| `"chat_completions"` | OpenAI Chat Completions (`/v1/chat/completions`) | Yes |
| `"responses"` | OpenAI Responses (`/v1/responses`) | |
| `"messages"` | Anthropic Messages (`/v1/messages`) | |

When you omit `api_backend`, ainxt uses `chat_completions`.

To send provider-specific authentication or version headers -- for example, Anthropic's `x-api-key` -- use the `extra_headers` field described below. ainxt sends those headers unchanged with every request to the endpoint.

---

## Model Capabilities

The Messages API has a core that every Messages-compatible endpoint implements
(`model`, `messages`, `tools`, `tool_choice`, `max_tokens`, `temperature`) plus
Anthropic-specific extensions that a self-hosted model behind a translating
gateway usually does not. Sending an extension to an endpoint that doesn't
implement it either fails the whole request or silently degrades the response.

Several extensions only appear once a conversation has history — a replayed
`thinking` block, for instance — so a mismatch typically looks like "the first
message works, then multi-turn and subagents break," rather than an obvious
failure on turn one.

ainxt tracks five capabilities per model:

| Capability | Controls |
|---|---|
| `thinking` | The top-level `thinking` request field, and replaying signed `thinking` blocks in assistant history |
| `prompt_caching` | `cache_control: {type: "ephemeral"}` cache breakpoints |
| `output_config` | The `output_config` field, carrying reasoning `effort` and native structured-output `format` |
| `system_blocks` | A block-array `system` prompt. When disabled, the sections are joined into a single string |
| `images` | Image content blocks. Not an Anthropic extension — a text-only model errors on an image rather than ignoring it. When disabled, images become a placeholder that names this flag |

### Defaults

ainxt derives the profile from the model id when you don't configure one:

- **In-house / self-hosted models** get the core-only profile: all five
  capabilities off. Note this includes `images`, since the common self-hosted
  coding models are text-only — set `images = true` for a local vision model. Recognized are the gateway's `local:` prefix, provider
  routes (`ollama:`, `vllm:`, `sglang:`), the open-weight family names with or
  without an org prefix (`kimi`, `qwen`, `glm`, `gemma`, `llama`, `mistral`,
  `deepseek`, `phi`, `gpt-oss`, e.g. `moonshotai/Kimi-K2.7-Code`), and the
  `-inhouse` / `-in-house` / `-internal` suffixes.
- **Every other model** gets the full profile, all five capabilities on.

So a self-hosted model generally needs no capability configuration at all.

### Overriding

Set any subset under `[model.<id>.capabilities]`; unlisted keys stay enabled.
An explicit profile always wins over the id-based default, in both directions —
re-enable a feature your gateway does support, or withhold one from a model the
detector treats as hosted:

```toml
[model.my-local-kimi]
model = "kimi-k2.7"
base_url = "http://10.0.0.5:8000/v1"
api_backend = "messages"
context_window = 262144
max_completion_tokens = 32768

[model.my-local-kimi.capabilities]
thinking = false          # gateway does not implement extended thinking
prompt_caching = false    # no prompt cache to address
output_config = false     # no reasoning-effort / json_schema support
system_blocks = true      # this gateway does accept a block-array system
images = false            # text-only model
```

A gateway can also advertise the profile per model in its `/v1/models`
response, under the same key names, which removes the need for local config
entirely.

To confirm what a model resolved to, run with `--log-sampling`: a narrowed
profile is logged once per model with each capability's value.

---

## Configuring Custom Models

Add custom model endpoints in `~/.ainxt/config.toml` under `[model.<name>]` sections:

```toml
[model.my-model]
model = "model-id"                        # Model identifier sent to the API
base_url = "https://api.example.com/v1"   # OpenAI-compatible endpoint
name = "Display Name"                     # Shown in the model picker
description = "Model description"          # Optional description
api_key = "sk-..."                        # API key for this provider (optional)
env_key = "AINXT_API_KEY"                   # Env var holding the API key (optional; string or array)
api_backend = "chat_completions"          # "chat_completions", "responses", or "messages"
# [model.my-model.capabilities]                # optional; see "Model Capabilities" above
temperature = 0.7                         # Sampling temperature
top_p = 0.95                              # Nucleus sampling parameter
max_completion_tokens = 8192              # Maximum tokens per response
context_window = 128000                   # Total context window in tokens
extra_headers = { "x-api-key" = "sk-..." } # Extra request headers, sent verbatim (optional)
```

### Credential Resolution

ainxt resolves the API key in this order:

1. The `api_key` field in the model config
2. The environment variable(s) named by `env_key` — a single string or an array of names. The first set, non-empty value wins (for example `env_key = ["ANTHROPIC_AUTH_TOKEN", "LC_ANTHROPIC_AUTH_TOKEN"]` for SSH `LC_*` forwarding)
3. Your signed-in session token (from `ainxt login`), for a model with no `api_key`/`env_key` of its own
4. The `AINXT_API_KEY` environment variable (global fallback; ainxt also accepts `AINXT_CODE_AINXT_API_KEY` for backward compatibility)

### Context Window

The `context_window` value tells ainxt when to trigger auto-compaction. When you override a known model, ainxt inherits that model's context window. When you define a new model and omit `context_window`, ainxt defaults to 200,000 tokens, so set it explicitly to match your provider.

### Global Default Headers

To apply the same headers to *every* model in the catalog -- built-in, prefetched from `/v1/models`, or custom -- set them once under the global `[models]` section instead of repeating them per model:

```toml
[models]
extra_headers = { "X-Request-Tags" = "team=example,env=prod" }
```

These act as a base for each model's inference requests. A per-model `[model.<id>].extra_headers` entry overrides the global default **per key** (matched case-insensitively): a key set on the model wins, while any global-only keys are still inherited by that model. Like the per-model field, they ride on that model's inference calls -- not on separate services such as image generation or video generation -- which makes them handy for attribution tags (for example, cost tracking) without re-declaring them whenever a new model appears.

### Global Default Values

A few common per-model settings can also be set once under `[models]` as a default for *every* model. A per-model `[model.<id>]` value always wins; the global only fills in where a model (or the server's model list) left the field unset:

```toml
[models]
temperature                 = 0.7
top_p                       = 0.95
max_completion_tokens       = 8192
max_retries                 = 8
inference_idle_timeout_secs = 600
stream_tool_calls           = true
```

This is a small, fixed set of environment-wide knobs. Settings that identify a specific model (`model`, `base_url`, `api_key`, `context_window`, ...) cannot be defaulted this way, and a few settings with their own dedicated configuration -- auto-compaction (`[session]`), the system-prompt label (`[agent]`), and reasoning effort (`[models].default_reasoning_effort`) -- keep their existing homes.

> **Note on `stream_tool_calls`:** this one affects request *shape*, not just sampling. A few endpoints (some BYOK providers) expect it left unset; if a global `stream_tool_calls = true` causes problems for such a model, opt that model out with `stream_tool_calls = false` in its `[model.<id>]` block.

---

## Overriding Built-in Models

You can override specific fields of built-in models without redefining everything. Only specify the fields you want to change:

```toml
# Override only the API key for a default model
[model.ainxt-build]
api_key = "my-api-key"

# Override temperature and add a custom API key
[model.ainxt-build]
temperature = 0.5
api_key = "sk-custom"
```

When you override a built-in model, ainxt starts with the default configuration (including the correct `base_url`), then applies only the fields you specify. Unspecified fields inherit from the default.

### Priority Order

1. Your config (`[model.*]`) -- highest priority
2. Prefetched models from remote `/v1/models`
3. Hardcoded defaults -- lowest priority

---

## Provider Examples

### Anthropic (Claude)

Use Claude models directly via the Anthropic Messages API:

```toml
[model.claude-opus]
model = "claude-opus-4-6"
base_url = "https://api.anthropic.com/v1"
name = "Claude Opus 4.6"
api_backend = "messages"
context_window = 200000
extra_headers = { "x-api-key" = "sk-ant-...", "anthropic-version" = "2023-06-01" }
```

The `messages` backend uses the Anthropic Messages protocol. Anthropic authenticates with an `x-api-key` header rather than `Authorization: Bearer`, so pass your key through `extra_headers`, which ainxt sends unchanged.

### OpenAI (Responses API) — recommended for current models

Current-generation OpenAI models (reasoning effort, tool use, and similar
features) are served through the newer Responses API. Use `api_backend =
"responses"`, not `"chat_completions"`, for these — the older backend can
fail outright depending on the model:

```toml
[model.gpt-5]
model = "gpt-5"
base_url = "https://api.openai.com/v1"
name = "GPT-5"
api_backend = "responses"
env_key = "OPENAI_API_KEY"
```

### OpenAI (Chat Completions)

`chat_completions` is ainxt's default when `api_backend` is omitted, and
still correct for OpenAI-compatible endpoints that only implement the older
Chat Completions shape (many self-hosted/BYOK gateways). Check your specific
model/provider's docs if you're unsure which one it expects:

```toml
[model.gpt-4o]
model = "gpt-4o"
base_url = "https://api.openai.com/v1"
name = "GPT-4o"
env_key = "OPENAI_API_KEY"
```

### Ollama (Local Models)

Run models locally with [Ollama](https://ollama.ai):

```toml
[model.ollama-codellama]
model = "codellama"
base_url = "http://localhost:11434/v1"
name = "CodeLlama (Ollama)"
```

Make sure Ollama is running (`ollama serve`) and the model is pulled (`ollama pull codellama`).

Then select it explicitly:

```bash
export AINXT_API_KEY=local     # placeholder — see the note below
ainxt -m ollama-codellama -p "hello"
```

> **You still need a credential set, even though Ollama does not check one.**
> ainxt refuses to start a session with no credential of any kind and exits with
> `Not signed in. To authenticate without a browser, run: ainxt login --device-code`.
> That message is misleading here — there is nothing to sign in to. Set
> `AINXT_API_KEY` to any non-empty placeholder value and the local model works.
>
> Do **not** set `AINXT_GATEWAY_URL=http://localhost:11434` for this. ainxt
> appends `/ainxt/v1/api` to that variable, so every call would 404 against
> Ollama and your model list would come back empty.

### Together AI

```toml
[model.together-mixtral]
model = "mistralai/Mixtral-8x7B-Instruct-v0.1"
base_url = "https://api.together.xyz/v1"
name = "Mixtral 8x7B"
env_key = "TOGETHER_API_KEY"
```

### Local OpenAI-Compatible Server

Any server that implements the OpenAI Chat Completions or Responses API:

```toml
[model.local-llama]
model = "llama-3.1-70b"
base_url = "http://localhost:8080/v1"
name = "Local Llama"
temperature = 0.8
```

---

## Custom Models Endpoint

Point ainxt at a custom OpenAI-compatible `/v1/models` endpoint instead of the default. Use this when your models sit behind a corporate gateway or a self-hosted inference service.

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `AINXT_MODELS_BASE_URL` | Yes | Base URL for inference. ainxt fetches the model list from `{base_url}/models`. |
| `AINXT_API_KEY` | Yes | API key sent as `Authorization: Bearer`. ainxt also accepts `AINXT_CODE_AINXT_API_KEY`. |
| `AINXT_MODELS_LIST_URL` | No | Override the model-list URL when it differs from `{base_url}/models`. |

### Setup

```bash
export AINXT_MODELS_BASE_URL="https://api.acme.com/v1"
export AINXT_API_KEY="ainxt-..."
ainxt
```

### Config File Alternative

```toml
[endpoints]
models_base_url = "https://api.acme.com/v1"

# Override only the API key for a specific model
[model.ainxt-build]
api_key = "my-api-key"
```

When you use `[endpoints]` with partial model overrides, ainxt inherits the `base_url` from the endpoints config, so you do not need to specify it in each `[model.*]` section.

### Auth Behavior

When you set `models_base_url`, ainxt uses API key auth (`Authorization: Bearer`) instead of session auth. You do not need `ainxt login` -- the API key is enough.

---

## Web Search Model

The `web_search` tool uses a separate model. Configure it with:

```toml
[models]
web_search = "ainxt-4.20-multi-agent"
```

Or via environment variable:

```bash
export AINXT_WEB_SEARCH_MODEL="ainxt-4.20-multi-agent"
```

If you point web search at a custom model, you also need a `[model.*]` entry so ainxt can reach it. Server-side ("backend") web search runs only when the model sets `supports_backend_search = true` (and the build enables backend search); it does not depend on `api_backend`:

```toml
[models]
web_search = "my-custom-model"

[model.my-custom-model]
model = "my-custom-model"
supports_backend_search = true
```

---

## Using Custom Models

```bash
# List available models (including custom)
ainxt models

# Use in the TUI via slash command
/model my-model

# Use in headless mode
ainxt -p "Hello" -m my-model

# Set as default in config.toml:
[models]
default = "my-model"
```

---

## Enterprise Deployment

A complete config for an enterprise deployment with custom models:

```toml
[cli]
auto_update = false

[auth]
auth_provider_command = "/usr/local/bin/my-company-auth-provider"
auth_provider_label = "Acme Corp"
auth_token_ttl = 3600

[models]
default = "company-ainxt"

[model.company-ainxt]
model = "ainxt-build"
base_url = "https://ainxt-proxy.acme.com/"
name = "ainxt Latest (Proxy)"
context_window = 128000

[features]
telemetry = false
```

---

## Troubleshooting

### Model Not Found

```bash
# List available models
ainxt models

# Check config.toml for typos in [model.*] sections
```

### Connection Errors

Verify the endpoint is reachable:

```bash
curl -s https://api.example.com/v1/models \
  -H "Authorization: Bearer $AINXT_API_KEY"
```

### Debug Logging

```bash
RUST_LOG=debug AINXT_LOG_FILE=/tmp/ainxt.log ainxt
tail -f /tmp/ainxt.log
```

Look for log entries containing `model` or `sampling` to trace model selection and API calls.
