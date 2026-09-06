# AINXT.md

This file provides guidance to AiNxt CLI agents when working with code in this
repository.

> **It is not loaded automatically under this name.** The agent reads project
> rules from `Agents.md`, `AGENTS.md`, `AGENT.md`, `Claude.md`, `CLAUDE.md` or
> `CLAUDE.local.md` — `AINXT.md` is not in that list and appears nowhere in the
> source. To have these rules picked up automatically, copy or symlink this file
> to `AGENTS.md`. See
> [Project rules](crates/codegen/ainxt-pager/docs/user-guide/12-project-rules.md)
> for the full resolution order.

> **Note:** This file was rewritten on 2026-07-23 (OSS-004). The previous version
> described a Bun + TypeScript project that does not exist. The actual codebase is
> described below.

---

## Project Overview

**AiNxt CLI** — a terminal-native AI coding assistant and agentic engineering platform.
Delivered as a single self-contained binary (`ainxt`). The product is a fork of
SpaceXAI's "Grok Build" CLI (Apache-2.0), rebranded and extended for the AiNxt platform.

| Fact | Value |
|------|-------|
| Language | **Rust** (edition 2024) |
| Build system | **Cargo** workspace |
| Crate count | ~80 first-party crates |
| Source files | ~2,172 `.rs` files |
| Dependencies | ~1,269 (via `Cargo.lock`) |
| License | Apache-2.0 |
| Binary name | `ainxt` |
| Config dir | `~/.ainxt/` |
| Env prefix | `AINXT_*` |

There is **no** `src-v2/` directory, no TypeScript, no Bun, no React/Ink, and no
`package.json` in this repository.

---

## Workspace Layout

```
ainxt-cli/
├── Cargo.toml                  # workspace root (resolver = "2")
├── Cargo.lock
├── deny.toml                   # cargo-deny license + advisory policy
├── rust-toolchain.toml         # pinned Rust toolchain (rustup auto-installs)
├── bin/protoc                  # DotSlash descriptor — downloads protoc on demand
├── crates/
│   ├── build/
│   │   └── ainxt-proto-build/  # build script: compiles .proto files
│   ├── codegen/                # main product crates (see §Crate Map)
│   └── common/                 # shared utility crates
├── third_party/                # vendored crates (mermaid-to-svg, dagre_rust, etc.)
├── prod/                       # production-only crates (e.g. cli-chat-proxy-types)
├── assets/                     # brand assets (logo, etc.)
├── scripts/                    # release scripts (build-release.sh, etc.)
└── NOTICE, LICENSE, THIRD-PARTY-NOTICES, deny.toml
```

---

## Key Crates (codegen/)

| Crate | Role |
|-------|------|
| `ainxt-pager-bin` | **Main binary entry point** — builds the `ainxt` executable |
| `ainxt-pager` | TUI shell: Ratatui-based terminal UI, REPL, rendering pipeline |
| `ainxt-agent` | Agent builder, definition parsing, system prompt assembly |
| `ainxt-agent-lifecycle` | Agent run lifecycle, turn management, compaction |
| `ainxt-sampler` | HTTP sampling client — streams from the gateway (SSE/JSON) |
| `ainxt-config` / `ainxt-config-types` | 8-source settings cascade, validation |
| `ainxt-env` | Centralized default endpoint constants (all env-overridable) |
| `ainxt-auth` | Bearer-token login/logout, credential storage (`~/.ainxt/credentials.json`) |
| `ainxt-hooks` | Lifecycle hooks (PreToolUse, PostToolUse, SessionStart, …) |
| `ainxt-tools` | Built-in tool implementations (file I/O, shell, git, web, …) |
| `ainxt-mcp` | Model Context Protocol client + server (stdio/SSE/HTTP/WS) |
| `ainxt-secrets` | Secret sanitizer — redacts keys/tokens from logs and output |
| `ainxt-telemetry` | Telemetry (disabled by default; no hardcoded token) |
| `ainxt-plugin-marketplace` | Plugin discovery, index, and install pipeline |
| `ainxt-memory` | Per-project workspace memory store |
| `ainxt-markdown` / `ainxt-markdown-core` | Markdown rendering for the TUI |
| `ainxt-mermaid` | Mermaid diagram rendering (SVG via vendored JS) |
| `ainxt-sandbox` | Sandboxed shell execution |
| `ainxt-update` | Auto-update check and download |
| `ainxt-workspace` / `ainxt-workspace-types` | Workspace detection and metadata |
| `ainxt-http` | Shared HTTP client (reqwest wrapper, TLS policy, SSRF defense) |
| `ainxt-gix-status` | Git status via `gix` (pure-Rust libgit2 alternative) |
| `ainxt-codebase-graph` | Codebase graph construction for semantic search |
| `ainxt-crash-handler` | Crash reporting and recovery |
| `ainxt-tracing-macros` | Tracing/logging macros |
| `ptyctl` / `ptyctl-cli` | PTY control for shell sessions |

---

## Build Commands

```sh
# Fast validation (no codegen, no link)
cargo check -p ainxt-pager-bin

# Debug build
cargo build -p ainxt-pager-bin --bin ainxt

# Release binary → target/release-dist/ainxt
cargo build --profile release-dist -p ainxt-pager-bin --bin ainxt

# Run directly
./target/release-dist/ainxt
```

> The Rust toolchain is pinned in `rust-toolchain.toml`. `rustup` installs it
> automatically on first build. `protoc` is provided via the DotSlash descriptor
> at `bin/protoc` — install `dotslash` and put it on `PATH` before building.

---

## Test Commands

```sh
# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p ainxt-sampler

# Run a specific test
cargo test -p ainxt-secrets -- sanitizer

# Clippy (lint)
cargo clippy --workspace --all-targets -- -D warnings

# Format check
cargo fmt --check

# Format (auto-fix)
cargo fmt
```

---

## Supply-Chain Gates (run before any release)

```sh
# Install tools (once)
cargo install cargo-deny
cargo install cargo-audit

# License compliance (policy in deny.toml)
cargo deny check licenses

# Security advisories (checks Cargo.lock vs RustSec DB)
cargo audit

# All deny checks at once
cargo deny check
```

See `RUN.md §8` for full details and policy explanation.

---

## Key Environment Variables

| Variable | Effect |
|----------|--------|
| `AINXT_GATEWAY_URL` | Override the backend gateway URL |
| `AINXT_CONFIG_DIR` | Override config dir (default `~/.ainxt`) |
| `AINXT_ALLOW_INSECURE` | Set to `1` to allow plain HTTP (testing only) |
| `AINXT_TELEMETRY_ENABLED` | Set to `1` to opt in to telemetry (off by default) |
| `AINXT_MARKETPLACE_SOURCE_URL` | Override the plugin marketplace Git URL |
| `AINXT_MARKETPLACE_ORG` | Override the marketplace org slug |
| `RUST_LOG` | Tracing filter, e.g. `ainxt_sampler=debug,ainxt_agent=info` |

All default production endpoints are defined in `crates/codegen/ainxt-env/src/lib.rs`
and are overridable via environment variables — no endpoint is hardcoded without an
escape hatch.

---

## Important Conventions

- **No TypeScript, no Bun, no Node.** This is a pure Rust project.
- **Workspace dependencies** are declared in the root `Cargo.toml` `[workspace.dependencies]`
  section and referenced with `{ workspace = true }` in crate `Cargo.toml` files.
- **Secret sanitizer** (`ainxt-secrets/src/sanitizer.rs`) must be applied to any output
  that could contain user credentials. Never log raw API keys or tokens.
- **TLS is secure by default.** Plain HTTP is refused unless `AINXT_ALLOW_INSECURE=1`.
- **Telemetry is off by default.** Do not add always-on telemetry calls.
- **SSRF defense** is active in `ainxt-hooks/src/runner/http.rs` — RFC-1918 private IP
  ranges are blocked for outbound hook HTTP calls.
- **License:** all first-party crates are `Apache-2.0`. Do not add
  GPL/AGPL/SSPL dependencies without legal review. MPL-2.0 requires sign-off (see `deny.toml`).
- **Fork attribution:** `NOTICE` and `THIRD-PARTY-NOTICES` must be kept up to date.
  Do not remove the SpaceXAI/Grok Build attribution.

---

## Key File Locations

| Area | Path |
|------|------|
| Main binary entry | `crates/codegen/ainxt-pager-bin/src/main.rs` |
| Default endpoints (env-overridable) | `crates/codegen/ainxt-env/src/lib.rs` |
| Marketplace slug | `crates/codegen/ainxt-plugin-marketplace/src/lib.rs` |
| Secret sanitizer | `crates/codegen/ainxt-secrets/src/sanitizer.rs` |
| Telemetry config | `crates/codegen/ainxt-telemetry/src/config.rs` |
| SSRF blocklist | `crates/codegen/ainxt-hooks/src/runner/http.rs` |
| Auth / credentials | `crates/codegen/ainxt-auth/src/` |
| License gate policy | `deny.toml` |
| Build + run guide | `RUN.md` |

