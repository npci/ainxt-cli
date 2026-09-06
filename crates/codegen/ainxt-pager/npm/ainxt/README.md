# Ainxt

Bring ainxt into your terminal. Fast, flicker-free CLI built for plans, subagents, and parallel work.

**[Source and documentation](https://github.com/npci/ainxt-cli)** — there is no
separate website; the repository is the homepage.

## Install

```bash
# See the repository's Releases page for platform binaries, or build from source.
```

Or install with npm:

```bash
npm i -g @ainxt/ainxt
```

## Get Started

```bash
# Launch the interactive TUI
ainxt

# Run a single task
ainxt -p "Explain this codebase"
```

On first launch Ainxt authenticates against the gateway named by
`AINXT_GATEWAY_URL`. For CI or headless environments set `AINXT_API_KEY` — a key
issued by your own gateway, or your model provider's own key. There is no
hosted AiNxt console:

```bash
export AINXT_API_KEY="ainxt-..."
```

## Update

```bash
ainxt update
```

Or if installed via npm:

```bash
npm i -g @ainxt/ainxt@latest
```

## Supported Platforms

| Platform | Architecture |
|---|---|
| macOS | Apple Silicon (arm64) |
| Linux | x86_64, arm64 |
| Windows | x86_64 |

## Documentation

Full documentation lives in the repository, not on a website: see the 24-chapter
**User Guide** under `crates/codegen/ainxt-pager/docs/user-guide/` — configuration,
MCP servers, custom models, headless mode, agent mode and more.

## Feedback

Run `/feedback` inside Ainxt to report issues or send feedback directly.
