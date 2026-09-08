//! Unit tests for the [`super`] Messages L2 stream transform. Extracted
//! from `messages.rs` so the implementation reads top-to-bottom; wired in
//! via `#[path = "messages_tests.rs"] mod tests;` in messages.rs.

use super::*;
use futures_util::stream;
use std::pin::pin;
use ainxt_sampling_types::messages::{
    ContentBlock, MessageDeltaBody, MessageDeltaUsage, MessagesResponse, MessagesUsage,
    StreamDelta, StreamError,
};

fn rid() -> RequestId {
    RequestId::from("msg-test")
}

fn message_start() -> MessageStreamEvent {
    MessageStreamEvent::MessageStart {
        message: MessagesResponse {
            id: "msg_1".into(),
            r#type: "message".into(),
            role: "assistant".into(),
            content: vec![],
            model: "messages-compatible-model".into(),
            stop_reason: None,
            usage: MessagesUsage {
                input_tokens: 10,
                output_tokens: 0,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
        },
    }
}

fn text_block_start(index: u32) -> MessageStreamEvent {
    MessageStreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlock::Text {
            text: String::new(),
            cache_control: None,
        },
    }
}

fn text_delta(index: u32, text: &str) -> MessageStreamEvent {
    MessageStreamEvent::ContentBlockDelta {
        index,
        delta: StreamDelta::TextDelta { text: text.into() },
    }
}

fn block_stop(index: u32) -> MessageStreamEvent {
    MessageStreamEvent::ContentBlockStop { index }
}

fn message_delta_with_stop(stop: messages::StopReason) -> MessageStreamEvent {
    MessageStreamEvent::MessageDelta {
        delta: MessageDeltaBody {
            stop_reason: Some(stop),
            stop_details: None,
        },
        usage: MessageDeltaUsage {
            output_tokens: 5,
            input_tokens: Some(10),
            ..Default::default()
        },
    }
}

/// A refusal `message_delta` carrying a provider `stop_details.explanation`,
/// mirroring the Anthropic Messages API ToS auto-refusal wire shape.
fn message_delta_refusal_with_explanation(explanation: &str) -> MessageStreamEvent {
    MessageStreamEvent::MessageDelta {
        delta: MessageDeltaBody {
            stop_reason: Some(messages::StopReason::Refusal),
            stop_details: Some(messages::StopDetails {
                r#type: Some("refusal".to_string()),
                category: Some("frontier_llm".to_string()),
                explanation: Some(explanation.to_string()),
            }),
        },
        usage: MessageDeltaUsage {
            output_tokens: 0,
            input_tokens: Some(10),
            ..Default::default()
        },
    }
}

async fn collect(s: impl Stream<Item = SamplingEvent>) -> Vec<SamplingEvent> {
    let mut out = Vec::new();
    let mut s = pin!(s);
    while let Some(ev) = s.next().await {
        out.push(ev);
    }
    out
}

#[tokio::test]
async fn empty_stream_yields_started_then_completed() {
    let raw = stream::iter(Vec::<Result<MessageStreamEvent, SamplingError>>::new()).boxed();
    let events = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
    assert!(matches!(events[1], SamplingEvent::Completed { .. }));
}

#[tokio::test]
async fn text_block_assembles_into_completed_response() {
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(text_block_start(0)),
        Ok(text_delta(0, "Hello, ")),
        Ok(text_delta(0, "world!")),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(messages::StopReason::EndTurn)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    let text_tokens: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            SamplingEvent::ChannelToken {
                channel: SamplingChannel::Text,
                text,
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text_tokens, vec!["Hello, ", "world!"]);

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let a = response.assistant().expect("assistant item present");
            assert_eq!(a.content.as_ref(), "Hello, world!");
            assert_eq!(a.model_id.as_deref(), Some("messages-compatible-model"));
            assert_eq!(response.stop_reason, Some(StopReason::Stop));
            let u = response.usage.as_ref().expect("usage extracted");
            assert_eq!(u.prompt_tokens, 10);
            assert_eq!(u.completion_tokens, 5);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn thinking_block_emits_reasoning_channel_and_preserved_in_response() {
    let thinking_start = MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::Thinking {
            thinking: String::new(),
            signature: String::new(),
        },
    };
    let thinking_delta = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: StreamDelta::ThinkingDelta {
            thinking: "let me think...".into(),
        },
    };
    let sig_delta = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: StreamDelta::SignatureDelta {
            signature: "abc123".into(),
        },
    };
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(thinking_start),
        Ok(thinking_delta),
        Ok(sig_delta),
        Ok(block_stop(0)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    let reasoning_tokens: Vec<&str> = evs
        .iter()
        .filter_map(|e| match e {
            SamplingEvent::ChannelToken {
                channel: SamplingChannel::Reasoning,
                text,
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning_tokens, vec!["let me think..."]);

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let r = response
                .reasoning_items()
                .next()
                .expect("reasoning sibling preserved");
            let rs::SummaryPart::SummaryText(t) = &r.summary[0];
            assert_eq!(t.text, "let me think...");
            assert_eq!(r.encrypted_content.as_deref(), Some("abc123"));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// A Messages-compatible gateway that receives a *complete* tool call from
/// upstream (rather than incremental fragments) has nothing to stream, so it
/// puts the whole argument object inline on `content_block_start` and sends no
/// `input_json_delta` events. That shape is legal on the wire — `input` is a
/// required field of a `tool_use` block — and the arguments must survive.
///
/// Before the fix the `input` was discarded and the call finalized with
/// `arguments: ""`, so every tool ran with unparseable arguments.
#[tokio::test]
async fn tool_use_input_on_block_start_without_deltas_is_kept() {
    let tool_start = MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::ToolUse {
            id: "call_inline".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "a.rs"}),
        },
    };
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(tool_start),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(messages::StopReason::ToolUse)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let calls = response.tool_calls();
            assert_eq!(calls.len(), 1, "got: {calls:?}");
            assert_eq!(calls[0].id.as_ref(), "call_inline");
            assert_eq!(calls[0].name, "read_file");
            let parsed: serde_json::Value = serde_json::from_str(calls[0].arguments.as_ref())
                .unwrap_or_else(|e| {
                    panic!(
                        "arguments must be valid JSON, got {:?}: {e}",
                        calls[0].arguments.as_ref()
                    )
                });
            assert_eq!(parsed, serde_json::json!({"path": "a.rs"}));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// When a server sends BOTH a complete `input` on the start event and streamed
/// fragments, the fragments must win — concatenating the two would produce
/// invalid JSON like `{"path":"a.rs"}{"path":"b.rs"}`.
#[tokio::test]
async fn streamed_fragments_override_a_block_start_seed() {
    let tool_start = MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::ToolUse {
            id: "call_both".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "stale.rs"}),
        },
    };
    let frag = |partial: &str| MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: StreamDelta::InputJsonDelta {
            partial_json: partial.into(),
        },
    };
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(tool_start),
        Ok(frag(r#"{"path":"#)),
        Ok(frag(r#""fresh.rs"}"#)),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(messages::StopReason::ToolUse)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let calls = response.tool_calls();
            assert_eq!(calls.len(), 1, "got: {calls:?}");
            assert_eq!(calls[0].arguments.as_ref(), r#"{"path":"fresh.rs"}"#);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// A tool call that genuinely takes no arguments must finalize as `{}`, not as
/// an empty string that fails every JSON parser downstream.
#[tokio::test]
async fn tool_use_with_no_arguments_finalizes_as_empty_object() {
    let tool_start = MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::ToolUse {
            id: "call_noargs".into(),
            name: "list_todos".into(),
            input: serde_json::json!({}),
        },
    };
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(tool_start),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(messages::StopReason::ToolUse)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let calls = response.tool_calls();
            assert_eq!(calls.len(), 1, "got: {calls:?}");
            assert_eq!(calls[0].arguments.as_ref(), "{}");
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// Several parallel tool calls delivered whole on their start events — the
/// shape a subagent fan-out produces — must each keep their own arguments.
#[tokio::test]
async fn multiple_inline_tool_uses_keep_separate_arguments() {
    let inline = |index: u32, id: &str, path: &str| MessageStreamEvent::ContentBlockStart {
        index,
        content_block: ContentBlock::ToolUse {
            id: id.into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": path}),
        },
    };
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(inline(0, "call_1", "a.rs")),
        Ok(block_stop(0)),
        Ok(inline(1, "call_2", "b.rs")),
        Ok(block_stop(1)),
        Ok(message_delta_with_stop(messages::StopReason::ToolUse)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let calls = response.tool_calls();
            assert_eq!(calls.len(), 2, "got: {calls:?}");
            assert_eq!(calls[0].id.as_ref(), "call_1");
            assert_eq!(calls[0].arguments.as_ref(), r#"{"path":"a.rs"}"#);
            assert_eq!(calls[1].id.as_ref(), "call_2");
            assert_eq!(calls[1].arguments.as_ref(), r#"{"path":"b.rs"}"#);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn tool_use_block_assembles_into_tool_call() {
    let tool_start = MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::ToolUse {
            id: "call_xyz".into(),
            name: "do_thing".into(),
            input: serde_json::json!({}),
        },
    };
    let arg_delta_1 = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: StreamDelta::InputJsonDelta {
            partial_json: "{\"x\":".into(),
        },
    };
    let arg_delta_2 = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: StreamDelta::InputJsonDelta {
            partial_json: "1}".into(),
        },
    };
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(tool_start),
        Ok(arg_delta_1),
        Ok(arg_delta_2),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(messages::StopReason::ToolUse)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    // Should yield three ToolCallDelta events: id+name, then two
    // arguments fragments.
    let deltas: Vec<_> = evs
        .iter()
        .filter_map(|e| match e {
            SamplingEvent::ToolCallDelta {
                tool_index,
                id,
                name,
                arguments_delta,
                ..
            } => Some((
                *tool_index,
                id.clone(),
                name.clone(),
                arguments_delta.clone(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(deltas.len(), 3);
    assert_eq!(deltas[0].0, 0);
    assert_eq!(deltas[0].1.as_deref(), Some("call_xyz"));
    assert_eq!(deltas[0].2.as_deref(), Some("do_thing"));
    assert_eq!(deltas[0].3, None);
    assert_eq!(deltas[1].3.as_deref(), Some("{\"x\":"));
    assert_eq!(deltas[2].3.as_deref(), Some("1}"));

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let calls = response.tool_calls();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id.as_ref(), "call_xyz");
            assert_eq!(calls[0].name, "do_thing");
            assert_eq!(calls[0].arguments.as_ref(), "{\"x\":1}");
            assert_eq!(response.stop_reason, Some(StopReason::ToolCalls));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// Regression: a stream whose terminal `message_delta` carries
/// `stop_reason: "refusal"` must complete cleanly — not error out and
/// discard the already-streamed response.
#[tokio::test]
async fn refusal_stop_reason_completes_stream() {
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(text_block_start(0)),
        Ok(text_delta(0, "I can't help with that.")),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(messages::StopReason::Refusal)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    assert!(
        !evs.iter()
            .any(|e| matches!(e, SamplingEvent::Failed { .. })),
        "refusal stream must not yield Failed: {evs:?}"
    );
    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            let a = response.assistant().expect("assistant item present");
            assert_eq!(a.content.as_ref(), "I can't help with that.");
            assert_eq!(response.stop_reason, Some(StopReason::ContentFilter));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// A refusal `stop_details.explanation` on the terminal delta must be
/// normalized onto the completed `ConversationResponse.stop_message` so the
/// agent loop can surface the provider's reason (empty-turn silence otherwise).
#[tokio::test]
async fn refusal_stop_message_flows_to_response() {
    let explanation = "This request was blocked by the provider's content policy.";
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(message_delta_refusal_with_explanation(explanation)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            assert_eq!(response.stop_reason, Some(StopReason::ContentFilter));
            assert_eq!(
                response.stop_message.as_deref(),
                Some(explanation),
                "provider explanation normalized onto stop_message"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn pause_turn_and_unknown_stop_reasons_complete_as_stop() {
    for stop in [
        messages::StopReason::PauseTurn,
        messages::StopReason::Unknown("mystery_reason".to_string()),
    ] {
        let label = format!("{stop:?}");
        let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
            Ok(message_start()),
            Ok(text_block_start(0)),
            Ok(text_delta(0, "partial answer")),
            Ok(block_stop(0)),
            Ok(message_delta_with_stop(stop)),
            Ok(MessageStreamEvent::MessageStop),
        ];
        let raw = stream::iter(events).boxed();
        let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;
        match evs.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                assert_eq!(
                    response.stop_reason,
                    Some(StopReason::Stop),
                    "{label} must end the turn like stop"
                );
            }
            other => panic!("{label}: expected Completed, got {other:?}"),
        }
    }
}

/// Pins the model_context_window_exceeded decision: it stays in the
/// max_tokens truncation class (fatal, non-retryable), not the
/// context-length Api class.
#[tokio::test]
async fn model_context_window_exceeded_fails_as_max_tokens_truncation() {
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(text_block_start(0)),
        Ok(text_delta(0, "truncated answ")),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(
            messages::StopReason::ModelContextWindowExceeded,
        )),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    assert!(
        !evs.iter()
            .any(|e| matches!(e, SamplingEvent::Completed { .. })),
        "context-window truncation must not complete: {evs:?}"
    );
    match evs.last().unwrap() {
        SamplingEvent::Failed { error, .. } => {
            assert_eq!(
                error.kind,
                crate::events::SamplingErrorKind::MaxTokensTruncation
            );
            assert!(!error.is_retryable, "truncation is deterministic");
        }
        other => panic!("expected Failed(MaxTokensTruncation), got {other:?}"),
    }
}

/// Pins the pre-existing override: completed tool_use blocks beat a terminal
/// Refusal, so the agent loop still resolves the calls.
#[tokio::test]
async fn refusal_after_tool_use_blocks_keeps_tool_calls_stop_reason() {
    let tool_start = MessageStreamEvent::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::ToolUse {
            id: "call_refused".into(),
            name: "do_thing".into(),
            input: serde_json::json!({}),
        },
    };
    let arg_delta = MessageStreamEvent::ContentBlockDelta {
        index: 0,
        delta: StreamDelta::InputJsonDelta {
            partial_json: "{}".into(),
        },
    };
    let events: Vec<Result<MessageStreamEvent, SamplingError>> = vec![
        Ok(message_start()),
        Ok(tool_start),
        Ok(arg_delta),
        Ok(block_stop(0)),
        Ok(message_delta_with_stop(messages::StopReason::Refusal)),
        Ok(MessageStreamEvent::MessageStop),
    ];
    let raw = stream::iter(events).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    match evs.last().unwrap() {
        SamplingEvent::Completed { response, .. } => {
            assert_eq!(response.tool_calls().len(), 1);
            assert_eq!(
                response.stop_reason,
                Some(StopReason::ToolCalls),
                "tool_use blocks must win over the refusal stop_reason"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn server_error_event_yields_failed_500() {
    let err_event = MessageStreamEvent::Error {
        error: StreamError {
            r#type: "overloaded_error".into(),
            message: "rate limit hit".into(),
        },
    };
    let raw = stream::iter(vec![Ok(message_start()), Ok(err_event)]).boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;

    match evs.last().unwrap() {
        SamplingEvent::Failed { error, .. } => {
            assert_eq!(error.kind, crate::events::SamplingErrorKind::Api);
            assert_eq!(error.status_code, Some(500));
            assert!(error.message.contains("overloaded_error"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn mid_stream_transport_error_yields_failed() {
    let raw = stream::iter(vec![
        Ok(message_start()),
        Err(SamplingError::EventStreamError("conn reset".into())),
    ])
    .boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;
    assert!(
        evs.iter()
            .any(|e| matches!(e, SamplingEvent::Failed { .. }))
    );
    assert!(
        !evs.iter()
            .any(|e| matches!(e, SamplingEvent::Completed { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn idle_timeout_when_stream_stalls() {
    let raw = stream::iter(vec![Ok(message_start())])
        .chain(stream::pending())
        .boxed();
    let evs = collect(stream_messages(
        raw,
        None,
        rid(),
        Duration::from_millis(100),
    ))
    .await;

    match evs.last().unwrap() {
        SamplingEvent::Failed { error, .. } => {
            assert_eq!(error.kind, crate::events::SamplingErrorKind::IdleTimeout);
        }
        other => panic!("expected Failed(IdleTimeout), got {other:?}"),
    }
}

#[tokio::test]
async fn model_metadata_yielded_after_stream_started() {
    let raw = stream::iter(vec![Ok(MessageStreamEvent::MessageStop)]).boxed();
    let metadata = ResponseModelMetadata {
        context_window: Some(200_000),
        ..Default::default()
    };
    let evs = collect(stream_messages(
        raw,
        Some(metadata),
        rid(),
        Duration::from_secs(60),
    ))
    .await;

    assert!(matches!(evs[0], SamplingEvent::StreamStarted { .. }));
    assert!(matches!(evs[1], SamplingEvent::ModelMetadata { .. }));
}

#[test]
fn meaningful_content_classifier_treats_ping_as_keepalive() {
    assert!(!messages_event_has_meaningful_content(
        &MessageStreamEvent::Ping
    ));
    assert!(messages_event_has_meaningful_content(
        &MessageStreamEvent::MessageStop
    ));
}

// ── Token usage: Anthropic Messages API cache-bucket accounting ────────────

fn message_start_with_cache(
    input: u32,
    cache_read: u32,
    cache_creation: u32,
) -> MessageStreamEvent {
    MessageStreamEvent::MessageStart {
        message: MessagesResponse {
            id: "msg_cache".into(),
            r#type: "message".into(),
            role: "assistant".into(),
            content: vec![],
            model: "messages-compatible-model".into(),
            stop_reason: None,
            usage: MessagesUsage {
                input_tokens: input,
                output_tokens: 0,
                cache_creation_input_tokens: cache_creation,
                cache_read_input_tokens: cache_read,
            },
        },
    }
}

fn message_delta_with_cache(
    output: u32,
    input: Option<u32>,
    cache_read: Option<u32>,
    cache_creation: Option<u32>,
) -> MessageStreamEvent {
    MessageStreamEvent::MessageDelta {
        delta: MessageDeltaBody {
            stop_reason: Some(messages::StopReason::EndTurn),
            stop_details: None,
        },
        usage: MessageDeltaUsage {
            output_tokens: output,
            input_tokens: input,
            cache_read_input_tokens: cache_read,
            cache_creation_input_tokens: cache_creation,
            ..Default::default()
        },
    }
}

/// Helper: drive a minimal stream with the supplied usage events and
/// pluck the `TokenUsage` out of the terminal `Completed` event.
async fn usage_from_stream(events: Vec<MessageStreamEvent>) -> TokenUsage {
    let raw = stream::iter(
        events
            .into_iter()
            .map(Ok::<_, SamplingError>)
            .collect::<Vec<_>>(),
    )
    .boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;
    match evs.last().expect("at least one event") {
        SamplingEvent::Completed { response, .. } => response
            .usage
            .clone()
            .expect("usage should be emitted when prompt or output tokens > 0"),
        other => panic!("expected Completed, got {other:?}"),
    }
}

/// Helper: drive a minimal stream and pluck the gateway-reported cost out of
/// the terminal `Completed` event.
async fn cost_from_stream(events: Vec<MessageStreamEvent>) -> Option<i64> {
    let raw = stream::iter(
        events
            .into_iter()
            .map(Ok::<_, SamplingError>)
            .collect::<Vec<_>>(),
    )
    .boxed();
    let evs = collect(stream_messages(raw, None, rid(), Duration::from_secs(60))).await;
    match evs.last().expect("at least one event") {
        SamplingEvent::Completed { response, .. } => response.cost_usd_ticks,
        other => panic!("expected Completed, got {other:?}"),
    }
}

fn message_delta_with_cost(ticks: Option<i64>, priced: Option<bool>) -> MessageStreamEvent {
    MessageStreamEvent::MessageDelta {
        delta: MessageDeltaBody {
            stop_reason: Some(messages::StopReason::EndTurn),
            stop_details: None,
        },
        usage: MessageDeltaUsage {
            output_tokens: 5,
            input_tokens: Some(10),
            ainxt_cost_in_usd_ticks: ticks,
            ainxt_cost_priced: priced,
            ..Default::default()
        },
    }
}

#[tokio::test]
async fn gateway_reported_cost_is_captured() {
    let cost = cost_from_stream(vec![
        message_start_with_cache(10, 0, 0),
        message_delta_with_cost(Some(1_234_500_000), Some(true)),
        MessageStreamEvent::MessageStop,
    ])
    .await;
    assert_eq!(cost, Some(1_234_500_000));
}

#[tokio::test]
async fn gateway_priced_zero_is_free_not_unknown() {
    // In-house models are billed at a real zero. That must survive as
    // `Some(0)` ("free"), not collapse to `None` ("unknown").
    let cost = cost_from_stream(vec![
        message_start_with_cache(10, 0, 0),
        message_delta_with_cost(Some(0), Some(true)),
        MessageStreamEvent::MessageStop,
    ])
    .await;
    assert_eq!(cost, Some(0));
}

#[tokio::test]
async fn unpriced_cost_is_unknown_never_free() {
    // The gateway answered "I could not price this". The CLI must not invent
    // a figure, and must not read the accompanying 0 as free.
    let cost = cost_from_stream(vec![
        message_start_with_cache(10, 0, 0),
        message_delta_with_cost(Some(0), Some(false)),
        MessageStreamEvent::MessageStop,
    ])
    .await;
    assert_eq!(cost, None);
}

#[tokio::test]
async fn absent_cost_extension_is_unknown() {
    // Stock Anthropic, or a gateway predating the extension: no cost fields
    // at all. Must be "unknown", never a silent zero.
    let cost = cost_from_stream(vec![
        message_start_with_cache(10, 0, 0),
        message_delta_with_cost(None, None),
        MessageStreamEvent::MessageStop,
    ])
    .await;
    assert_eq!(cost, None);
}

#[tokio::test]
async fn ainxt_cache_counts_override_gated_standard_fields() {
    // The gateway zeroes the standard cache fields when TRACK_CACHE_TOKENS is
    // off, but still reports the true counts on the ainxt extension.
    let usage = usage_from_stream(vec![
        message_start_with_cache(10, 0, 0),
        MessageStreamEvent::MessageDelta {
            delta: MessageDeltaBody {
                stop_reason: Some(messages::StopReason::EndTurn),
                stop_details: None,
            },
            usage: MessageDeltaUsage {
                output_tokens: 4,
                input_tokens: Some(10),
                cache_read_input_tokens: Some(0),
                cache_creation_input_tokens: Some(0),
                ainxt_cache_read_tokens: Some(900),
                ainxt_cache_write_tokens: Some(50),
                ..Default::default()
            },
        },
        MessageStreamEvent::MessageStop,
    ])
    .await;

    assert_eq!(usage.cached_prompt_tokens, 900);
    assert_eq!(usage.prompt_tokens, 10 + 900 + 50);
}

#[tokio::test]
async fn prompt_tokens_sums_all_three_anthropic_buckets() {
    // prompt_tokens = uncached + cache_read + cache_creation;
    // cached_prompt_tokens = cache_read only (writes aren't a hit).
    let usage = usage_from_stream(vec![
        message_start_with_cache(100, 5000, 200),
        text_block_start(0),
        text_delta(0, "ok"),
        block_stop(0),
        message_delta_with_cache(7, None, None, None),
        MessageStreamEvent::MessageStop,
    ])
    .await;

    assert_eq!(usage.prompt_tokens, 100 + 5000 + 200);
    assert_eq!(usage.cached_prompt_tokens, 5000);
    assert_eq!(usage.completion_tokens, 7);
    assert_eq!(usage.total_tokens, 100 + 5000 + 200 + 7);
}

#[tokio::test]
async fn message_delta_cache_fields_override_message_start() {
    // Providers can report zero cache at message_start and emit the real
    // values on the final delta; honor the delta when present.
    let usage = usage_from_stream(vec![
        message_start_with_cache(10, 0, 0),
        message_delta_with_cache(4, Some(10), Some(900), Some(50)),
        MessageStreamEvent::MessageStop,
    ])
    .await;

    assert_eq!(usage.prompt_tokens, 10 + 900 + 50);
    assert_eq!(usage.cached_prompt_tokens, 900);
    assert_eq!(usage.completion_tokens, 4);
}

#[tokio::test]
async fn pure_cache_hit_with_zero_uncached_still_emits_usage() {
    // 100% cache hit: Anthropic Messages API reports input_tokens=0 with cache_read>0.
    // The emit-guard must still fire so callers see the cached cost.
    let usage = usage_from_stream(vec![
        message_start_with_cache(0, 2500, 0),
        message_delta_with_cache(1, None, None, None),
        MessageStreamEvent::MessageStop,
    ])
    .await;

    assert_eq!(usage.prompt_tokens, 2500);
    assert_eq!(usage.cached_prompt_tokens, 2500);
    assert_eq!(usage.total_tokens, 2501);
}
