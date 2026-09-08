# Configuration

The CLI is configured through environment variables and an optional TOML file.
There is **one canonical, fully-annotated reference** for every environment
variable it reads:

> ### [`env.example`](env.example)

## Quick start

```sh
cp env.example .env
# Set AINXT_GATEWAY_URL to your gateway address.
# Set AINXT_TOKEN to your token (or leave blank and run `ainxt login`).
source .env
```

Only two settings are needed to get running — `AINXT_GATEWAY_URL` and
`AINXT_TOKEN`. Everything else has a working default.

## Configuration priority

Highest to lowest — a value set at a higher level wins:

1. Command-line flags
2. Environment variables (`AINXT_GATEWAY_URL`, `AINXT_TOKEN`, `AINXT_URL_*`, …)
3. `~/.ainxt/config.toml` — models, UI preferences, features, MCP servers
4. Built-in defaults

The gateway is the single most important setting: setting `AINXT_GATEWAY_URL`
derives the API paths automatically, so you rarely need to set the others.

## Defaults worth knowing

| Setting | Default | Notes |
|---|---|---|
| Gateway | `localhost:8000` | The CLI bundles no models and no backend |
| Telemetry | **off** | Opt-in only (`AINXT_TELEMETRY_ENABLED=1`). When enabled, event data (e.g. permission decisions, tool-usage patterns) is sent to **Mixpanel** (`api.mixpanel.com`) — this destination is currently fixed, not configurable. String values are redacted for secrets before sending, but the vendor and destination were previously undocumented; noted here for informed opt-in. |
| Policy | permissive | Tighten per deployment |
| Config home | `~/.ainxt` | Override with `AINXT_HOME` |

## See also

- [`env.example`](env.example) — every variable, annotated
- [`RUN.md`](RUN.md) — running the CLI
- `~/.ainxt/config.toml` — models, UI, MCP servers

---

> **Why this file is short.** It previously contained a full copy of the
> environment-variable reference, which meant the same content lived in two
> places and drifted. `env.example` is now the single source of truth; this file
> orients you to it.
