//! Wire-level proof that a model's capability profile shapes what actually
//! leaves the process.
//!
//! The per-layer unit tests cover the profile type, the request builder, and the
//! client injection separately. These drive a real `SamplingClient` against a
//! mock Messages endpoint and assert on the received JSON, so a future
//! refactor that drops the profile somewhere between `SamplerConfig` and the
//! socket fails here.

use ainxt_sampler::{ApiBackend, SamplerConfig, SamplingClient};
use ainxt_sampling_types::{
    ConversationItem, ConversationRequest, ModelCapabilities, ReasoningEffort,
};
use ainxt_test_support::mock_server::MockInferenceServer;
use serde_json::Value;

fn config(base_url: String, model: &str, capabilities: ModelCapabilities) -> SamplerConfig {
    SamplerConfig {
        api_key: Some("test-key".to_string()),
        base_url,
        model: model.to_string(),
        api_backend: ApiBackend::Messages,
        capabilities,
        context_window: 262_144,
        max_completion_tokens: Some(4096),
        ..SamplerConfig::default()
    }
}

/// A second-turn conversation: a reasoning sibling plus a prior assistant turn,
/// so the history-only extensions (replayed `thinking` blocks) are in play.
fn second_turn_request(signed: bool) -> ConversationRequest {
    let mut reasoning = ainxt_sampling_types::synthesized_reasoning_item("deliberating");
    if signed {
        reasoning.encrypted_content = Some("sig-from-server".to_string());
    }
    let mut request = ConversationRequest::from_items(vec![
        ConversationItem::system("you are a coding agent"),
        ConversationItem::user("first turn"),
        ConversationItem::Reasoning(reasoning),
        ConversationItem::assistant("an answer"),
        ConversationItem::user("second turn"),
    ]);
    request.reasoning_effort = Some(ReasoningEffort::High);
    request
}

/// Every content block type present anywhere in the request body.
fn block_types(body: &Value) -> Vec<String> {
    body.get("messages")
        .and_then(|m| m.as_array())
        .map(|msgs| {
            msgs.iter()
                .filter_map(|m| m.get("content").and_then(|c| c.as_array()))
                .flatten()
                .filter_map(|b| b.get("type").and_then(|t| t.as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

async fn send(server: &MockInferenceServer, cfg: SamplerConfig, signed: bool) -> Value {
    let client = SamplingClient::new(cfg).expect("client builds");
    // The response shape is irrelevant here; only the request body is asserted.
    let _ = client
        .conversation_stream_messages(second_turn_request(signed))
        .await;
    server
        .request_bodies()
        .into_iter()
        .next()
        .expect("the mock recorded a request body")
}

/// The core-only profile must produce a request with no Anthropic extensions:
/// no `thinking`, no `output_config`, no `cache_control`, and a string `system`.
#[tokio::test]
async fn core_only_profile_sends_no_extensions_on_the_wire() {
    let server = MockInferenceServer::start().await.expect("mock starts");
    let cfg = config(
        server.url(),
        "local:kimi-k2.7-code",
        ModelCapabilities::core_only(),
    );
    let body = send(&server, cfg, true).await;

    assert!(
        body.get("thinking").is_none(),
        "thinking must be absent: {body}"
    );
    assert!(
        body.get("output_config").is_none(),
        "output_config must be absent: {body}"
    );
    assert!(
        !body.to_string().contains("cache_control"),
        "no cache breakpoints: {body}"
    );
    assert!(
        body.get("system").map(|s| s.is_string()).unwrap_or(false),
        "system must be a plain string, got {:?}",
        body.get("system")
    );
    let types = block_types(&body);
    assert!(
        !types.iter().any(|t| t == "thinking"),
        "no thinking blocks replayed: {types:?}"
    );

    // The conversation itself must be fully intact.
    let serialized = body.to_string();
    for expected in ["first turn", "an answer", "second turn", "coding agent"] {
        assert!(serialized.contains(expected), "{expected} missing: {body}");
    }
}

/// The full profile is the untouched first-party path: extensions present, and
/// a signed thinking block replayed.
#[tokio::test]
async fn full_profile_sends_every_extension_on_the_wire() {
    let server = MockInferenceServer::start().await.expect("mock starts");
    let cfg = config(server.url(), "claude-sonnet-4-6", ModelCapabilities::full());
    let body = send(&server, cfg, true).await;

    assert!(
        body.get("thinking").is_some(),
        "thinking must be present: {body}"
    );
    assert!(
        body.get("output_config").is_some(),
        "output_config must be present: {body}"
    );
    assert!(
        body.to_string().contains("cache_control"),
        "cache breakpoint expected: {body}"
    );
    let types = block_types(&body);
    assert!(
        types.iter().any(|t| t == "thinking"),
        "signed thinking block must be replayed: {types:?}"
    );
}

/// A request-level profile is never trusted: callers build `ConversationRequest`s
/// without knowing which model will serve them, so the client's config wins.
#[tokio::test]
async fn client_config_overrides_any_request_level_profile() {
    let server = MockInferenceServer::start().await.expect("mock starts");
    let cfg = config(
        server.url(),
        "local:kimi-k2.7-code",
        ModelCapabilities::core_only(),
    );
    let client = SamplingClient::new(cfg).expect("client builds");

    let mut request = second_turn_request(true);
    // A stale/permissive value on the request must not survive dispatch.
    request.capabilities = ModelCapabilities::full();
    let _ = client.conversation_stream_messages(request).await;

    let body = server
        .request_bodies()
        .into_iter()
        .next()
        .expect("recorded body");
    assert!(
        body.get("thinking").is_none(),
        "the client's core-only profile must win: {body}"
    );
}

/// An unsigned thinking block is unsendable even on a fully capable model —
/// there is no valid wire form for it.
#[tokio::test]
async fn unsigned_thinking_is_dropped_even_on_the_full_profile() {
    let server = MockInferenceServer::start().await.expect("mock starts");
    let cfg = config(server.url(), "claude-sonnet-4-6", ModelCapabilities::full());
    let body = send(&server, cfg, false).await;

    let types = block_types(&body);
    assert!(
        !types.iter().any(|t| t == "thinking"),
        "unsigned thinking must not be replayed: {types:?}"
    );
    assert!(
        body.to_string().contains("an answer"),
        "the assistant text still ships: {body}"
    );
}

/// Everything below covers `--log-sampling` request/response body recording:
/// the diagnostic that turns "the model failed" into the exact JSON that was
/// sent and the exact rejection that came back.
mod sampling_log_bodies {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
    use tracing_subscriber::registry::LookupSpan;

    /// Captures `field = value` pairs of every `target: "sampling_log"` event.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<(String, String)>>>);

    impl<S> Layer<S> for Captured
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            if event.metadata().target() != "sampling_log" {
                return;
            }
            let mut visitor = Visitor(Vec::new());
            event.record(&mut visitor);
            self.0.lock().unwrap().extend(visitor.0);
        }
    }

    struct Visitor(Vec<(String, String)>);

    impl tracing::field::Visit for Visitor {
        fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
            self.0.push((f.name().to_string(), format!("{v:?}")));
        }
        fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
            self.0.push((f.name().to_string(), v.to_string()));
        }
    }

    impl Captured {
        fn get(&self, key: &str) -> Option<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        }
        fn all(&self, key: &str) -> Vec<String> {
            self.0
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .collect()
        }
    }

    /// The recorded body is the real request: it carries the conversation, and
    /// it reflects the capability profile that shaped it.
    #[tokio::test]
    async fn request_body_is_recorded_with_the_profile_applied() {
        let server = MockInferenceServer::start().await.expect("mock starts");
        let captured = Captured::default();
        let subscriber = tracing_subscriber::registry().with(captured.clone());

        let cfg = config(
            server.url(),
            "local:kimi-k2.7-code",
            ModelCapabilities::core_only(),
        );
        let client = SamplingClient::new(cfg).expect("client builds");

        // `set_default` installs the subscriber for this thread and keeps it
        // until the guard drops. `#[tokio::test]` runs a current-thread
        // runtime, so the future stays on this thread and its events are
        // captured. (Blocking on the future inside a sync `with_default`
        // closure would instead stall the reactor the request needs.)
        let _guard = tracing::subscriber::set_default(subscriber);
        let _ = client
            .conversation_stream_messages(second_turn_request(true))
            .await;

        let body = captured
            .get("body")
            .expect("a request_body event must be recorded");
        assert!(
            captured.all("event").iter().any(|e| e == "request_body"),
            "the event must be tagged request_body"
        );
        assert_eq!(captured.get("endpoint").as_deref(), Some("messages"));
        assert_eq!(
            captured.get("model").as_deref(),
            Some("local:kimi-k2.7-code")
        );
        // It is the real conversation…
        assert!(body.contains("second turn"), "body: {body}");
        // …shaped by the profile.
        assert!(!body.contains("thinking"), "body: {body}");
        assert!(!body.contains("cache_control"), "body: {body}");
    }

}
