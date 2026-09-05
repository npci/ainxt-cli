# Plan: Exposing AiNxt CLI as a REST Service

> ## Status: DESIGN PROPOSAL — NOT IMPLEMENTED
>
> This document is a design study, not documentation of shipped behaviour.
> **None of the REST endpoints, and no `ainxt-rest-server` crate, exist in this
> repository** — the crate is not in `Cargo.toml` and there is no such directory
> under `crates/`. Nothing here can be built, called or relied upon today.
>
> What *is* real and shipping, and may be what you actually want:
>
> | Need | Use |
> |---|---|
> | One-shot scripting / CI | `ainxt -p "prompt"` — see [Headless mode](crates/codegen/ainxt-pager/docs/user-guide/14-headless-mode.md) |
> | Editor / IDE integration | `ainxt agent stdio` (ACP over stdio) |
> | Structured output for tooling | `ainxt -p … --output-format json` |
>
> Sections 1–3 below are an accurate survey of the existing architecture and are
> useful reading. Everything from section 4 ("Proposed Architecture") onward
> describes work that has not been done.
>
> It is kept in the repository because the architecture survey has value and the
> proposal records design intent. Treat it as an RFC.

## 1. Codebase Architecture Summary

After a thorough exploration of all 60+ crates, here is the actual runtime architecture:

```
ainxt-pager-bin (main.rs)
  └─ ainxt-pager (TUI: ratatui + crossterm)
       └─ ACP channel (agent-client-protocol mpsc)
            └─ ainxt-shell (MvpAgent + AcpSession)
                 ├─ ainxt-agent (Agent definition, ToolBridge)
                 ├─ ainxt-tools (bash, read_file, grep, web_fetch, …)
                 ├─ ainxt-mcp (MCP server integration)
                 ├─ ainxt-sampler (LLM HTTP streaming)
                 ├─ ainxt-chat-state (conversation actor)
                 └─ ainxt-workspace (FS, VCS, permissions, Hub WS)
```

The binary already supports multiple modes:

| Mode | Command | Description |
|---|---|---|
| Interactive TUI | `ainxt` | ratatui + crossterm terminal UI |
| Headless single-turn | `ainxt -p "prompt"` | stdout streaming, no TUI |
| ACP over stdio | `ainxt agent stdio` | raw ACP protocol on stdin/stdout |
| Headless agent | `ainxt agent headless` | non-interactive agent |
| Agent server (leader) | `ainxt agent serve` | multi-session leader process |
| Workspace lifecycle | `ainxt workspace start/stop/…` | workspace management |

---

## 2. TUI Decision-Making Capabilities (Must Be Preserved in REST)

The TUI is **not just a display layer** — it contains significant decision-making logic that must be replicated or bridged in the REST service.

### 2.1 Dispatch Router (`dispatch/router.rs`)

A pure synchronous state machine: `dispatch(action, app) → Vec<Effect>`. Key decisions:

| Decision Point | Logic |
|---|---|
| **Slash command routing** | `/` prefix → command registry lookup → `CommandResult` (Handled / PassThrough / QueueCommand / InjectSkill) |
| **Prompt queuing vs. immediate send** | `immediate_server_send_eligible()` → direct ACP send vs. local queue drain |
| **Exit alias detection** | `"exit"`, `"quit"`, `":q"` → `Action::Quit` |
| **Reconnect guard** | `reconnect_pending?` → block prompt, show toast |
| **Project picker** | `needs_project_picker()` → open project selection modal |
| **Mode gating** | Restricted commands → upsell modal |

### 2.2 Permission Handling (`dispatch/permissions.rs`)

- `PermissionSelect(option_id)` — user approves/denies a tool call
- `PermissionFollowup(text)` — user provides follow-up text for a permission
- `PermissionCancel` — user cancels a pending permission
- **REST must expose these as endpoints** since the agent **blocks** waiting for permission responses

### 2.3 Mode Switching (`dispatch/modes.rs`)

- `CycleMode` — Normal → Plan → AlwaysApprove → Normal
- `SetPlanMode(On|Off)` — plan mode toggle
- `SetPermissionMode(Default|Ask|Auto|AlwaysApprove)` — permission mode
- `SetYoloMode` — yolo (auto-approve all) mode

### 2.4 Queue Management (`dispatch/prompt.rs`)

- Server-authoritative prompt queue with versioned entries (`QueueEntryWire`)
- Operations: remove, reorder, clear, edit, interject
- `running_prompt_id` tracks the currently executing prompt

### 2.5 ACP Extension Notifications (Inbound from Agent)

The TUI handles 20+ `ainxt.dev/*` extension notifications. REST clients need SSE/WebSocket to receive:

| Notification | Purpose |
|---|---|
| `ainxt.dev/session_notification` | Turn progress, tool calls, text deltas |
| `ainxt.dev/queue/changed` | Queue state updates |
| `ainxt.dev/follow_ups` | Suggested follow-up chips |
| `ainxt.dev/ask_user_question` | Agent asking user a question (blocks turn) |
| `ainxt.dev/exit_plan_mode` | Plan mode exit signal |
| `ainxt.dev/monitor_event` | Background monitor events |
| `ainxt.dev/scheduled_task_*` | Scheduler events |
| `ainxt.dev/models/update` | Model catalog refresh |
| `ainxt.dev/mcp/*` | MCP server status changes |

### 2.6 Multi-Agent Dashboard

- `OpenDashboard` / `DashboardDispatch` / `DashboardPeekReply` — orchestrator ↔ subagent routing
- `DashboardPermissionSelect` — approve permissions for a specific subagent row
- REST must expose per-subagent endpoints or a multiplexed stream

---

## 3. Existing REST-Adjacent Infrastructure

### 3.1 Headless Mode (Lowest-Friction Path)

`crates/codegen/ainxt-pager/src/headless.rs` already implements:
1. `spawn_ainxt_shell()` — spawns the shell process
2. ACP lifecycle: `Initialize → Authenticate → NewSession → Prompt`
3. Streams `SessionNotification` events to stdout in `Plain | Json | StreamingJson` format
4. Clean shutdown via `CancellationToken`

**This is the foundation for the REST service.**

### 3.2 `ainxt-http` Crate

- `shared_client()` — pooled HTTP/2 reqwest client
- `send_with_retry_escaping_pool()` — retry with pool escape
- `with_auth_retry()` — 401 auto-retry middleware
- Custom headers: `x-ainxt-conv-id`, `x-ainxt-req-id`, `x-ainxt-session-id`, etc.

### 3.3 `prod/mc/cli-chat-proxy-types`

Already-defined REST wire types for:
- Session CRUD (`RegisterSessionRequest`, `UpdateSessionRequest`, `SessionReplicaResponse`)
- Storage (signed GCS upload URLs, batch upload)
- Feedback (`FeedbackSubmission`, `SessionSignalsUpdate`)
- Deployment config, metadata, subagent bundles

### 3.4 `ainxt-tools-api` Proto

A `.proto` file exists in `crates/codegen/ainxt-tools-api/` — the tool API is already protobuf-defined, meaning **gRPC is also a viable transport** alongside REST.

### 3.5 Axum Already in Workspace

`axum = "0.8"` is already a dependency in the workspace `Cargo.toml`. **No new HTTP framework needs to be added.**

---

## 4. Proposed Architecture: `ainxt-rest-server` Crate

### 4.1 New Crate Structure

```
crates/codegen/ainxt-rest-server/
  Cargo.toml
  src/
    main.rs          ← binary entry point (or integrate into ainxt-pager-bin)
    lib.rs           ← library root
    server.rs        ← Axum router setup, startup
    state.rs         ← AppState (shared across handlers)
    auth.rs          ← API key / Bearer token middleware
    error.rs         ← unified ApiError → HTTP response
    routes/
      sessions.rs    ← /api/sessions CRUD
      prompt.rs      ← /api/sessions/{id}/prompt (POST, streaming)
      queue.rs       ← /api/sessions/{id}/queue operations
      modes.rs       ← /api/sessions/{id}/mode, /model, /permission-mode
      permissions.rs ← /api/sessions/{id}/permissions (approve/deny)
      mcps.rs        ← /api/sessions/{id}/mcps CRUD
      stream.rs      ← /api/sessions/{id}/stream (SSE)
      health.rs      ← /health, /ready
    bridge/
      acp_bridge.rs  ← ACP ↔ REST translation layer
      session_mgr.rs ← per-session ACP connection pool
      event_fan.rs   ← fan-out AcpClientMessage to SSE subscribers
```

### 4.2 Shared State (`AppState`)

```rust
pub struct AppState {
    // One ACP connection per active session
    sessions: Arc<DashMap<SessionId, SessionHandle>>,
    // Auth manager (reuse ainxt-auth)
    auth_manager: Arc<AuthManager>,
    // Config (reuse ainxt-config)
    config: Arc<EffectiveConfig>,
    // REST API key for authenticating REST clients
    api_key: Option<String>,
}

pub struct SessionHandle {
    pub agent_tx: AcpAgentTx,                      // send to agent
    pub event_tx: broadcast::Sender<RestEvent>,     // fan-out to SSE subscribers
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub state: Arc<RwLock<SessionState>>,
}

pub struct SessionState {
    pub mode: PermissionModeKind,
    pub plan_mode: bool,
    pub model_id: String,
    pub queue: Vec<QueueEntryWire>,
    pub pending_permissions: Vec<PendingPermission>,
    pub turn_active: bool,
}
```

### 4.3 REST Event Type (SSE payload)

```rust
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RestEvent {
    // Text streaming
    TextDelta { text: String },
    // Tool call lifecycle
    ToolCallStart { tool_call_id: String, tool_name: String, args: Value },
    ToolCallProgress { tool_call_id: String, text: String },
    ToolCallComplete { tool_call_id: String, output: Value },
    // Turn lifecycle
    TurnStart { turn_id: String },
    TurnComplete { turn_id: String, usage: Option<UsageSummary> },
    TurnError { message: String },
    TurnCancelled,
    // Permission gate (blocks turn until resolved)
    PermissionRequest {
        request_id: String,
        tool_name: String,
        description: String,
        options: Vec<PermissionOption>,
    },
    // Queue updates
    QueueChanged { entries: Vec<QueueEntryWire>, running_prompt_id: Option<String> },
    // Agent asking user a question
    AskUserQuestion { question_id: String, question: String, options: Vec<String> },
    // Follow-up suggestions
    FollowUps { suggestions: Vec<String> },
    // Plan mode
    PlanModeExited,
    // Monitor events
    MonitorEvent { monitor_id: String, line: String },
    // Error
    Error { code: String, message: String },
    // Heartbeat
    Ping,
}
```

---

## 5. REST API Endpoints

### 5.1 Session Management

| Method | Path | Description | Maps To |
|---|---|---|---|
| `POST` | `/api/sessions` | Create new session | `Effect::CreateSession` |
| `GET` | `/api/sessions` | List sessions | `Effect::FetchSessionList` |
| `GET` | `/api/sessions/{id}` | Get session info | ACP `LoadSession` + state |
| `DELETE` | `/api/sessions/{id}` | Delete session | `Effect::DeleteSession` |
| `POST` | `/api/sessions/{id}/fork` | Fork session | `Effect::ForkSession` |
| `PATCH` | `/api/sessions/{id}/rename` | Rename session | `Effect::RenameSession` |

**Create Session Request:**
```json
{
  "cwd": "/path/to/project",
  "model_id": "claude-opus-4-7",
  "agent_id": "default",
  "chat_kind": "chat"
}
```

**Create Session Response:**
```json
{
  "session_id": "sess_abc123",
  "agent_id": "agent_xyz",
  "created_at": "2026-07-24T10:00:00Z",
  "stream_url": "/api/sessions/sess_abc123/stream"
}
```

### 5.2 Prompt / Turn Control

| Method | Path | Description | Maps To |
|---|---|---|---|
| `POST` | `/api/sessions/{id}/prompt` | Send prompt (streaming SSE) | `Effect::SendPrompt` |
| `POST` | `/api/sessions/{id}/prompt/now` | Cancel current + send | `Effect::SendPromptNow` |
| `POST` | `/api/sessions/{id}/bash` | Send bash command | `Effect::SendBashCommand` |
| `POST` | `/api/sessions/{id}/interject` | Inject mid-turn | `Effect::Interject` |
| `DELETE` | `/api/sessions/{id}/turn` | Cancel current turn | `Effect::CancelTurn` |
| `POST` | `/api/sessions/{id}/compact` | Trigger compaction | `Effect::Compact` |
| `POST` | `/api/sessions/{id}/recap` | Request recap | `Effect::SendRecap` |

**Send Prompt Request:**
```json
{
  "text": "Refactor the auth module to use JWT",
  "images": [],
  "stream": true
}
```

**Response (SSE stream when `stream: true`):**
```
data: {"type":"turn_start","turn_id":"turn_001"}
data: {"type":"text_delta","text":"I'll start by examining"}
data: {"type":"tool_call_start","tool_call_id":"tc_1","tool_name":"read_file","args":{"target_file":"src/auth.rs"}}
data: {"type":"tool_call_complete","tool_call_id":"tc_1","output":{"content":"..."}}
data: {"type":"text_delta","text":"Based on the code, here's my plan..."}
data: {"type":"turn_complete","turn_id":"turn_001","usage":{"input_tokens":1200,"output_tokens":450}}
```

**Response (JSON when `stream: false`):**
```json
{
  "turn_id": "turn_001",
  "text": "I'll refactor the auth module...",
  "tool_calls": [],
  "usage": { "input_tokens": 1200, "output_tokens": 450 }
}
```

### 5.3 Permission Handling (Critical — Blocks Turn)

| Method | Path | Description | Maps To |
|---|---|---|---|
| `GET` | `/api/sessions/{id}/permissions` | List pending permissions | `SessionState.pending_permissions` |
| `POST` | `/api/sessions/{id}/permissions/{req_id}/approve` | Approve with option | `Action::PermissionSelect` |
| `POST` | `/api/sessions/{id}/permissions/{req_id}/followup` | Provide followup text | `Action::PermissionFollowup` |
| `DELETE` | `/api/sessions/{id}/permissions/{req_id}` | Cancel permission | `Action::PermissionCancel` |

**Approve Permission Request:**
```json
{ "option_id": "allow_once" }
```

### 5.4 Queue Management

| Method | Path | Description | Maps To |
|---|---|---|---|
| `GET` | `/api/sessions/{id}/queue` | Get current queue | `QueueChanged` state |
| `DELETE` | `/api/sessions/{id}/queue/{qid}` | Remove entry | `Effect::QueueRemove` |
| `PUT` | `/api/sessions/{id}/queue/{qid}` | Edit entry text | `Effect::QueueEdit` |
| `POST` | `/api/sessions/{id}/queue/reorder` | Reorder entries | `Effect::QueueReorder` |
| `DELETE` | `/api/sessions/{id}/queue` | Clear all entries | `Effect::QueueClear` |
| `POST` | `/api/sessions/{id}/queue/{qid}/interject` | Interject into running turn | `Effect::QueueInterject` |

### 5.5 Mode Control

| Method | Path | Description | Maps To |
|---|---|---|---|
| `PUT` | `/api/sessions/{id}/mode` | Set permission mode | `Effect::SetSessionMode` |
| `POST` | `/api/sessions/{id}/plan-mode/toggle` | Toggle plan mode | `Effect::TogglePlanMode` |
| `PUT` | `/api/sessions/{id}/model` | Switch model | `Effect::SwitchModel` |

**Set Mode Request:**
```json
{ "mode": "always_approve" }
```
Options: `"default"` | `"ask"` | `"auto"` | `"always_approve"`

### 5.6 MCP Server Management

| Method | Path | Description | Maps To |
|---|---|---|---|
| `GET` | `/api/sessions/{id}/mcps` | List MCP servers | `Effect::FetchMcpsList` |
| `POST` | `/api/sessions/{id}/mcps` | Add/update MCP server | `Effect::UpsertMcpServer` |
| `DELETE` | `/api/sessions/{id}/mcps/{name}` | Remove MCP server | `Effect::DeleteMcpServer` |
| `PATCH` | `/api/sessions/{id}/mcps/{name}` | Enable/disable server | `Effect::ToggleMcpServer` |
| `PATCH` | `/api/sessions/{id}/mcps/{name}/tools/{tool}` | Enable/disable tool | `Effect::ToggleMcpTool` |

### 5.7 Event Stream (SSE)

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/sessions/{id}/stream` | Subscribe to all session events (SSE) |
| `GET` | `/api/sessions/{id}/stream?filter=turn,permission` | Filtered event stream |

The SSE stream is the **primary notification channel** for REST clients. It replaces the TUI's `tokio::select!` loop over `AcpClientChannel`.

### 5.8 Interactive Responses (Agent → Client)

| Method | Path | Description | Maps To |
|---|---|---|---|
| `POST` | `/api/sessions/{id}/questions/{qid}/answer` | Answer `ask_user_question` | `AcpAgentMessage::ExtMethod` reply |
| `POST` | `/api/sessions/{id}/plan-mode/exit` | Confirm plan mode exit | `AcpAgentMessage::ExtMethod` reply |

### 5.9 Health & Meta

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Liveness check |
| `GET` | `/ready` | Readiness (auth + agent connected) |
| `GET` | `/api/models` | Available models |
| `GET` | `/api/version` | Server version |

---

## 6. ACP Bridge Layer (`bridge/acp_bridge.rs`)

This is the **core translation layer** between REST and the existing ACP protocol.

### 6.1 Session Lifecycle

```rust
pub async fn create_session(state: &AppState, req: CreateSessionRequest) -> Result<SessionHandle> {
    // 1. Spawn ainxt-shell process (reuse spawn_ainxt_shell() from headless.rs)
    // 2. Send ACP Initialize → Authenticate → NewSession
    // 3. Start background task: pump AcpClientMessage → broadcast::Sender<RestEvent>
    // 4. Register SessionHandle in AppState.sessions
    // 5. Return SessionHandle
}
```

### 6.2 Event Fan-Out (`bridge/event_fan.rs`)

```rust
pub async fn pump_acp_events(
    mut acp_rx: AcpClientRx,
    event_tx: broadcast::Sender<RestEvent>,
    session_state: Arc<RwLock<SessionState>>,
) {
    loop {
        match acp_rx.recv().await {
            AcpClientMessage::SessionNotification(notif) => {
                // Map SessionNotification → RestEvent variants
                // Update session_state (turn_active, etc.)
                let _ = event_tx.send(map_session_notification(notif));
            }
            AcpClientMessage::RequestPermission(req) => {
                // Store in session_state.pending_permissions
                // Emit RestEvent::PermissionRequest
                let _ = event_tx.send(RestEvent::PermissionRequest { ... });
            }
            AcpClientMessage::ExtMethod(ext) => {
                match ext.method.as_str() {
                    "ainxt.dev/ask_user_question" => { /* emit AskUserQuestion */ }
                    "ainxt.dev/exit_plan_mode"    => { /* emit PlanModeExited */ }
                    _ => {}
                }
            }
            AcpClientMessage::ExtNotification(notif) => {
                match notif.method.as_str() {
                    "ainxt.dev/queue/changed"  => { /* update state + emit QueueChanged */ }
                    "ainxt.dev/follow_ups"     => { /* emit FollowUps */ }
                    "ainxt.dev/monitor_event"  => { /* emit MonitorEvent */ }
                    _ => {}
                }
            }
        }
    }
}
```

### 6.3 Prompt Handler (Streaming)

```rust
pub async fn send_prompt_streaming(
    handle: &SessionHandle,
    text: String,
) -> Result<impl Stream<Item = RestEvent>> {
    // 1. Generate prompt_id
    // 2. Send AcpAgentMessage::Prompt to agent_tx
    // 3. Subscribe to handle.event_tx (broadcast::Receiver)
    // 4. Stream RestEvent as SSE until TurnComplete | TurnError | TurnCancelled
    // 5. Unsubscribe
}
```

---

## 7. Implementation Phases

### Phase 1: Foundation (New Crate + Basic Session CRUD)
**Effort: ~3–4 days**

- [ ] Create `crates/codegen/ainxt-rest-server/` crate
- [ ] Add to workspace `Cargo.toml`
- [ ] Implement `AppState`, `SessionHandle`, `SessionState`
- [ ] Implement `POST /api/sessions` — create session (reuse `headless.rs` spawn logic)
- [ ] Implement `DELETE /api/sessions/{id}` — close session
- [ ] Implement `GET /health` and `GET /ready`
- [ ] Add Bearer token auth middleware (simple API key check)
- [ ] Add to `ainxt-pager-bin/src/main.rs` as new subcommand: `ainxt rest-server --port 8080`

### Phase 2: Prompt + SSE Streaming
**Effort: ~3–4 days**

- [ ] Implement `bridge/event_fan.rs` — ACP → `broadcast::Sender<RestEvent>` pump
- [ ] Implement `GET /api/sessions/{id}/stream` — SSE endpoint using `axum::response::Sse`
- [ ] Implement `POST /api/sessions/{id}/prompt` — send prompt, return SSE stream
- [ ] Map `SessionNotification` → `RestEvent` variants (text delta, tool calls, turn lifecycle)
- [ ] Implement `DELETE /api/sessions/{id}/turn` — cancel turn
- [ ] Add heartbeat ping every 15s to SSE stream

### Phase 3: Permission Handling
**Effort: ~2 days**

- [ ] Implement `GET /api/sessions/{id}/permissions` — list pending
- [ ] Implement `POST /api/sessions/{id}/permissions/{req_id}/approve`
- [ ] Implement `DELETE /api/sessions/{id}/permissions/{req_id}` — cancel
- [ ] Implement `POST /api/sessions/{id}/questions/{qid}/answer` — answer `ask_user_question`
- [ ] Store pending permissions in `SessionState` with `tokio::sync::oneshot` for reply

### Phase 4: Queue + Mode Control
**Effort: ~2 days**

- [ ] Implement all `/api/sessions/{id}/queue/*` endpoints
- [ ] Implement `/api/sessions/{id}/mode`, `/model`, `/plan-mode/toggle`
- [ ] Map queue operations to ACP ext methods (`ainxt.dev/queue/*`)
- [ ] Sync queue state from `ainxt.dev/queue/changed` notifications

### Phase 5: MCP + Advanced Features
**Effort: ~2 days**

- [ ] Implement `/api/sessions/{id}/mcps/*` endpoints
- [ ] Implement `POST /api/sessions/{id}/interject`
- [ ] Implement `POST /api/sessions/{id}/bash`
- [ ] Implement `GET /api/sessions` — list sessions
- [ ] Implement `POST /api/sessions/{id}/fork`
- [ ] Implement `GET /api/models`

### Phase 6: Non-Streaming (Sync) Mode + Polish
**Effort: ~2 days**

- [ ] Implement `stream: false` mode for `POST /api/sessions/{id}/prompt` — buffer full turn, return JSON
- [ ] Add request/response logging middleware
- [ ] Add CORS middleware (for browser clients)
- [ ] Add rate limiting (tower-governor or custom)
- [ ] Add OpenAPI spec generation (utoipa or aide)
- [ ] Write integration tests

---

## 8. Key Technical Decisions

### 8.1 Process Model: In-Process vs. Subprocess

| Option | Description | Pros | Cons |
|---|---|---|---|
| **A: In-Process** *(Recommended for production)* | REST server and `ainxt-shell` agent run in the same process via in-memory ACP channels | No IPC overhead, reuse `MvpAgent` directly | More complex wiring |
| **B: Subprocess** *(Recommended for Phase 1)* | REST server spawns `ainxt agent stdio` as a subprocess | Reuse `headless.rs` exactly, fault-isolated | Slightly higher latency |

**Recommendation: Start with Option B (subprocess) for Phase 1–3, migrate to Option A for production.**

### 8.2 Session Multiplexing

The existing leader process (`ainxt agent serve`) already multiplexes multiple sessions over a single agent process. The REST server can:

| Option | Description |
|---|---|
| **A: Connect to existing leader** *(Recommended)* | REST server is a new client of the existing leader via Unix socket |
| **B: Dedicated agent per session** | Spawn one agent process per REST session |
| **C: Spawn one leader** | Spawn one leader, connect all REST sessions to it |

**Recommendation: Option A — connect REST server to the existing leader process.**

### 8.3 Authentication

Two auth layers:
1. **REST client auth**: Bearer token / API key in `Authorization` header (new, simple)
2. **LLM backend auth**: Reuse existing `AuthManager` from `ainxt-auth` (OIDC / API key)

### 8.4 Streaming Protocol

Use **Server-Sent Events (SSE)** as the primary streaming protocol:
- Native browser support, no special client library needed
- Works through HTTP/1.1 and HTTP/2
- Axum has built-in `axum::response::Sse` support
- Simpler than WebSocket for unidirectional server→client streaming

For bidirectional use cases (e.g., interactive permission approval during a stream), clients can:
- Keep the SSE stream open for events
- Make separate POST requests to approve/deny permissions

### 8.5 Slash Command Handling

| Option | Description |
|---|---|
| **1: Client-side** *(Recommended initially)* | Document slash commands; REST clients send them as plain text; agent handles `/` prefix |
| **2: Server-side** | Implement a `SlashCommandRouter` in the REST server that intercepts `/` prefixed prompts |

---

## 9. New Crate Dependencies

```toml
# crates/codegen/ainxt-rest-server/Cargo.toml
[dependencies]
ainxt-acp-lib         = { path = "../ainxt-acp-lib" }
ainxt-auth            = { path = "../ainxt-auth" }
ainxt-config          = { path = "../ainxt-config" }
ainxt-http            = { path = "../ainxt-http" }
ainxt-prompt-queue    = { path = "../ainxt-prompt-queue" }
ainxt-sampling-types  = { path = "../ainxt-sampling-types" }
ainxt-workspace-types = { path = "../ainxt-workspace-types" }
cli-chat-proxy-types  = { path = "../../../prod/mc/cli-chat-proxy-types" }

axum         = { version = "0.8", features = ["macros", "ws"] }
tokio        = { version = "1", features = ["full"] }
tower        = "0.5"
tower-http   = { version = "0.6", features = ["cors", "trace", "compression-gzip"] }
serde        = { version = "1", features = ["derive"] }
serde_json   = "1"
tokio-stream = { version = "0.1", features = ["sync"] }
dashmap      = "6"
uuid         = { version = "1", features = ["v4"] }
tracing      = "0.1"
anyhow       = "1"
thiserror    = "2"
```

---

## 10. Integration with Existing CLI

Add a new subcommand to `ainxt-pager-bin/src/main.rs`:

```rust
// In Command enum:
RestServer(RestServerArgs),

// Args:
#[derive(Args)]
pub struct RestServerArgs {
    #[arg(long, default_value = "8080")]
    pub port: u16,
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, env = "AINXT_REST_API_KEY")]
    pub api_key: Option<String>,
    #[arg(long)]
    pub leader_socket: Option<PathBuf>,  // connect to existing leader
    #[arg(long, default_value = "false")]
    pub cors_allow_all: bool,
}

// Handler:
async fn run_rest_server(args: RestServerArgs, config: EffectiveConfig) -> Result<()> {
    ainxt_rest_server::serve(args, config).await
}
```

**Usage:**
```bash
ainxt rest-server --port 8080 --api-key my-secret-key
```

---

## 11. Example Client Usage

```bash
# Create a session
SESSION=$(curl -s -X POST http://localhost:8080/api/sessions \
  -H "Authorization: Bearer <YOUR_API_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"cwd": "/my/project", "model_id": "claude-opus-4-7"}' | jq -r .session_id)

# Send a prompt and stream the response
curl -N http://localhost:8080/api/sessions/$SESSION/prompt \
  -H "Authorization: Bearer <YOUR_API_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"text": "Explain the auth module", "stream": true}'

# Approve a pending permission
curl -X POST http://localhost:8080/api/sessions/$SESSION/permissions/req_001/approve \
  -H "Authorization: Bearer <YOUR_API_KEY>" \
  -H "Content-Type: application/json" \
  -d '{"option_id": "allow_once"}'

# Cancel the current turn
curl -X DELETE http://localhost:8080/api/sessions/$SESSION/turn \
  -H "Authorization: Bearer <YOUR_API_KEY>"

# Delete the session
curl -X DELETE http://localhost:8080/api/sessions/$SESSION \
  -H "Authorization: Bearer <YOUR_API_KEY>"
```

---

## 12. Summary: What Can Be Exposed as REST

| Capability | REST Feasibility | Notes |
|---|---|---|
| Create / list / delete sessions | ✅ Full | Direct ACP mapping |
| Send prompts (streaming SSE) | ✅ Full | SSE replaces TUI rendering |
| Send prompts (sync / blocking) | ✅ Full | Buffer turn, return JSON |
| Cancel turns | ✅ Full | ACP `CancelNotification` |
| Permission approval / denial | ✅ Full | Critical — blocks turn |
| Queue management | ✅ Full | ACP ext methods |
| Mode switching (plan / yolo / auto) | ✅ Full | ACP ext methods |
| Model switching | ✅ Full | ACP `SetSessionModel` |
| MCP server management | ✅ Full | ACP ext methods |
| Fork sessions | ✅ Full | ACP `NewSession` with parent |
| Bash command execution | ✅ Full | ACP `PromptRequest` with bash kind |
| Mid-turn interjection | ✅ Full | ACP ext method |
| Multi-agent dashboard | ⚠️ Partial | Needs per-subagent session tracking |
| Slash command routing | ⚠️ Partial | Most pass through to agent; some need REST-side handling |
| Voice mode | ❌ N/A | TUI-only feature |
| Mermaid / image rendering | ❌ N/A | TUI-only feature |
| Inline terminal (PTY) | ❌ N/A | TUI-only; could add WebSocket PTY later |
| Hot-reload config | ⚠️ Partial | REST server can watch config file |
| Leader reconnect / replay | ✅ Full | Reuse existing `StdioReplayState` logic |
