# Installing AiNxt CLI

Two steps: **install**, **connect a model** (your own API key — no gateway needed), **use it**.

> **Two ways in.** [Option A](#option-a--one-command) downloads the published
> binary for your platform — minutes, no toolchain. [Option B](#option-b--build-from-source)
> builds it locally, which you want if you intend to modify or audit the code, or
> run an unreleased commit. Binaries live on the
> [Releases](https://github.com/npci/ainxt-cli/releases/latest) page; grab one by
> hand if you would rather not pipe a script into a shell.

---

## Step 1 — Install

### Option A — One command

**macOS / Linux**

```sh
curl -fsSL https://raw.githubusercontent.com/npci/ainxt-cli/main/install.sh | bash
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/npci/ainxt-cli/main/install.ps1 | iex
```

There is no single command that covers all three operating systems: `curl | bash`
needs a POSIX shell and Windows PowerShell needs `irm | iex`. On Windows you can
use the `curl` line instead if you are in Git Bash, MSYS2 or WSL.

Both installers:

- detect your OS and CPU (`darwin`/`linux`/`win32` × `aarch64`/`x86_64`),
- download that binary from this repository's **GitHub Releases**,
- verify a published `.sha256` beside it when present,
- install to `~/.ainxt/bin/ainxt` and add it to your `PATH`.

**Verify integrity strictly** — refuse to install anything unverifiable:

```sh
curl -fsSL https://raw.githubusercontent.com/npci/ainxt-cli/main/install.sh | AINXT_REQUIRE_CHECKSUM=1 bash
```

```powershell
$env:AINXT_REQUIRE_CHECKSUM=1; irm https://raw.githubusercontent.com/npci/ainxt-cli/main/install.ps1 | iex
```

**Pin a version:**

```sh
curl -fsSL https://raw.githubusercontent.com/npci/ainxt-cli/main/install.sh -o install.sh && bash install.sh 0.2.101
```

```powershell
$env:AINXT_VERSION="0.2.101"; irm https://raw.githubusercontent.com/npci/ainxt-cli/main/install.ps1 | iex
```

<details>
<summary><b>About <code>curl | bash</code></b></summary>

Piping a remote script into a shell makes the transport part of your trust
boundary — you are trusting whatever the host serves at the moment you run it.
That is a real cost. It is accepted here for the sake of a one-command install,
and reduced by: HTTPS only, a single origin this project controls
(GitHub Releases), **no fallback origin**, and checksum verification of the
downloaded binary.

If you would rather not pipe to a shell, download the script, read it, then run
it — or build from source, which involves no artifact host at all.

</details>

### Option B — Build from source

Works today, no release or artifact host required. Needs Rust and ~10 GB of disk.

No Rust yet? That is the only prerequisite, and it is one command:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"                 # or open a new shell
cargo install dotslash               # runs the pinned protoc under bin/
```

You do not need to choose a Rust version — `rust-toolchain.toml` pins it and
rustup fetches that toolchain on the first build. Then:

```sh
git clone https://github.com/npci/ainxt-cli.git
cd ainxt-cli
./setup.sh
```

`./setup.sh` checks prerequisites, creates `.env` from `env.example`, builds the
binary, and prints what to run next. `./setup.sh --check` inspects prerequisites
without changing anything; `./setup.sh --release` builds the optimised binary.

Then put it on your `PATH`:

```sh
mkdir -p ~/.ainxt/bin
cp target/debug/ainxt ~/.ainxt/bin/ainxt          # or target/release-dist/ainxt
echo 'export PATH="$HOME/.ainxt/bin:$PATH"' >> ~/.zshrc   # or ~/.bashrc
exec $SHELL
```

### Option C — Download a binary by hand

Releases publish **bare binaries, not archives**. Take the one for your platform
from the [Releases](../../releases/latest) page, check it against the published
`.sha256`, then install it:

```sh
shasum -a 256 -c ainxt-<version>-<os>-<arch>.sha256
sudo install -m 0755 ainxt-<version>-<os>-<arch> /usr/local/bin/ainxt
```

Verify any of the three:

```sh
ainxt --version
```

---

## Step 2 — Connect it to an API endpoint

AiNxt CLI bundles **no models**. It gets its model catalogue from an endpoint you
point it at. There are two routes and picking the wrong one is the most common
setup failure.

### Route A — An AiNxt gateway

Use this when you have an [AiNxt Platform](https://github.com/npci/ainxt-enterprise)
deployment, or any server exposing `/ainxt/v1/api/models` and
`/ainxt/v1/api/messages`.

```sh
export AINXT_GATEWAY_URL=https://your-gateway.example.com
ainxt login          # paste your bearer token; stored in ~/.ainxt/credentials.json
```

For CI, skip `login` and set a token directly:

```sh
export AINXT_TOKEN=<your-bearer-token>
```

The CLI appends `/ainxt/v1/api` to `AINXT_GATEWAY_URL` and calls:

| Method | Path |
|---|---|
| `GET`  | `/ainxt/v1/api/models` |
| `POST` | `/ainxt/v1/api/messages` |
| `GET`  | `/ainxt/v1/api/feedback/config` |
| `GET`  | `/ainxt/v1/api/bundle/archive` |

all with `Authorization: Bearer <token>`. The request body at `messages` defaults
to **Chat Completions** format (override with `AINXT_API_BACKEND`).

Check reachability before launching:

```sh
curl "$AINXT_GATEWAY_URL/ainxt/v1/api/models"      # should return a JSON list
```

### Route B — Call a provider directly, with your own API key

This covers **any** provider with an HTTP API, cloud or local: Anthropic,
OpenAI, Together AI, Ollama, vLLM, LiteLLM, llama.cpp. No accounts, no gateway,
no `ainxt login` — you supply a key and the CLI calls the provider.

Do **not** put these in `AINXT_GATEWAY_URL`. That variable is Route A only and
has `/ainxt/v1/api` appended to it, so every call would 404 and your model list
would come back empty. Declare the provider as a model entry in
`~/.ainxt/config.toml`:

```toml
[model.ollama-llama]
model = "llama3.1:latest"
base_url = "http://localhost:11434/v1"
name = "Llama 3.1 (Ollama)"
```

```sh
export AINXT_API_KEY=local     # placeholder: the CLI requires a credential
                               # even when the endpoint needs none
ainxt -m ollama-llama -p "hello"
```

A cloud provider works the same way — only `base_url`, the auth field and
`api_backend` differ. Anthropic uses the Messages protocol and an `x-api-key`
header rather than a bearer token:

```toml
[model.claude-opus]
model = "claude-opus-4-6"
base_url = "https://api.anthropic.com/v1"
name = "Claude Opus 4.6"
api_backend = "messages"
context_window = 200000
extra_headers = { "x-api-key" = "sk-ant-...", "anthropic-version" = "2023-06-01" }
```

Wire protocols: `messages` (Anthropic), `chat_completions` (default — OpenAI and
most others), `responses` (OpenAI Responses API). Keys come from `api_key`, from
an env var via `env_key` (e.g. `OPENAI_API_KEY`), or from `extra_headers`.

Worked examples for every provider above:
[Custom models → Provider Examples](crates/codegen/ainxt-pager/docs/user-guide/11-custom-models.md).

### Making it permanent

```sh
cp env.example .env
# edit .env, then:
set -a && . ./.env && set +a     # plain `source .env` will NOT export the vars
```

`env.example` documents the operational settings.

---

## Step 3 — Use it

```sh
ainxt                            # full-screen TUI
ainxt -p "explain this repo"      # headless, one-shot
ainxt -m <model-id> -p "hello"    # pick a model
ainxt -c                          # continue your last session
ainxt models                      # list every model available to you
ainxt agent stdio                 # ACP over stdio, for editor integrations
```

Next: [Getting started](crates/codegen/ainxt-pager/docs/user-guide/01-getting-started.md)
and the 24-chapter [User Guide](crates/codegen/ainxt-pager/docs/user-guide/).

### Scripting it in CI

```sh
export AINXT_TOKEN=<token>
export AINXT_MAX_RETRIES=2       # default is 15 ≈ 6 minutes of SILENT retries
ainxt -p "review the diff" > out.txt
```

> Set `AINXT_MAX_RETRIES` in every CI job. On an unreachable or misconfigured
> gateway, `ainxt -p` prints nothing on stdout or stderr for the whole retry
> budget — measured at **339 s** with the default — before exiting non-zero. It
> looks like a hang. Also set a job-level timeout.

---

## Troubleshooting

| Symptom | Cause |
|---|---|
| `ainxt -p` prints nothing, seems hung | Retry budget burning silently. Set `AINXT_MAX_RETRIES=2` and `RUST_LOG=ainxt_shell=debug,ainxt_sampler=debug`. |
| 404s on `/ainxt/v1/api/...`, empty model list | `AINXT_GATEWAY_URL` points at an OpenAI-compatible server. Use Route B. |
| `Not signed in … machine with a browser` | Using a local model with no credential. Set `AINXT_API_KEY` to any placeholder. |
| Installer: `could not determine the latest release` | The GitHub API call failed — rate limit, no network, or a private repo needing auth. Retry, pin a version (`install.sh 0.2.101`), download the asset by hand from the Releases page, or set `AINXT_BASE_URL` to your own host. |
| `insufficient_quota` / 429 | The gateway's upstream provider key, not the CLI. |

Full troubleshooting: [`RUN.md` §7](RUN.md).

---

## Publishing a release (maintainers)

The one-liners in Option A need a GitHub Release whose assets are named exactly
as the installers request. Build them:

```sh
scripts/build-release.sh 0.2.101
# -> dist/ainxt-0.2.101-darwin-aarch64
#    dist/ainxt-0.2.101-darwin-x86_64
#    dist/ainxt-0.2.101-linux-x86_64
#    dist/ainxt-0.2.101-linux-aarch64
#    dist/ainxt-0.2.101-win32-x86_64.exe
```

Then, for each artifact, publish a `<artifact>.sha256` beside it:

```sh
cd dist && for f in ainxt-*; do shasum -a 256 "$f" > "$f.sha256"; done
```

and attach everything to a release tagged `v0.2.101`.

**The naming must match exactly.** The installers request
`ainxt-<version>-{darwin|linux|win32}-{aarch64|x86_64}[.exe]`, which is what
`build-release.sh` emits. Renaming to `macos`/`windows` breaks every install.

Nothing about the release is enforced by CI — **no CI workflows ship in this
repository**. Verify the asset names by running an installer against the draft
release before publishing.

Running your own artifact host instead of GitHub Releases? Set `AINXT_BASE_URL`
and serve a flat layout: `<base>/stable` containing the latest version string,
and `<base>/ainxt-<version>-<platform>` for the binaries. For org-internal
deployments use `install-enterprise.sh` / `.ps1`, which require `AINXT_BASE_URL`
and have no default origin at all.
