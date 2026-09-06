//! Sampling log — emits `tracing` events with `target: "sampling_log"`.
//! A dedicated layer in `ainxt-telemetry` routes these to
//! `~/.ainxt/logs/sampling.jsonl`. Enable with `--log-sampling`.

use crate::types::RequestId;

pub const TARGET: &str = "sampling_log";

#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub auth_type: &'static str,
    pub auth_prefix: Option<String>,
}

pub fn request_span(
    request_id: &RequestId,
    model: &str,
    api_backend: &str,
    base_url: &str,
    auth: &AuthInfo,
) -> tracing::Span {
    tracing::info_span!(
        target: TARGET,
        "sampling_request",
        request_id = %request_id,
        model = model,
        api_backend = api_backend,
        base_url = base_url,
        auth_type = auth.auth_type,
        auth_prefix = auth.auth_prefix.as_deref().unwrap_or(""),
        // Recorded from `SamplerConfig` / response usage as the request
        // progresses; `field::Empty` lets callers `record()` them later.
        reasoning_effort = tracing::field::Empty,
        output_tokens = tracing::field::Empty,
        reasoning_tokens = tracing::field::Empty,
    )
}

/// Cap on how much of a request/response body one event records. Bodies carry
/// the whole conversation, so an uncapped dump would make the log unusable
/// (and unbounded) on a long session.
pub const MAX_BODY_BYTES: usize = 128 * 1024;

/// Truncate to at most `max` bytes on a UTF-8 boundary.
fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Record the exact JSON body of an outgoing request.
///
/// Only fires when the sampling log is enabled (`--log-sampling` /
/// `AINXT_LOG_SAMPLING=1`); the layer drops the event otherwise, so this costs
/// a formatting call and nothing else in normal runs.
///
/// The body is the conversation verbatim — prompts, file contents, tool output.
/// It stays on disk in `~/.ainxt/logs/sampling.jsonl` and is never uploaded,
/// but treat that file as sensitive. API keys are not included: credentials
/// travel in headers, which are redacted separately.
pub fn log_request_body(endpoint: &str, model: &str, body: &[u8]) {
    let text = String::from_utf8_lossy(body);
    let truncated = text.len() > MAX_BODY_BYTES;
    tracing::info!(
        target: TARGET,
        event = "request_body",
        endpoint = endpoint,
        model = model,
        body_bytes = body.len(),
        truncated = truncated,
        body = %truncate_on_char_boundary(&text, MAX_BODY_BYTES),
    );
}

/// Record the status and body of a failed response, so a rejection can be read
/// next to the request that caused it.
pub fn log_error_response(endpoint: &str, model: &str, status: u16, body: &str) {
    tracing::info!(
        target: TARGET,
        event = "error_response",
        endpoint = endpoint,
        model = model,
        status = status,
        body = %truncate_on_char_boundary(body, MAX_BODY_BYTES),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_never_splits_a_multibyte_char() {
        // "é" is two bytes; cutting at 1 must fall back to 0.
        assert_eq!(truncate_on_char_boundary("é", 1), "");
        assert_eq!(truncate_on_char_boundary("aé", 2), "a");
        assert_eq!(truncate_on_char_boundary("aé", 3), "aé");
    }

    #[test]
    fn short_bodies_pass_through_unchanged() {
        assert_eq!(truncate_on_char_boundary("{}", MAX_BODY_BYTES), "{}");
    }
}
