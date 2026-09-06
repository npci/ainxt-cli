//! Layer-2 stream transform for the Chat Completions API.
//!
//! Consumes a raw `ChatCompletionChunk` stream and produces
//! [`SamplingEvent`]s. Pure: no I/O, no shell coupling.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream::{BoxStream, Stream};

use ainxt_sampling_types::{
    AssistantItem, ChatCompletionChunk, ConversationItem, ConversationResponse,
    ResponseModelMetadata, SamplingError, StopReason, TokenUsage, ToolCall,
};

use crate::events::{SamplingChannel, SamplingErrorInfo, SamplingEvent};
use crate::metrics::InferenceLatencyStats;
use crate::types::RequestId;

/// One tool call being assembled from streaming deltas.
struct ToolCallSlot {
    /// The `index` the server reported for this call. Kept only to order the
    /// final list; it is not an identity, because servers may omit it.
    wire_index: u32,
    id: String,
    name: String,
    arguments: String,
}

/// Pick the slot a tool-call delta belongs to, opening a new one if needed.
///
/// * A delta carrying an `id` that matches a slot already in flight continues
///   that slot (servers may repeat the id on every fragment).
/// * A delta carrying an `id` that matches nothing is a *new* call — even when
///   the server reused the same `index`. This is what keeps N parallel calls
///   from collapsing into one when a server never advances `index`.
/// * A delta with no `id` is a continuation fragment: it attaches to the most
///   recent slot with that wire index, or opens one if this index is new.
///
/// The remaining ambiguous case — a server that sends neither ids nor
/// advancing indices — is indistinguishable from a single call at this layer
/// and still merges; nothing on the wire separates the calls.
fn resolve_tool_call_slot(
    slots: &mut Vec<ToolCallSlot>,
    wire_index: u32,
    id: Option<&str>,
) -> usize {
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        if let Some(pos) = slots.iter().position(|slot| slot.id == id) {
            return pos;
        }
    } else if let Some(pos) = slots.iter().rposition(|slot| slot.wire_index == wire_index) {
        return pos;
    }
    slots.push(ToolCallSlot {
        wire_index,
        id: String::new(),
        name: String::new(),
        arguments: String::new(),
    });
    slots.len() - 1
}

/// Transform a raw Chat Completions chunk stream into a stream of
/// [`SamplingEvent`]s.
///
/// The output stream emits exactly one terminal event per request:
/// [`SamplingEvent::Completed`] on normal stream end, or
/// [`SamplingEvent::Failed`] on error / idle timeout. Callers must not
/// consume past the terminal event (the implementation `return`s after
/// yielding it).
///
/// `idle_timeout` covers two cases:
/// 1. The transport stops yielding chunks at all (`tokio::time::timeout`).
/// 2. The transport keeps yielding empty / keepalive chunks but no
///    meaningful content (separate `last_content_chunk_at` timer).
///
/// Both produce `SamplingEvent::Failed { kind: IdleTimeout }`.
pub fn stream_chat_completions<'a>(
    raw_stream: BoxStream<'a, Result<ChatCompletionChunk, SamplingError>>,
    model_metadata: Option<ResponseModelMetadata>,
    request_id: RequestId,
    idle_timeout: Duration,
) -> impl Stream<Item = SamplingEvent> + Send + 'a {
    async_stream::stream! {
        let stream_start = Instant::now();
        let mut chunk_timestamps: Vec<Instant> = Vec::new();

        // Emit StreamStarted before reading any chunks so subscribers
        // can record TTFB / TTLB baselines.
        yield SamplingEvent::StreamStarted {
            request_id: request_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };

        if let Some(metadata) = model_metadata {
            yield SamplingEvent::ModelMetadata {
                request_id: request_id.clone(),
                metadata,
            };
        }

        // Per-response accumulators
        let mut first_chunk_seen = false;
        let mut first_choice_seen = false;
        let mut first_token_emitted = false;
        let mut model: String = String::new();
        let mut model_fingerprint: Option<String> = None;
        let mut usage: Option<TokenUsage> = None;
        let mut cost_usd_ticks: Option<i64> = None;
        let mut finish_reason: Option<StopReason> = None;

        let mut content_acc = String::new();
        let mut reasoning_acc = String::new();
        // Tool calls under construction, in arrival order. The first chunk for
        // a call carries id+name and starts the arguments buffer; subsequent
        // chunks append to arguments only.
        //
        // Deliberately NOT keyed by the wire `index` alone: that field is
        // `#[serde(default)]`, so a server that omits it (or hardcodes 0) makes
        // every tool call in a turn land on index 0. Keying on it alone then
        // overwrites each call's id/name with the next one's and *concatenates*
        // their argument buffers into invalid JSON, silently turning N parallel
        // calls into one broken call. A delta bearing an `id` that does not
        // match a call already in flight is unambiguously a new call, so
        // [`resolve_tool_call_slot`] opens a new slot for it.
        let mut tool_call_acc: Vec<ToolCallSlot> = Vec::new();

        // Index counter spanning text + reasoning chunks (matches the
        // shell's chunk_index used for notification correlation).
        let mut chunk_index: u64 = 0;
        // Separate counter for AgentMessageChunk (text-only) emissions;
        // mirrored onto ConversationResponse.message_chunks_emitted so
        // downstream can detect lost-streaming-events scenarios.
        let mut message_chunk_count: u64 = 0;

        // Content-aware idle timer: the outer
        // `tokio::time::timeout(idle_timeout, stream.next())` already
        // catches "transport stops yielding chunks". This second timer
        // catches the more subtle case where the model keeps emitting
        // keepalive / empty-delta SSE events that satisfy the outer
        // timer but make no real progress -- some inference engines
        // do exactly that.
        let mut last_content_chunk_at = Instant::now();

        let mut stream = raw_stream;
        loop {
            let next = match tokio::time::timeout(idle_timeout, stream.next()).await {
                Ok(Some(next)) => next,
                Ok(None) => break, // stream ended normally
                Err(_elapsed) => {
                    let err = SamplingError::IdleTimeout {
                        elapsed_secs: idle_timeout.as_secs(),
                    };
                    yield SamplingEvent::Failed {
                        request_id: request_id.clone(),
                        error: SamplingErrorInfo::from(&err),
                    };
                    return;
                }
            };
            let chunk = match next {
                Ok(chunk) => chunk,
                Err(err) => {
                    yield SamplingEvent::Failed {
                        request_id: request_id.clone(),
                        error: SamplingErrorInfo::from(&err),
                    };
                    return;
                }
            };

            if !first_chunk_seen {
                model = chunk.model.clone();
                model_fingerprint = chunk
                    .system_fingerprint
                    .clone()
                    .filter(|s| !s.is_empty());
                first_chunk_seen = true;
            }

            if let Some(u) = chunk.usage.clone() {
                // Wire cost is cumulative for the response, so last-write-wins.
                // Never clobber a known cost with missing/unreported.
                let chunk_cost = ainxt_sampling_types::reported_cost_ticks(u.cost_in_usd_ticks);
                cost_usd_ticks = match (cost_usd_ticks, chunk_cost) {
                    (_, Some(n)) => Some(n),
                    (prev, None) => prev,
                };
                usage = Some(u.into());
            }

            // Track whether this chunk carried meaningful content.
            // Set inside the choices loop and checked at the end.
            let mut chunk_has_content = false;

            for choice in chunk.choices.into_iter() {
                first_choice_seen = true;
                if let Some(fr) = choice.finish_reason {
                    finish_reason = Some(fr.into());
                    chunk_has_content = true;
                }

                let delta = choice.delta;

                if let Some(text) = delta.content
                    && !text.is_empty()
                {
                    if !first_token_emitted {
                        first_token_emitted = true;
                        yield SamplingEvent::FirstToken {
                            request_id: request_id.clone(),
                        };
                    }
                    chunk_has_content = true;
                    chunk_timestamps.push(Instant::now());
                    chunk_index += 1;
                    message_chunk_count += 1;
                    content_acc.push_str(&text);
                    yield SamplingEvent::ChannelToken {
                        request_id: request_id.clone(),
                        channel: SamplingChannel::Text,
                        text,
                        chunk_index,
                    };
                }

                if let Some(thought) = delta.reasoning_content
                    && !thought.is_empty()
                {
                    if !first_token_emitted {
                        first_token_emitted = true;
                        yield SamplingEvent::FirstToken {
                            request_id: request_id.clone(),
                        };
                    }
                    chunk_has_content = true;
                    chunk_index += 1;
                    reasoning_acc.push_str(&thought);
                    yield SamplingEvent::ChannelToken {
                        request_id: request_id.clone(),
                        channel: SamplingChannel::Reasoning,
                        text: thought,
                        chunk_index,
                    };
                }

                for tc_delta in delta.tool_calls.into_iter() {
                    chunk_has_content = true;

                    let slot = resolve_tool_call_slot(
                        &mut tool_call_acc,
                        tc_delta.index,
                        tc_delta.id.as_deref(),
                    );
                    let entry = &mut tool_call_acc[slot];

                    let mut id_for_event: Option<String> = None;
                    let mut name_for_event: Option<String> = None;
                    let mut args_for_event: Option<String> = None;

                    if let Some(id) = tc_delta.id {
                        entry.id = id.clone();
                        id_for_event = Some(id);
                    }
                    if let Some(func) = tc_delta.function {
                        if let Some(name) = func.name {
                            entry.name = name.clone();
                            name_for_event = Some(name);
                        }
                        if let Some(args) = func.arguments {
                            entry.arguments.push_str(&args);
                            args_for_event = Some(args);
                        }
                    }

                    // The slot index, not the wire index: downstream correlates
                    // streaming tool-call UI by `tool_index`, so it has to be
                    // unique per call even when the server never advances its
                    // own index.
                    yield SamplingEvent::ToolCallDelta {
                        request_id: request_id.clone(),
                        tool_index: slot as u32,
                        id: id_for_event,
                        name: name_for_event,
                        arguments_delta: args_for_event,
                    };
                }
            }

            if chunk_has_content {
                last_content_chunk_at = Instant::now();
            } else if last_content_chunk_at.elapsed() > idle_timeout {
                let err = SamplingError::IdleTimeout {
                    elapsed_secs: idle_timeout.as_secs(),
                };
                yield SamplingEvent::Failed {
                    request_id: request_id.clone(),
                    error: SamplingErrorInfo::from(&err),
                };
                return;
            }
        }

        // ── Build the final response ─────────────────────────────────
        // Stable-sort by the wire index so a server that reports positions
        // out of order still yields them in its intended order; within one
        // wire index (the "server never advances index" case) arrival order
        // is preserved. This matches the previous index-keyed ordering for
        // any server that advances the index normally.
        tool_call_acc.sort_by_key(|slot| slot.wire_index);
        let tool_calls: Vec<ToolCall> = tool_call_acc
            .into_iter()
            .map(|slot| ToolCall {
                id: std::sync::Arc::<str>::from(slot.id),
                name: slot.name,
                arguments: std::sync::Arc::<str>::from(slot.arguments),
            })
            .collect();

        // Honor tool calls by overriding the stop reason if the model
        // forgot to set it (matches the shell's behavior).
        if !tool_calls.is_empty() {
            finish_reason = Some(StopReason::ToolCalls);
        }

        // Build the trailing Assistant + any reasoning sibling.
        let mut items: Vec<ConversationItem> = Vec::new();
        if first_choice_seen {
            if !reasoning_acc.is_empty() {
                items.push(ConversationItem::Reasoning(
                    ainxt_sampling_types::synthesized_reasoning_item(reasoning_acc),
                ));
            }
            items.push(ConversationItem::Assistant(AssistantItem {
                content: std::sync::Arc::<str>::from(content_acc),
                tool_calls,
                model_id: Some(model),
                model_fingerprint,
                // Chat Completions does not echo the applied reasoning effort.
                reasoning_effort: None,
            }));
        } else {
            items.push(ConversationItem::assistant(""));
        }

        let stream_end = Instant::now();
        let metrics =
            InferenceLatencyStats::from_timestamps(stream_start, &chunk_timestamps, stream_end);

        let response = ConversationResponse {
            items,
            stop_reason: finish_reason,
            usage,
            cost_usd_ticks,
            message_chunks_emitted: message_chunk_count,
            doom_loop_signals: Vec::new(),
            stop_message: None,
        };

        yield SamplingEvent::Completed {
            request_id: request_id.clone(),
            response: Box::new(response),
            metrics,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::pin::pin;
    use ainxt_sampling_types::{
        ChatChunkChoice, ChatChunkDelta, FinishReason, Role, ToolCallDelta as ChunkToolCallDelta,
        ToolCallFunctionDelta, Usage, rs,
    };

    fn rid() -> RequestId {
        RequestId::from("test-req")
    }

    fn make_chunk(deltas: Vec<ChatChunkDelta>) -> ChatCompletionChunk {
        ChatCompletionChunk {
            id: "chunk-1".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test-model".into(),
            choices: deltas
                .into_iter()
                .enumerate()
                .map(|(i, delta)| ChatChunkChoice {
                    index: i as u32,
                    delta,
                    finish_reason: None,
                })
                .collect(),
            usage: None,
            system_fingerprint: None,
        }
    }

    fn text_chunk(text: &str) -> ChatCompletionChunk {
        make_chunk(vec![ChatChunkDelta {
            role: Some(Role::Assistant),
            content: Some(text.to_string()),
            reasoning_content: None,
            tool_calls: vec![],
            tool_call_id: None,
        }])
    }

    fn final_chunk(reason: FinishReason) -> ChatCompletionChunk {
        let mut chunk = make_chunk(vec![ChatChunkDelta::default()]);
        chunk.choices[0].finish_reason = Some(reason);
        chunk
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
        let raw = stream::iter(Vec::<Result<ChatCompletionChunk, SamplingError>>::new()).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
        match &events[1] {
            SamplingEvent::Completed { response, .. } => {
                assert!(response.is_empty());
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn text_only_stream_emits_first_token_then_channel_tokens_then_completed() {
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("Hello, ")),
            Ok(text_chunk("world!")),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        // Expected sequence: StreamStarted, FirstToken, ChannelToken(Text)
        // x 2, Completed.
        assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
        assert!(matches!(events[1], SamplingEvent::FirstToken { .. }));

        let text_tokens: Vec<&str> = events
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

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let a = response.assistant().expect("assistant item present");
                assert_eq!(a.content.as_ref(), "Hello, world!");
                assert_eq!(response.stop_reason, Some(StopReason::Stop));
                assert_eq!(response.message_chunks_emitted, 2);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reasoning_chunk_emits_reasoning_channel_and_first_token_once() {
        let mut reasoning_chunk = make_chunk(vec![ChatChunkDelta {
            role: Some(Role::Assistant),
            content: None,
            reasoning_content: Some("thinking...".into()),
            tool_calls: vec![],
            tool_call_id: None,
        }]);
        reasoning_chunk.choices[0].finish_reason = None;

        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(reasoning_chunk),
            Ok(text_chunk("done")),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        // FirstToken should appear exactly once.
        let first_token_count = events
            .iter()
            .filter(|e| matches!(e, SamplingEvent::FirstToken { .. }))
            .count();
        assert_eq!(first_token_count, 1);

        let mut saw_reasoning = false;
        let mut saw_text = false;
        for e in &events {
            if let SamplingEvent::ChannelToken { channel, text, .. } = e {
                match channel {
                    SamplingChannel::Reasoning => {
                        assert_eq!(text, "thinking...");
                        saw_reasoning = true;
                    }
                    SamplingChannel::Text => {
                        assert_eq!(text, "done");
                        saw_text = true;
                    }
                }
            }
        }
        assert!(saw_reasoning && saw_text);

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let r = response
                    .reasoning_items()
                    .next()
                    .expect("reasoning sibling preserved");
                let rs::SummaryPart::SummaryText(t) = &r.summary[0];
                assert_eq!(t.text, "thinking...");
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// Two DISTINCT tool calls that a server streams without ever advancing
    /// `index` (it omits the field, so `#[serde(default)]` makes it 0, or it
    /// hardcodes 0). Both calls carry their own `id`, so they are
    /// unambiguously separate calls.
    ///
    /// Correct behavior: two tool calls out. Broken behavior: one call whose
    /// arguments are the two JSON objects concatenated into invalid JSON.
    #[tokio::test]
    async fn distinct_ids_at_same_index_do_not_merge() {
        let call = |id: &str, name: &str, args: &str| {
            make_chunk(vec![ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: vec![ChunkToolCallDelta {
                    index: 0,
                    id: Some(id.into()),
                    kind: Some("function".into()),
                    function: Some(ToolCallFunctionDelta {
                        name: Some(name.into()),
                        arguments: Some(args.into()),
                    }),
                }],
                tool_call_id: None,
            }])
        };

        let raw = stream::iter::<Vec<Result<ChatCompletionChunk, SamplingError>>>(vec![
            Ok(call("call_1", "read_file", r#"{"path":"a.rs"}"#)),
            Ok(call("call_2", "read_file", r#"{"path":"b.rs"}"#)),
        ])
        .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let calls = response.tool_calls();
                assert_eq!(
                    calls.len(),
                    2,
                    "two distinct tool-call ids must yield two calls, got: {calls:?}"
                );
                assert_eq!(calls[0].id.as_ref(), "call_1");
                assert_eq!(calls[0].arguments.as_ref(), r#"{"path":"a.rs"}"#);
                assert_eq!(calls[1].id.as_ref(), "call_2");
                assert_eq!(calls[1].arguments.as_ref(), r#"{"path":"b.rs"}"#);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// A well-behaved server that advances `index` and interleaves the argument
    /// fragments of two parallel calls must still produce exactly two calls,
    /// each with its own arguments. Guards the fix against regressing the
    /// normal OpenAI-faithful streaming shape.
    #[tokio::test]
    async fn interleaved_parallel_calls_with_advancing_index_stay_separate() {
        let frag = |index: u32, id: Option<&str>, name: Option<&str>, args: &str| {
            make_chunk(vec![ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: vec![ChunkToolCallDelta {
                    index,
                    id: id.map(Into::into),
                    kind: id.map(|_| "function".to_string()),
                    function: Some(ToolCallFunctionDelta {
                        name: name.map(Into::into),
                        arguments: Some(args.into()),
                    }),
                }],
                tool_call_id: None,
            }])
        };

        let raw = stream::iter::<Vec<Result<ChatCompletionChunk, SamplingError>>>(vec![
            Ok(frag(0, Some("call_a"), Some("read_file"), r#"{"path":"#)),
            Ok(frag(1, Some("call_b"), Some("write_file"), r#"{"path":"#)),
            Ok(frag(0, None, None, r#""a.rs"}"#)),
            Ok(frag(1, None, None, r#""b.rs"}"#)),
        ])
        .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let calls = response.tool_calls();
                assert_eq!(calls.len(), 2, "got: {calls:?}");
                assert_eq!(calls[0].id.as_ref(), "call_a");
                assert_eq!(calls[0].name, "read_file");
                assert_eq!(calls[0].arguments.as_ref(), r#"{"path":"a.rs"}"#);
                assert_eq!(calls[1].id.as_ref(), "call_b");
                assert_eq!(calls[1].name, "write_file");
                assert_eq!(calls[1].arguments.as_ref(), r#"{"path":"b.rs"}"#);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// A server that repeats the same `id` on every fragment (rather than only
    /// the first) must not open a new call per fragment.
    #[tokio::test]
    async fn repeated_id_on_every_fragment_stays_one_call() {
        let frag = |args: &str| {
            make_chunk(vec![ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: vec![ChunkToolCallDelta {
                    index: 0,
                    id: Some("call_same".into()),
                    kind: None,
                    function: Some(ToolCallFunctionDelta {
                        name: None,
                        arguments: Some(args.into()),
                    }),
                }],
                tool_call_id: None,
            }])
        };

        let raw = stream::iter::<Vec<Result<ChatCompletionChunk, SamplingError>>>(vec![
            Ok(frag(r#"{"path":"#)),
            Ok(frag(r#""a.rs"}"#)),
        ])
        .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let calls = response.tool_calls();
                assert_eq!(calls.len(), 1, "got: {calls:?}");
                assert_eq!(calls[0].arguments.as_ref(), r#"{"path":"a.rs"}"#);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// With `index` pinned at 0, an id-less continuation fragment must attach
    /// to the most recent call, not the first one.
    #[tokio::test]
    async fn idless_fragment_attaches_to_most_recent_call_at_that_index() {
        let opener = |id: &str, args: &str| {
            make_chunk(vec![ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: vec![ChunkToolCallDelta {
                    index: 0,
                    id: Some(id.into()),
                    kind: Some("function".into()),
                    function: Some(ToolCallFunctionDelta {
                        name: Some("read_file".into()),
                        arguments: Some(args.into()),
                    }),
                }],
                tool_call_id: None,
            }])
        };
        let continuation = |args: &str| {
            make_chunk(vec![ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: vec![ChunkToolCallDelta {
                    index: 0,
                    id: None,
                    kind: None,
                    function: Some(ToolCallFunctionDelta {
                        name: None,
                        arguments: Some(args.into()),
                    }),
                }],
                tool_call_id: None,
            }])
        };

        let raw = stream::iter::<Vec<Result<ChatCompletionChunk, SamplingError>>>(vec![
            Ok(opener("call_1", r#"{"path":"a.rs"}"#)),
            Ok(opener("call_2", r#"{"path":"#)),
            Ok(continuation(r#""b.rs"}"#)),
        ])
        .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let calls = response.tool_calls();
                assert_eq!(calls.len(), 2, "got: {calls:?}");
                assert_eq!(calls[0].arguments.as_ref(), r#"{"path":"a.rs"}"#);
                assert_eq!(calls[1].id.as_ref(), "call_2");
                assert_eq!(calls[1].arguments.as_ref(), r#"{"path":"b.rs"}"#);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// Every call in a turn must get a distinct streaming `tool_index`, even
    /// when the server reports the same wire index for all of them —
    /// downstream correlates in-flight tool-call UI by that number.
    #[tokio::test]
    async fn streamed_tool_index_is_unique_per_call() {
        let call = |id: &str| {
            make_chunk(vec![ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: vec![ChunkToolCallDelta {
                    index: 0,
                    id: Some(id.into()),
                    kind: Some("function".into()),
                    function: Some(ToolCallFunctionDelta {
                        name: Some("read_file".into()),
                        arguments: Some("{}".into()),
                    }),
                }],
                tool_call_id: None,
            }])
        };

        let raw = stream::iter::<Vec<Result<ChatCompletionChunk, SamplingError>>>(vec![
            Ok(call("call_1")),
            Ok(call("call_2")),
            Ok(call("call_3")),
        ])
        .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        let indices: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                SamplingEvent::ToolCallDelta { tool_index, .. } => Some(*tool_index),
                _ => None,
            })
            .collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[tokio::test]
    async fn tool_call_stream_emits_deltas_and_assembles_final_call() {
        // First chunk has id + name + part of arguments.
        let chunk1 = make_chunk(vec![ChatChunkDelta {
            role: None,
            content: None,
            reasoning_content: None,
            tool_calls: vec![ChunkToolCallDelta {
                index: 0,
                id: Some("call_abc".into()),
                kind: Some("function".into()),
                function: Some(ToolCallFunctionDelta {
                    name: Some("do_thing".into()),
                    arguments: Some("{\"x\":".into()),
                }),
            }],
            tool_call_id: None,
        }]);
        // Second chunk has only argument fragment.
        let chunk2 = make_chunk(vec![ChatChunkDelta {
            role: None,
            content: None,
            reasoning_content: None,
            tool_calls: vec![ChunkToolCallDelta {
                index: 0,
                id: None,
                kind: None,
                function: Some(ToolCallFunctionDelta {
                    name: None,
                    arguments: Some("1}".into()),
                }),
            }],
            tool_call_id: None,
        }]);

        let raw = stream::iter::<Vec<Result<ChatCompletionChunk, SamplingError>>>(vec![
            Ok(chunk1),
            Ok(chunk2),
        ])
        .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        let deltas: Vec<_> = events
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

        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0].0, 0);
        assert_eq!(deltas[0].1.as_deref(), Some("call_abc"));
        assert_eq!(deltas[0].2.as_deref(), Some("do_thing"));
        assert_eq!(deltas[0].3.as_deref(), Some("{\"x\":"));
        assert_eq!(deltas[1].1, None);
        assert_eq!(deltas[1].2, None);
        assert_eq!(deltas[1].3.as_deref(), Some("1}"));

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let calls = response.tool_calls();
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id.as_ref(), "call_abc");
                assert_eq!(calls[0].name, "do_thing");
                assert_eq!(calls[0].arguments.as_ref(), "{\"x\":1}");
                // Tool calls force ToolCalls stop reason.
                assert_eq!(response.stop_reason, Some(StopReason::ToolCalls));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mid_stream_error_yields_failed_no_completed() {
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("hi")),
            Err(SamplingError::EventStreamError("conn reset".into())),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        assert!(
            events
                .iter()
                .any(|e| matches!(e, SamplingEvent::Failed { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SamplingEvent::Completed { .. }))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_when_stream_stalls() {
        // A stream that yields one chunk then hangs forever.
        let raw = stream::iter(vec![Ok(text_chunk("hello"))])
            .chain(stream::pending())
            .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_millis(100),
        ))
        .await;

        // Stream should emit StreamStarted, FirstToken, ChannelToken
        // then Failed(IdleTimeout) when the stall hits the deadline.
        match events.last().unwrap() {
            SamplingEvent::Failed { error, .. } => {
                assert_eq!(error.kind, crate::events::SamplingErrorKind::IdleTimeout);
            }
            other => panic!("expected Failed(IdleTimeout), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn model_metadata_yielded_after_stream_started() {
        let raw = stream::iter(Vec::<Result<ChatCompletionChunk, SamplingError>>::new()).boxed();
        let metadata = ResponseModelMetadata {
            context_window: Some(8192),
            max_completion_tokens: Some(4096),
            models_etag: None,
        };
        let events = collect(stream_chat_completions(
            raw,
            Some(metadata.clone()),
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
        match &events[1] {
            SamplingEvent::ModelMetadata { metadata: m, .. } => {
                assert_eq!(m.context_window, Some(8192));
                assert_eq!(m.max_completion_tokens, Some(4096));
            }
            other => panic!("expected ModelMetadata second, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn usage_is_extracted_from_chunk() {
        let mut chunk_with_usage = make_chunk(vec![ChatChunkDelta::default()]);
        chunk_with_usage.usage = Some(Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            prompt_tokens_details: None,
            completion_tokens_details: None,
            cost_in_usd_ticks: None,
        });

        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("ok")),
            Ok(chunk_with_usage),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let u = response.usage.as_ref().expect("usage extracted");
                assert_eq!(u.prompt_tokens, 100);
                assert_eq!(u.completion_tokens, 50);
                assert_eq!(u.total_tokens, 150);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// Server-reported cost lands on the response; the REST mapper's `0`
    /// backfill means "unreported" and must yield `None`.
    #[tokio::test]
    async fn cost_is_extracted_and_zero_is_unreported() {
        for (wire, expected) in [(Some(78), Some(78)), (Some(0), None), (None, None)] {
            let mut chunk_with_usage = make_chunk(vec![ChatChunkDelta::default()]);
            chunk_with_usage.usage = Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                prompt_tokens_details: None,
                completion_tokens_details: None,
                cost_in_usd_ticks: wire,
            });
            let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
                Ok(text_chunk("ok")),
                Ok(chunk_with_usage),
                Ok(final_chunk(FinishReason::Stop)),
            ];
            let raw = stream::iter(chunks).boxed();
            let events = collect(stream_chat_completions(
                raw,
                None,
                rid(),
                Duration::from_secs(60),
            ))
            .await;
            match events.last().unwrap() {
                SamplingEvent::Completed { response, .. } => {
                    assert_eq!(response.cost_usd_ticks, expected, "wire {wire:?}");
                }
                other => panic!("expected Completed, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn later_missing_cost_does_not_clobber_earlier_ticks() {
        let mut first = make_chunk(vec![ChatChunkDelta::default()]);
        first.usage = Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: None,
            completion_tokens_details: None,
            cost_in_usd_ticks: Some(99),
        });
        let mut second = make_chunk(vec![ChatChunkDelta::default()]);
        second.usage = Some(Usage {
            prompt_tokens: 12,
            completion_tokens: 6,
            total_tokens: 18,
            prompt_tokens_details: None,
            completion_tokens_details: None,
            cost_in_usd_ticks: Some(0),
        });
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("ok")),
            Ok(first),
            Ok(second),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
        ))
        .await;
        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                assert_eq!(response.cost_usd_ticks, Some(99));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }
}
