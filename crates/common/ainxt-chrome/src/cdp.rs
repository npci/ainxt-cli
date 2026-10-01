//! The DevTools Protocol session: one WebSocket, many concurrent commands.
//!
//! CDP multiplexes request/response and unsolicited events over a single
//! socket, correlating replies by an integer `id`. A reader task owns the
//! stream and routes each frame either to the oneshot channel waiting on that
//! id, or — for frames with no id — to the event sink.

use crate::error::{ChromeError, Result};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// Pending replies, keyed by CDP command id.
type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<serde_json::Value>>>>;

/// A live CDP session against one browser or page target.
pub struct CdpSession {
    tx: mpsc::UnboundedSender<Message>,
    pending: Pending,
    next_id: AtomicI64,
    /// Kept so the reader task is aborted when the session is dropped.
    reader: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for CdpSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpSession").finish_non_exhaustive()
    }
}

impl Drop for CdpSession {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl CdpSession {
    /// Connect to a DevTools WebSocket endpoint.
    pub async fn connect(ws_url: &str) -> Result<Self> {
        let (stream, _) = tokio_tungstenite::connect_async(ws_url).await?;
        let (mut sink, mut source) = stream.split();

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        // Writer: serialises all outbound frames onto the single sink.
        tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Reader: routes replies to their waiters, drops events on the floor
        // for now (the vertical slice has no event subscribers yet).
        let reader_pending = Arc::clone(&pending);
        let reader = tokio::spawn(async move {
            while let Some(Ok(msg)) = source.next().await {
                let Message::Text(text) = msg else { continue };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                let Some(id) = value.get("id").and_then(serde_json::Value::as_i64) else {
                    continue; // an event, not a reply
                };
                if let Some(waiter) = reader_pending.lock().await.remove(&id) {
                    let _ = waiter.send(value);
                }
            }
            // Socket closed: wake every waiter so callers get ConnectionClosed
            // instead of hanging forever.
            reader_pending.lock().await.clear();
        });

        Ok(Self {
            tx,
            pending,
            next_id: AtomicI64::new(1),
            reader,
        })
    }

    /// Issue a CDP command and await its result.
    pub async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (done_tx, done_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, done_tx);

        let frame = serde_json::json!({ "id": id, "method": method, "params": params });
        self.tx
            .send(Message::Text(frame.to_string().into()))
            .map_err(|_| ChromeError::ConnectionClosed)?;

        let reply = done_rx.await.map_err(|_| ChromeError::ConnectionClosed)?;

        if let Some(err) = reply.get("error") {
            let message = err
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown error")
                .to_owned();
            return Err(ChromeError::Command {
                method: method.to_owned(),
                message,
            });
        }

        reply
            .get("result")
            .cloned()
            .ok_or_else(|| ChromeError::Protocol {
                method: method.to_owned(),
                detail: "reply had neither `result` nor `error`".to_owned(),
            })
    }
}

/// One page (tab) in the browser.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PageTarget {
    /// DevTools target id.
    pub id: String,
    /// Current page title.
    #[serde(default)]
    pub title: String,
    /// Current URL.
    #[serde(default)]
    pub url: String,
    /// Per-page WebSocket endpoint.
    #[serde(rename = "webSocketDebuggerUrl", default)]
    pub ws_url: String,
    /// DevTools target type — "page", "iframe", "service_worker", ...
    #[serde(rename = "type", default)]
    pub kind: String,
}

/// List the browser's open page targets via the DevTools HTTP endpoint.
pub async fn list_pages(port: u16) -> Result<Vec<PageTarget>> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let targets: Vec<PageTarget> = reqwest::get(&url).await?.json().await?;
    Ok(targets.into_iter().filter(|t| t.kind == "page").collect())
}

/// Open a new tab and return it.
pub async fn new_page(port: u16, url: &str) -> Result<PageTarget> {
    let endpoint = format!(
        "http://127.0.0.1:{port}/json/new?{}",
        urlencode(url)
    );
    let client = reqwest::Client::new();
    // /json/new requires PUT on current Chrome; older builds accepted GET.
    let target: PageTarget = client.put(&endpoint).send().await?.json().await?;
    Ok(target)
}

/// Minimal percent-encoding for the characters that appear in URLs passed as
/// a query value. Avoids pulling in a dependency for one call site.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_preserves_url_shape_and_escapes_the_rest() {
        assert_eq!(urlencode("https://example.com/a"), "https://example.com/a");
        assert_eq!(urlencode("a b"), "a%20b");
        assert_eq!(urlencode("?x=1&y=2"), "%3Fx%3D1%26y%3D2");
    }

    #[test]
    fn page_targets_deserialize_and_non_pages_are_filtered_by_kind() {
        let json = serde_json::json!({
            "id": "ABC",
            "title": "Example",
            "url": "https://example.com",
            "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/ABC",
            "type": "page"
        });
        let t: PageTarget = serde_json::from_value(json).unwrap();
        assert_eq!(t.id, "ABC");
        assert_eq!(t.kind, "page");
    }

    #[tokio::test]
    async fn connect_to_a_dead_endpoint_fails_rather_than_hangs() {
        let result = CdpSession::connect("ws://127.0.0.1:1/devtools/browser/none").await;
        assert!(result.is_err());
    }
}
