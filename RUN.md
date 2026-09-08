# Running AiNxt

This guide covers building the `ainxt` binaries and running the CLI against an
AiNxt CLI gateway. `ainxt` does not bundle any models or default backend — it talks
to a gateway you run and point it at.

---

## 1. Prerequisites (build machine)

- **Rust** — the toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml);
  `rustup` installs it automatically on first build.
- **DotSlash** — needed so the hermetic `protoc` under [`bin/protoc`](bin/protoc)
  can run. Install and put `dotslash` on `PATH` before building:
  ```sh
  cargo install dotslash
  # sanity check
  /usr/bin/env dotslash --help
  ```
  (Alternatively, install `protoc` yourself and set `$PROTOC` to its path.)
- macOS and Linux are supported build hosts. Windows builds are produced via
  cross-compilation / CI (see §3).

> Behind an outbound proxy? Set `HTTPS_PROXY` / `HTTP_PROXY` before the first
> `cargo build` so crates.io and DotSlash downloads succeed. Once built, the
> binary needs no proxy for normal use (only for reaching the gateway).

---

## 2. Build

### Native (this machine)
```sh
# fast validation
cargo check -p ainxt-pager-bin

# release binary -> target/release-dist/ainxt
cargo build --profile release-dist -p ainxt-pager-bin --bin ainxt

# run it directly
./target/release-dist/ainxt
```

### All platforms at once (local)
```sh
scripts/build-release.sh 3.0.0-beta
# -> dist/ainxt-3.0.0-beta-darwin-aarch64
#    dist/ainxt-3.0.0-beta-darwin-x86_64
#    dist/ainxt-3.0.0-beta-linux-x86_64
#    dist/ainxt-3.0.0-beta-linux-aarch64
#    dist/ainxt-3.0.0-beta-win32-x86_64.exe   (needs the Windows cross toolchain)
```
The version you pass is both stamped into the artifact filename and compiled in,
so `ainxt --version` reports the same string. Linux and Windows cross-builds from
macOS use `cargo-zigbuild` (`cargo install cargo-zigbuild`, plus `brew install
zig`). Targets whose toolchain is missing are skipped with a warning rather than
failing the whole run.

### All platforms via CI (recommended for releases)
**No release workflow ships in this repository** — you supply your own. A tag-triggered
job should build macOS (arm64 + x64), Linux (x64 + aarch64) and Windows (x64), then attach
the artifacts to the release. Name them exactly as the install scripts expect, or
installation will fail:

```
ainxt-<version>-<os>-<arch>[.exe]
```

### Headless / ACP
Headless is **not** a separate binary — the same `ainxt` runs headless for
scripting/CI and over stdio/ACP for editor integrations:
```sh
ainxt -p "summarize the changes in this repo"   # one-shot, no TUI
ainxt agent stdio                               # ACP over stdio
```

---

## 3. Configure the gateway

AiNxt CLI talks to a gateway you operate. Point it there with one environment
variable (this is the only required setting):

```sh
export AINXT_GATEWAY_URL=https://gateway.example.com
```

The CLI calls `${AINXT_GATEWAY_URL}/ainxt/v1/api/...` — auth, models, chat, etc.

### Key environment variables

**Gateway & auth** (most commonly needed):

| Variable | Purpose | Default |
|---|---|---|
| `AINXT_GATEWAY_URL` | **Primary config** — gateway base URL; derives all API paths | `http://localhost:8000/ainxt/v1/api` |
| `AINXT_TOKEN` | Bearer token for non-interactive / CI use | — |
| `AINXT_DEPLOYMENT_KEY` | Enterprise deployment key (takes priority over `AINXT_TOKEN`) | — |
| `AINXT_API_BASE_URL` | Override model API base independently of the gateway | gateway |
| `AINXT_HOME` | Config/state directory | `~/.ainxt` |

**UI / branding** (override to point at your own portal):

All UI links ship **empty** in the open-source build — there is no hosted AiNxt
portal, and an empty value means "this build has no such page", so the UI omits
the affordance instead of offering a dead link. Set these only to pages you
operate. (Background: these constants once pointed at a domain the project did
not own — see the note in `crates/codegen/ainxt-env/src/lib.rs`.)

| Variable | Default |
|---|---|
| `AINXT_URL_SUBSCRIBE` | *(empty)* |
| `AINXT_URL_USAGE` | *(empty)* |
| `AINXT_URL_DOCS` | *(empty — there is no hosted docs site; the guide is in this repo)* |
| `AINXT_URL_LEGAL` | *(empty)* |
| `AINXT_URL_PROMO`, `AINXT_URL_CONNECTORS`, `AINXT_URL_CHANGELOG_BASE` | *(empty)* |

**Marketplace** (override to use your own plugin registry):

| Variable | Default |
|---|---|
| `AINXT_MARKETPLACE_SOURCE_URL` | `https://github.com/ainxt-org/plugin-marketplace.git` |
| `AINXT_MARKETPLACE_ORG` | `ainxt-org/plugin-marketplace` |

**Security / TLS** (dev/SIT only — never production):

| Variable | Purpose |
|---|---|
| `AINXT_ALLOW_INSECURE=1` | Allow a plaintext `http://` gateway on a non-loopback host. Loopback (`localhost`, `127.0.0.1`) is always permitted without this. |

> **`AINXT_TLS_INSECURE` no longer exists.** The runtime TLS-verification bypass
> was removed from this codebase; setting the variable has no effect. To use a
> gateway with a private or self-signed certificate, add your CA to the system
> trust store instead — that keeps server identity verified.

**Retries / timeouts** (important for headless and CI):

| Variable | Purpose | Default |
|---|---|---|
| `AINXT_MAX_RETRIES` | Retry budget for API calls | `15` — with the 30s backoff cap this is **~6 minutes** of retrying |
| `AINXT_CONNECT_TIMEOUT_SECS` | Per-request connect timeout | — |

> These retries emit **nothing** on stdout or stderr while they run. The most
> common cause of an apparent hang — an empty model catalogue — is now caught
> before the prompt is sent and fails in a few seconds with an explanatory
> error. But a gateway that *does* serve a catalogue and then fails on
> `/messages` will still retry for the full budget in silence, so set
> `AINXT_MAX_RETRIES=2` in CI along with a job-level timeout.

**Auto-update**:

| Variable | Values | Purpose |
|---|---|---|
| `AINXT_INSTALLER` | `gateway`, `gh-release`, `internal`, `npm` | Update channel (auto-detected from `AINXT_GATEWAY_URL`) |
| `AINXT_DISABLE_AUTOUPDATER=1` | any | Disable background update check |

**API backend / wire protocol**:

| Variable | Values | Purpose |
|---|---|---|
| `AINXT_API_BACKEND` | `chat_completions` (default), `messages`, `responses` | Force the request **body format** for all models |

> Note that the wire format and the URL path are independent. The path is always
> `{gateway}/ainxt/v1/api/messages` — that is the AiNxt gateway's own endpoint
> name — while the body sent to it defaults to **Chat Completions** format
> (`ApiBackend::default()`), not Anthropic Messages format. If you are
> implementing a gateway, this is the combination to expect from an unconfigured
> client. Per-model `api_backend` in `config.toml` overrides this.

**Debugging**:

| Variable | Purpose |
|---|---|
| `AINXT_DEBUG_LOG=1` | Enable debug logging (or set to a file path) |
| `RUST_LOG=ainxt_shell=debug,ainxt_sampler=debug` | Verbose log filter |

> **Full reference:** [`env.example`](env.example) documents the operational
> settings with descriptions and defaults. It is not exhaustive — the code reads
> roughly 350 `AINXT_*` variables, most of them internal test or tuning knobs.
> The fullest prose reference is
> [`05-configuration.md`](crates/codegen/ainxt-pager/docs/user-guide/05-configuration.md).

You can make settings permanent in your shell profile (`~/.zshrc` / `~/.bashrc`)
or in `~/.ainxt/config.toml`.

---

## 4. Sign in

AiNxt CLI authenticates with a **bearer token** issued by the gateway:

```sh
ainxt login
# -> "Paste your AiNxt CLI token:"  (create one in the AiNxt CLI web console)
```

The token is stored at `~/.ainxt/credentials.json` (mode 600). For email/password
sign-in instead: `ainxt login --password`. For CI/headless, set `AINXT_TOKEN` and
skip interactive login entirely.

Verify:
```sh
ainxt --version
ainxt       # launches the TUI; models are loaded from the gateway
```

If the gateway is unreachable, the TUI shows no models (there are no
bundled/fallback models — everything comes from the gateway).

In **headless** mode (`ainxt -p`) an empty catalogue is detected before the
prompt is sent: the CLI exits non-zero within a few seconds and prints what to
check. With `--output-format json` the same thing arrives on stdout as
`{"type":"error", ...}`.

If the catalogue loads but requests then fail, the retry budget still applies and
is still silent — use `AINXT_MAX_RETRIES=2` and
`RUST_LOG=ainxt_shell=debug,ainxt_sampler=debug` while you are getting the
configuration right.

---

## 5. Install so `AiNxt` is on your PATH

**Quick (personal):** copy the binary somewhere on your `PATH`:
```sh
mkdir -p ~/.ainxt/bin
cp target/release-dist/ainxt ~/.ainxt/bin/ainxt
echo 'export PATH="$HOME/.ainxt/bin:$PATH"' >> ~/.zshrc   # or ~/.bashrc
exec $SHELL
ainxt --version
```

**Team distribution:** host the `dist/ainxt-<ver>-<os>-<arch>` artifacts on a
server your users can reach and adapt [`crates/codegen/ainxt-pager/scripts/install.sh`](crates/codegen/ainxt-pager/scripts/install.sh)
(set its base URL to your artifact host). Windows: `install.ps1`.

---

## 6. Quick checklist

```sh
# 1. build
cargo build --profile release-dist -p ainxt-pager-bin --bin ainxt

# 2. point at the gateway
export AINXT_GATEWAY_URL=https://gateway.example.com

# 3. sign in with your token
./target/release-dist/ainxt login

# 4. run
./target/release-dist/ainxt
```

That's it — `ainxt` connects to the gateway, loads your models, and you're in
the TUI.

---

## 7. Troubleshooting

**The chat keeps "retrying" and never answers.** This almost always means the
model catalog came back empty, so no model was selected. Check, in order:

- `AINXT_GATEWAY_URL` is set and reachable. A quick check:
  `curl "$AINXT_GATEWAY_URL/ainxt/v1/api/models"` should return a JSON list.
- You are signed in (`ainxt login`) — the token in `~/.ainxt/credentials.json`
  authenticates the model-list fetch, not just chat.
- Pick a model explicitly if the default is unavailable (e.g. an upstream
  provider is out of quota): `ainxt -m "<model-id>" -p "hello"`. List the ids
  the gateway offers with `ainxt models`.

**`ainxt -p` exits immediately with "No models are available".** The model
catalogue came back empty, so there was nothing to send the prompt to. The error
lists what to check; the causes are the same as the entry above. This is a
fail-fast guard: before it existed the same situation produced ~6 minutes of
complete silence and then a bare non-zero exit.

**`ainxt -p` hangs with no output.** The catalogue loaded, so the failure is in
the request itself, and the retry loop is silent for the whole
`AINXT_MAX_RETRIES` budget (~6 min by default). Run with `AINXT_MAX_RETRIES=2`
and `RUST_LOG=ainxt_shell=debug,ainxt_sampler=debug` to see the real cause.
Always set `AINXT_MAX_RETRIES` and a job timeout in CI.

**404s on `/ainxt/v1/api/...`, and an empty model list.** You have almost
certainly pointed `AINXT_GATEWAY_URL` at something that is not an AiNxt gateway.
The CLI appends `/ainxt/v1/api` to that URL, so Ollama, vLLM, LiteLLM and plain
OpenAI-compatible servers — which serve `/v1/...` — will 404 every call. Those
belong in a `[model.*]` entry in `config.toml` with `base_url`, or in
`AINXT_API_BASE_URL`; see §6 of [`env.example`](env.example) and
[`11-custom-models.md`](crates/codegen/ainxt-pager/docs/user-guide/11-custom-models.md).

**"Not signed in" when using only a local model.** The CLI requires *some*
credential before it will start a session, even against an endpoint that needs
none. Set `AINXT_API_KEY` to any non-empty placeholder value.

**"insufficient_quota" / 429 from a cloud model.** That is the upstream provider
(OpenAI/Anthropic/Gemini) rejecting the gateway's key — a gateway-side billing
issue, not the CLI. Switch to a model whose provider has quota (for example an
in-house `local:*` model).

**Debugging.** Set `RUST_LOG=ainxt_shell=debug,ainxt_sampler=debug` to see the
model-fetch URL, the selected model, and any auth or stream-decode errors.

---

## 8. Supply-chain & license gates (OSS-002)

Before publishing or cutting a release, run the following two tools to confirm
the dependency tree is clean.

> **No CI workflows ship in this repository** — there is no `.github/` directory,
> so nothing below is enforced automatically. Run these by hand before a release,
> or wire them into the CI you supply (see §2, "All platforms via CI").

### Install

```sh
cargo install cargo-deny
cargo install cargo-audit
```

### Run

```sh
# License compliance — checks all deps against deny.toml policy
cargo deny check licenses

# Security advisories — checks Cargo.lock against RustSec advisory DB
cargo audit

# Run all deny checks at once (licenses + advisories + bans + sources)
cargo deny check
```

### Policy

The policy is defined in [`deny.toml`](deny.toml) at the repo root. Key rules:

- **Allowed:** MIT, Apache-2.0, BSD-2/3-Clause, ISC, Unicode, Zlib, 0BSD, BSL-1.0, CC0-1.0
- **Exceptions (require legal sign-off):** MPL-2.0 (9 deps — file-level copyleft only),
  `libgit2-sys` (GPL-2.0 WITH linking exception — acceptable when linked dynamically)
- **Denied:** AGPL, LGPL (without dynamic-linking defense), SSPL, BUSL, non-commercial

If `cargo deny check licenses` reports an unexpected license, add an explicit
`exceptions` entry in `deny.toml` with a comment explaining the legal rationale,
and get sign-off before merging.
