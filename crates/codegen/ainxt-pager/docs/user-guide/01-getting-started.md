# Getting Started

ainxt is a terminal-based AI coding agent. It runs as a TUI (Terminal User Interface) that understands your codebase, executes shell commands, edits files, searches the web, and manages tasks.

You can use it interactively as a full-screen TUI, run it headlessly for scripting and CI/CD, or integrate it into editors via the Agent Client Protocol (ACP).

> **ainxt does not bundle any AI models.** It connects to a gateway you point it
> at. You need a gateway URL and a bearer token before the CLI is useful.

---

## Step 1 — Get the binary

### Option A — One-line installer

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/npci/ainxt-cli/main/crates/codegen/ainxt-pager/scripts/install.sh | bash
```

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/npci/ainxt-cli/main/crates/codegen/ainxt-pager/scripts/install.ps1 | iex
```

Detects your platform, downloads the matching binary from this repository's
GitHub Releases, verifies a published `.sha256` when present, and installs to
`~/.ainxt/bin/ainxt`. Add `AINXT_REQUIRE_CHECKSUM=1` to refuse anything it
cannot verify.

> **Needs a published GitHub Release, and none exists yet.** Until one does, use
> Option B. See [`INSTALL.md`](../../../../../INSTALL.md) for every install route.

### Option A2 — Download a binary by hand

Releases publish **bare binaries**, not archives. Take the one for your platform
from the **Releases** page, check it against the published `.sha256`, install it:

```bash
# Linux / macOS
shasum -a 256 -c ainxt-<version>-<os>-<arch>.sha256
sudo install -m 0755 ainxt-<version>-<os>-<arch> /usr/local/bin/ainxt
```

```powershell
# Windows (PowerShell)
Get-FileHash .\ainxt-<version>-win32-x86_64.exe -Algorithm SHA256   # compare with the .sha256
```

Names are `ainxt-<version>-{darwin|linux|win32}-{aarch64|x86_64}[.exe]`.
Always verify the checksum before you make anything executable.

Verify:
```bash
ainxt --version
```

### Option B — Build from source

If you cloned the repository and want to build the binary yourself:

```bash
# Prerequisites: Rust (rustup.rs) + DotSlash (cargo install dotslash)
# No gateway or token needed to build — just Rust.

# Debug build — fast to compile, use for development
cargo build -p ainxt-pager-bin --bin ainxt
./target/debug/ainxt --version

# Release build — use for real work
cargo build --profile release-dist -p ainxt-pager-bin --bin ainxt
./target/release-dist/ainxt --version

# Optional: copy to PATH
cp target/release-dist/ainxt ~/.local/bin/ainxt
```

> The build stamps the git commit hash into `--version` automatically.
> Everything else (gateway URL, telemetry, policy) uses safe defaults
> that you override at runtime — no rebuild needed.

See the [repository README](../../../../../README.md) for the full build guide.

---

## Step 2 — Point it at a gateway

ainxt needs a gateway URL. Set it before launching:

```bash
export AINXT_GATEWAY_URL=https://your-gateway.example.com
```

> **What is a gateway?** The gateway provides the model catalog, handles auth,
> and routes prompts to AI models. See
> [`env.example`](../../../../../env.example) for every option, including how to
> point the CLI at a self-hosted or local gateway.

---

## Step 3 — Authenticate

```bash
ainxt login
# Paste your bearer token when prompted.
```

For non-interactive / CI use, skip login entirely:
```bash
export AINXT_TOKEN=<your-bearer-token>
```

See [Authentication](02-authentication.md) for OAuth2/OIDC, external auth providers, and device code flow.

---

## Step 4 — Launch

```bash
ainxt                          # full-screen TUI
ainxt -p "explain this repo"   # headless, one-shot
ainxt -m <model-id> -p "hello" # specific model
ainxt -c                       # continue last session
```

---

## Basic Interaction

Once authenticated, ainxt presents a full-screen TUI with two main areas:

- **Scrollback** -- the conversation history showing your prompts, ainxt's responses, tool calls, file edits, and more.
- **Prompt** -- the input area at the bottom where you type messages.

Type a message and press `Enter` to send it. ainxt reads files, runs commands, and edits code as needed. Each tool run streams into the scrollback in real time.

Press `Tab` to move focus between the prompt and the scrollback. While a turn is running, `Ctrl+C` cancels it (or clears a non-empty draft first); `Esc` is a no-op mid-turn. Idle, press `Esc` twice within 800ms to clear a non-empty prompt, or (with an empty prompt and conversation messages) to open rewind — see [Keyboard Shortcuts](03-keyboard-shortcuts.md#escape). With the scrollback focused, use the arrow keys to select entries and to collapse or expand them. To navigate with `j`/`k` and fold with `h`/`l` instead, enable Vim mode.

### File References

Use `@` in your prompt to attach files:

```
@src/main.rs              # Attach a file
@src/main.rs:10-50        # Attach lines 10-50
@src/                     # Browse a directory
```

The `@` operator opens a fuzzy file picker. By default it respects `.gitignore` and hides dotfiles. Prefix with `!` to search hidden files:

```
@!.github                 # Search hidden files
@!.env                    # Attach a .env file
```

### Permissions

By default, ainxt asks for permission before executing shell commands or editing files. You can approve individually or toggle always-approve mode:

- Press `Ctrl+O` to toggle always-approve mode
- Use the `--yolo` flag at launch: `ainxt --yolo`
- Type `/always-approve` in the prompt to toggle the mode

---

## Key Concepts

### Sessions

Every conversation is a **session**. Sessions are automatically saved to `~/.ainxt/sessions/` and can be resumed later. Each session tracks the full conversation history, tool calls, file edits, and task state.

- Start a new session: `Ctrl+N` or `/new`
- Resume a previous session: `/resume` in the TUI, or `--resume <ID>` from the CLI
- Continue the most recent session: `ainxt -c`

### Scrollback

The scrollback is the main display area. It shows:

- **User prompts** -- your messages, rendered as sticky headers
- **Agent messages** -- ainxt's responses with full markdown rendering and syntax highlighting
- **Thinking blocks** -- ainxt's reasoning process (collapsible)
- **Tool calls** -- file edits (with inline diffs), command executions, search results, and more
- **Task lists** -- TODO items tracking progress

Collapse or expand the selected entry with the `Left`/`Right` arrow keys (or `h`/`l` and `e` in Vim mode). In Vim mode, press `y` to copy its content and `Y` to copy its metadata (for example, the command that ran). Press `Enter` to open it in the fullscreen viewer (in any mode).

### Tools

ainxt has built-in tools for:

| Tool | Description |
|------|-------------|
| `read_file` / `search_replace` | Read and edit files with line-precise changes |
| `grep` | Regex search across your codebase (powered by ripgrep) |
| `list_dir` | List directory contents |
| `run_terminal_command` | Execute shell commands |
| `web_search` / `web_fetch` | Search the web and fetch URLs |
| `todo_write` | Create and manage task lists |
| `spawn_subagent` | Spawn parallel subagent sessions |
| `memory_search` | Search cross-session memory |

Tools can be extended with [MCP servers](05-configuration.md#mcp-servers) for integrations like GitHub, databases, and more.

### Slash Commands

Type `/` in the prompt to access commands. These provide quick actions without writing a full prompt:

```
/model ainxt-build                 # Switch model
/compact                          # Compress conversation history
/always-approve                   # Toggle always-approve mode
/new                              # Start a new session
```

See [Slash Commands](04-slash-commands.md) for the complete reference.

---

## Common Launch Options

```bash
# Launch the interactive TUI and submit an initial prompt as the first turn
ainxt "fix the failing auth test and run it"

# Initial prompt in a new git worktree. Use --worktree=<name> (with `=`) so the
# prompt isn't swallowed as the worktree name — `ainxt -w "refactor module X"`
# would treat "refactor module X" as the worktree label, not the prompt.
ainxt --worktree=feat "refactor module X"

# Base the worktree on a specific branch (e.g. main) instead of the current HEAD:
ainxt -w --ref main "implement feature from main"


# Start in a specific project directory
ainxt --cwd ~/projects/my-app

# Add project-specific rules
ainxt --rules "Always use TypeScript. Prefer functional components."

# Auto-approve all tool executions
ainxt --yolo

# Use a specific model
ainxt -m ainxt-build

# Resume a previous session
ainxt --resume <session-id>

# Continue the most recent session
ainxt -c

# Experimental scrollback-native render mode. Sticky: plain `ainxt` reopens in
# the mode last chosen via --minimal/--fullscreen (or /minimal//fullscreen).
ainxt --minimal

# Back to the standard fullscreen TUI (and make it sticky again)
ainxt --fullscreen

# Headless mode (for scripts)
ainxt -p "Explain this codebase"
```

---

## Headless Mode

Run ainxt non-interactively for scripting, CI/CD, and automation:

```bash
ainxt -p "Your prompt here"
```

Output formats:

| Format | Flag | Description |
|--------|------|-------------|
| `plain` | (default) | Human-readable text |
| `json` | `--output-format json` | Single JSON object with `text`, `stopReason`, `sessionId`, and `requestId` |
| `streaming-json` | `--output-format streaming-json` | NDJSON event stream for real-time processing |

Example CI/CD usage:

```bash
ainxt -p "Review changes for bugs" --output-format json --yolo | jq -r '.text'
```

---

## Project Rules (AGENTS.md)

Add per-project instructions by creating an `AGENTS.md` file in your repository. ainxt reads these files and injects their contents as a project-instructions message at the start of the conversation:

```
~/.ainxt/AGENTS.md           # Global rules (apply to all projects)
<repo-root>/AGENTS.md       # Repository-level rules
<cwd>/AGENTS.md             # Directory-level rules (highest priority)
```

Deeper files take precedence. ainxt also reads `CLAUDE.md` files for compatibility.

---

## Where to Go Next

| Document | What You Will Learn |
|----------|-------------------|
| [Authentication](02-authentication.md) | Browser login, API keys, OIDC, external auth, device code flow |
| [Keyboard Shortcuts](03-keyboard-shortcuts.md) | Complete reference for all key bindings |
| [Slash Commands](04-slash-commands.md) | All available `/` commands |
| [Configuration](05-configuration.md) | config.toml, pager.toml, environment variables |
