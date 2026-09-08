# AiNxt CLI User Guide

Learn how to install, configure, and extend AiNxt CLI, the terminal-based AI
coding agent.

> **Derived from upstream.** AiNxt CLI is a fork of
> [Grok Build](https://github.com/xai-org/grok-build) by SpaceXAI / xAI
> (Apache-2.0), and this guide is derived from
> [upstream's user guide](https://github.com/xai-org/grok-build/tree/main/crates/codegen/xai-grok-pager/docs/user-guide):
> chapters 01–24 below correspond to upstream chapters of the same number and
> filename, adapted for this fork's renaming and behavioural changes.
>
> Upstream's full documentation — **[docs.x.ai/build/overview](https://docs.x.ai/build/overview)**
> — remains the reference for behaviour this fork did not change. Read it with
> the names translated: upstream documents the `grok` binary with `XAI_*` /
> `GROK_*` variables; here it is `ainxt` with `AINXT_*` and `~/.ainxt/`.
>
> Where upstream and this guide disagree on **gateway, authentication, model
> catalogue, endpoints or TLS**, this guide is correct — those are the areas the
> fork changed. See [`NOTICE`](../../../../../NOTICE) for the upstream
> attribution and record of modifications. Note also that upstream chapters `25-status-line`,
> `26-config-reference` and `27-grok-clone` are not present here.

---

## Tier 1: Essential User Docs

Start here. These guides cover what you need on your first day.

| # | Document | Description |
|---|----------|-------------|
| 1 | [Getting Started](01-getting-started.md) | Installation, first launch, authentication, basic interaction, and key concepts |
| 2 | [Authentication](02-authentication.md) | Browser login, API keys, OIDC/SSO, external auth providers, and device-code flow |
| 3 | [Keyboard Shortcuts](03-keyboard-shortcuts.md) | Reference for every key binding and mouse action in the TUI |
| 4 | [Slash Commands](04-slash-commands.md) | Every `/` command for sessions, models, memory, hooks, and plugins |
| 5 | [Configuration](05-configuration.md) | `config.toml`, `pager.toml`, environment variables, and file locations |

---

## Tier 2: Core Feature Docs

Customize and extend AiNxt CLI.

| # | Document | Description |
|---|----------|-------------|
| 6 | [Theming and Appearance](06-theming.md) | Themes, the `/theme` command, `pager.toml`, and color-support detection |
| 7 | [MCP Servers](07-mcp-servers.md) | External tool integrations through the Model Context Protocol |
| 8 | [Skills](08-skills.md) | Reusable prompt packages in the SKILL.md format |
| 9 | [Plugins](09-plugins.md) | Bundle and share skills, commands, agents, hooks, and MCP servers; install from marketplace sources |
| 10 | [Hooks](10-hooks.md) | Lifecycle scripts and HTTP callbacks for pre- and post-tool-use events |
| 11 | [Custom Models](11-custom-models.md) | Bring-your-own-key, Ollama, and OpenAI-compatible endpoints |
| 12 | [Project Rules (AGENTS.md)](12-project-rules.md) | Per-directory AGENTS.md instructions and their precedence |
| 13 | [Memory](13-memory.md) | Cross-session knowledge persistence with `/flush`, `/dream`, and hybrid search |

---

## Tier 3: Advanced Usage Docs

Automate, script, and integrate AiNxt CLI with other systems.

| # | Document | Description |
|---|----------|-------------|
| 14 | [Headless Mode and Scripting](14-headless-mode.md) | `ainxt -p`, output formats, CI/CD integration, and piping |
| 15 | [Agent Mode and IDE Integration](15-agent-mode.md) | ACP stdio transport, WebSocket relay, and SDK integration |
| 16 | [Subagents and Personas](16-subagents.md) | Parallel child sessions, agent types, personas, and capability modes |
| 17 | [Session Management](17-sessions.md) | Save, load, resume, rewind, compact, and the session persistence format |
| 18 | [Sandbox Mode](18-sandbox.md) | OS-level filesystem and network isolation profiles |
| 19 | [Plan Mode](19-plan-mode.md) | Structured planning, plan-file edits, and approval before coding |
| 20 | [Background Tasks and Monitoring](20-background-tasks.md) | `background: true`, `/loop`, `monitor`, and `Ctrl+G` to demote |
| 21 | [Terminal Support and Troubleshooting](21-terminal-support.md) | tmux, SSH, truecolor, clipboard, and OSC 52 |
| 22 | [Permissions and Safety Controls](22-permissions-and-safety.md) | `dontAsk` mode, auto-approved tools, the safe-bash list, and restrictive PreToolUse hooks (such as git/gh-only) |
