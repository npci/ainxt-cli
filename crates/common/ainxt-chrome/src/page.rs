//! Page-level operations: navigate, and read the page back as structured text.

use crate::cdp::CdpSession;
use crate::error::{ChromeError, Result};
use std::time::Duration;

/// A CDP session bound to one page target.
#[derive(Debug)]
pub struct Page {
    session: CdpSession,
    target_id: String,
}

/// What a page looked like after an operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PageInfo {
    /// Final URL, after any redirects.
    pub url: String,
    /// Document title.
    pub title: String,
}

impl Page {
    /// Attach to a page target's WebSocket endpoint.
    pub async fn attach(ws_url: &str, target_id: impl Into<String>) -> Result<Self> {
        let session = CdpSession::connect(ws_url).await?;
        // Page events drive load detection; Runtime backs the readers below.
        session.call("Page.enable", serde_json::json!({})).await?;
        session.call("Runtime.enable", serde_json::json!({})).await?;
        // The DOM agent must be enabled before backendNodeId lookups
        // (getBoxModel, focus) resolve to anything.
        session.call("DOM.enable", serde_json::json!({})).await?;
        Ok(Self {
            session,
            target_id: target_id.into(),
        })
    }

    /// The DevTools target id this page is bound to.
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    /// Navigate to `url` and wait until the document is ready.
    pub async fn navigate(&self, url: &str, timeout: Duration) -> Result<PageInfo> {
        validate_scheme(url)?;

        let result = self
            .session
            .call("Page.navigate", serde_json::json!({ "url": url }))
            .await?;

        // Chrome reports a refused navigation in the result rather than as a
        // CDP error, so a bad scheme or blocked URL surfaces here.
        if let Some(err) = result.get("errorText").and_then(serde_json::Value::as_str) {
            return Err(ChromeError::Command {
                method: "Page.navigate".to_owned(),
                message: format!("{err} (navigating to {url})"),
            });
        }

        self.wait_for_ready(timeout).await?;
        self.info().await
    }

    /// Poll `document.readyState` until the document finishes loading.
    ///
    /// Polling rather than waiting on `Page.loadEventFired` keeps this working
    /// for navigations that were already complete before the call, which the
    /// event-based approach would wait out to the full timeout.
    async fn wait_for_ready(&self, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            let state = self.eval_string("document.readyState").await?;
            if state == "complete" || state == "interactive" {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(ChromeError::Command {
            method: "Page.navigate".to_owned(),
            message: format!("document did not finish loading within {}s", timeout.as_secs()),
        })
    }

    /// Current URL and title.
    pub async fn info(&self) -> Result<PageInfo> {
        Ok(PageInfo {
            url: self.eval_string("location.href").await?,
            title: self.eval_string("document.title").await?,
        })
    }

    /// Evaluate an expression and coerce the result to a string.
    async fn eval_string(&self, expression: &str) -> Result<String> {
        let result = self
            .session
            .call(
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;

        if let Some(details) = result.get("exceptionDetails") {
            let text = details
                .get("exception")
                .and_then(|e| e.get("description"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("script threw");
            return Err(ChromeError::Command {
                method: "Runtime.evaluate".to_owned(),
                message: text.to_owned(),
            });
        }

        Ok(result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// Click the element with this `backendDOMNodeId` — the `[ref=N]` handle
    /// from [`Self::read_accessibility_tree`].
    ///
    /// Dispatches real mouse events at the element's centre rather than
    /// calling `.click()` in JS: that way hover handlers, focus changes and
    /// event delegation all behave as they do for a human.
    pub async fn click(&self, backend_node_id: i64) -> Result<()> {
        // An element below the fold has a box model, but clicking its
        // coordinates would hit whatever is actually at that point.
        self.session
            .call(
                "DOM.scrollIntoViewIfNeeded",
                serde_json::json!({ "backendNodeId": backend_node_id }),
            )
            .await?;

        let (x, y) = self.element_center(backend_node_id).await?;

        // A click that navigates does so asynchronously. Remember where we
        // were so `settle` can tell a navigation from an in-page click.
        let url_before = self.eval_string("location.href").await.unwrap_or_default();

        for event in ["mousePressed", "mouseReleased"] {
            self.session
                .call(
                    "Input.dispatchMouseEvent",
                    serde_json::json!({
                        "type": event,
                        "x": x,
                        "y": y,
                        "button": "left",
                        "clickCount": 1,
                    }),
                )
                .await?;
        }

        self.settle(&url_before).await;
        Ok(())
    }

    /// Wait for the page to stop moving after an interaction.
    ///
    /// Reading the URL straight after a click reports the *old* page: the
    /// navigation it triggered has not happened yet. This watches for the URL
    /// to change — covering both real navigations and SPA `pushState` routing
    /// — and then waits for the document to finish loading.
    ///
    /// Best-effort by design: a click that changes nothing (a checkbox, a
    /// menu toggle) simply costs the settle window and reports the same URL,
    /// so this never fails a click that otherwise worked.
    async fn settle(&self, url_before: &str) {
        const NAV_WINDOW: Duration = Duration::from_millis(1200);
        const READY_TIMEOUT: Duration = Duration::from_secs(10);

        let deadline = std::time::Instant::now() + NAV_WINDOW;
        let mut navigated = false;
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            match self.eval_string("location.href").await {
                Ok(now) if now != url_before => {
                    navigated = true;
                    break;
                }
                // The old execution context is torn down mid-navigation, so
                // an error here means a navigation is in flight.
                Err(_) => {
                    navigated = true;
                    break;
                }
                Ok(_) => {}
            }
        }

        if navigated {
            let _ = self.wait_for_ready(READY_TIMEOUT).await;
        }
    }

    /// Focus the element and type `text` into it.
    ///
    /// `Input.insertText` delivers the whole string as a composition event,
    /// which is what a paste or an IME does. Pages that listen for individual
    /// key events (some autocompletes) may need `press_enter` to commit.
    pub async fn type_text(
        &self,
        backend_node_id: i64,
        text: &str,
        press_enter: bool,
    ) -> Result<()> {
        self.session
            .call(
                "DOM.focus",
                serde_json::json!({ "backendNodeId": backend_node_id }),
            )
            .await?;

        self.session
            .call("Input.insertText", serde_json::json!({ "text": text }))
            .await?;

        let url_before = if press_enter {
            self.eval_string("location.href").await.unwrap_or_default()
        } else {
            String::new()
        };

        if press_enter {
            for event in ["keyDown", "keyUp"] {
                self.session
                    .call(
                        "Input.dispatchKeyEvent",
                        serde_json::json!({
                            "type": event,
                            "key": "Enter",
                            "code": "Enter",
                            "windowsVirtualKeyCode": 13,
                            "nativeVirtualKeyCode": 13,
                        }),
                    )
                    .await?;
            }
            // Enter usually submits, which navigates.
            self.settle(&url_before).await;
        }
        Ok(())
    }

    /// Capture the page as an image, returned base64-encoded.
    ///
    /// `full_page` captures beyond the viewport by asking Chrome for the full
    /// scrollable content size. JPEG keeps the payload small enough to sit in
    /// a model's context; PNG is crisper for fine UI text.
    pub async fn screenshot(&self, full_page: bool, jpeg_quality: Option<u8>) -> Result<(String, String)> {
        let mut params = serde_json::json!({
            "captureBeyondViewport": full_page,
        });
        let mime = match jpeg_quality {
            Some(q) => {
                params["format"] = "jpeg".into();
                params["quality"] = q.min(100).into();
                "image/jpeg"
            }
            None => {
                params["format"] = "png".into();
                "image/png"
            }
        };

        if full_page {
            // captureBeyondViewport alone still clips to the viewport unless
            // the clip rect covers the whole scrollable area.
            let metrics = self
                .session
                .call("Page.getLayoutMetrics", serde_json::json!({}))
                .await?;
            if let Some(content) = metrics.get("cssContentSize") {
                params["clip"] = serde_json::json!({
                    "x": 0,
                    "y": 0,
                    "width": content.get("width").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                    "height": content.get("height").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
                    "scale": 1,
                });
            }
        }

        let result = self.session.call("Page.captureScreenshot", params).await?;
        let data = result
            .get("data")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ChromeError::Protocol {
                method: "Page.captureScreenshot".to_owned(),
                detail: "reply carried no image data".to_owned(),
            })?;
        Ok((data.to_owned(), mime.to_owned()))
    }

    /// Viewport-relative centre of an element, from its box model.
    async fn element_center(&self, backend_node_id: i64) -> Result<(f64, f64)> {
        let result = self
            .session
            .call(
                "DOM.getBoxModel",
                serde_json::json!({ "backendNodeId": backend_node_id }),
            )
            .await
            .map_err(|e| match e {
                // CDP's own message here is "Could not compute box model",
                // which does not say what the caller got wrong.
                ChromeError::Command { .. } => ChromeError::Command {
                    method: "DOM.getBoxModel".to_owned(),
                    message: format!(
                        "element [ref={backend_node_id}] has no layout box — it is \
                         hidden, detached, or the page changed since it was read. \
                         Read the page again to get current refs."
                    ),
                },
                other => other,
            })?;

        // `content` is a quad: x1,y1, x2,y2, x3,y3, x4,y4 — clockwise from
        // the top-left corner.
        let quad = result
            .get("model")
            .and_then(|m| m.get("content"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ChromeError::Protocol {
                method: "DOM.getBoxModel".to_owned(),
                detail: "no content quad in box model".to_owned(),
            })?;

        if quad.len() < 8 {
            return Err(ChromeError::Protocol {
                method: "DOM.getBoxModel".to_owned(),
                detail: format!("content quad had {} points, expected 8", quad.len()),
            });
        }

        let n = |i: usize| quad[i].as_f64().unwrap_or(0.0);
        Ok(((n(0) + n(4)) / 2.0, (n(1) + n(5)) / 2.0))
    }

    /// Read the page as an indented accessibility outline.
    ///
    /// The accessibility tree is what a screen reader exposes: roles, names
    /// and values, with the presentational wrappers collapsed away. It is far
    /// smaller than the DOM and far more useful to a model than raw HTML, and
    /// the `backendDOMNodeId` on each node is the handle a future click/type
    /// tool will address.
    pub async fn read_accessibility_tree(&self, max_chars: usize) -> Result<String> {
        let result = self
            .session
            .call("Accessibility.getFullAXTree", serde_json::json!({}))
            .await?;

        let nodes = result
            .get("nodes")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ChromeError::Protocol {
                method: "Accessibility.getFullAXTree".to_owned(),
                detail: "no `nodes` array in reply".to_owned(),
            })?;

        Ok(format_ax_tree(nodes, max_chars))
    }
}

/// Render the flat AX node list as an indented outline.
///
/// CDP returns the tree flat, with parent→child links by node id, so this
/// walks from the root rather than relying on array order.
fn format_ax_tree(nodes: &[serde_json::Value], max_chars: usize) -> String {
    use std::collections::HashMap;

    let by_id: HashMap<&str, &serde_json::Value> = nodes
        .iter()
        .filter_map(|n| n.get("nodeId").and_then(serde_json::Value::as_str).map(|id| (id, n)))
        .collect();

    // The root is the node nothing else claims as a child.
    fn children_of(n: &serde_json::Value) -> Vec<&str> {
        n.get("childIds")
            .and_then(serde_json::Value::as_array)
            .map(|c| c.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default()
    }
    let claimed: std::collections::HashSet<&str> =
        nodes.iter().flat_map(|n| children_of(n)).collect();
    let roots: Vec<&str> = by_id
        .keys()
        .filter(|id| !claimed.contains(*id))
        .copied()
        .collect();

    let mut out = String::new();
    let mut truncated = false;
    let mut stack: Vec<(&str, usize)> = roots.iter().rev().map(|id| (*id, 0usize)).collect();

    while let Some((id, depth)) = stack.pop() {
        let Some(node) = by_id.get(id) else { continue };

        // Nodes Chrome marks ignored are presentational wrappers; their
        // children still matter, so descend without emitting a line.
        let ignored = node
            .get("ignored")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        if !ignored && let Some(line) = format_ax_node(node, depth) {
            if out.len() + line.len() > max_chars {
                truncated = true;
                break;
            }
            out.push_str(&line);
            out.push('\n');
        }

        let next_depth = if ignored { depth } else { depth + 1 };
        for child in children_of(node).into_iter().rev() {
            stack.push((child, next_depth));
        }
    }

    if truncated {
        out.push_str("\n… tree truncated; narrow the read or raise max_chars\n");
    }
    out
}

/// One AX node as `  role "name" [ref=N]`, or `None` when it carries nothing.
fn format_ax_node(node: &serde_json::Value, depth: usize) -> Option<String> {
    let role = node
        .get("role")
        .and_then(|r| r.get("value"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let name = node
        .get("name")
        .and_then(|n| n.get("value"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");

    // A node with neither role nor name tells the model nothing.
    if role.is_empty() && name.is_empty() {
        return None;
    }

    let mut line = format!("{}{role}", "  ".repeat(depth.min(20)));
    if !name.is_empty() {
        // Names can carry newlines from the page; keep every node one line.
        let flat = name.replace('\n', " ");
        let clipped: String = flat.chars().take(160).collect();
        line.push_str(&format!(" \"{clipped}\""));
    }
    if let Some(value) = node
        .get("value")
        .and_then(|v| v.get("value"))
        .and_then(serde_json::Value::as_str)
        && !value.is_empty()
    {
        let clipped: String = value.replace('\n', " ").chars().take(80).collect();
        line.push_str(&format!(" = \"{clipped}\""));
    }
    if let Some(backend_id) = node
        .get("backendDOMNodeId")
        .and_then(serde_json::Value::as_i64)
    {
        line.push_str(&format!(" [ref={backend_id}]"));
    }
    Some(line)
}


/// Schemes a browsing agent has no business loading.
///
/// `devtools://` pages run the DevTools frontend, which is privileged: it can
/// reach the debugging APIs of the very browser driving it. `chrome://` and
/// its aliases expose browser internals — `chrome://net-export`,
/// `chrome://settings` and friends are not pages, they are controls. Chrome
/// itself already refuses top-level `javascript:` navigation over CDP, which
/// is why that scheme is not listed here; it is blocked anyway.
///
/// `file://` is deliberately still allowed: previewing a locally built page
/// is a real thing a coding agent does. It does mean navigate-plus-read can
/// read any file the user can, which is no more than the agent's own file
/// tools grant — but it is worth knowing when the Chrome tools are handed to
/// a session whose file access is otherwise restricted.
const BLOCKED_SCHEMES: &[&str] = &[
    "devtools:",
    "chrome:",
    "chrome-untrusted:",
    "chrome-extension:",
    "chrome-search:",
    "view-source:",
];

/// Reject a URL whose scheme grants more than page browsing.
fn validate_scheme(url: &str) -> Result<()> {
    let lowered = url.trim().to_ascii_lowercase();
    if let Some(blocked) = BLOCKED_SCHEMES
        .iter()
        .find(|s| lowered.starts_with(*s))
    {
        return Err(ChromeError::BlockedScheme {
            scheme: blocked.trim_end_matches(':').to_owned(),
            url: url.to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ax(id: &str, role: &str, name: &str, children: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "nodeId": id,
            "role": { "value": role },
            "name": { "value": name },
            "childIds": children,
            "backendDOMNodeId": 42,
        })
    }

    #[test]
    fn tree_is_rendered_parent_before_child_with_indentation() {
        let nodes = vec![
            ax("1", "RootWebArea", "Example", &["2"]),
            ax("2", "button", "Sign in", &[]),
        ];
        let out = format_ax_tree(&nodes, 10_000);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("RootWebArea"), "got {:?}", lines);
        assert!(lines[1].starts_with("  button"), "got {:?}", lines);
        assert!(lines[1].contains("[ref=42]"));
    }

    #[test]
    fn ignored_nodes_are_skipped_but_their_children_survive() {
        let mut wrapper = ax("1", "generic", "", &["2"]);
        wrapper["ignored"] = serde_json::Value::Bool(true);
        let nodes = vec![wrapper, ax("2", "link", "Docs", &[])];
        let out = format_ax_tree(&nodes, 10_000);
        assert!(!out.contains("generic"));
        // Depth did not advance past the skipped wrapper.
        assert!(out.starts_with("link"), "got {out:?}");
    }

    #[test]
    fn privileged_schemes_are_refused() {
        for url in [
            "devtools://devtools/bundled/devtools_app.html",
            "chrome://version",
            "CHROME://settings",
            "  chrome-extension://abc/page.html",
            "view-source:https://example.com",
        ] {
            assert!(
                validate_scheme(url).is_err(),
                "{url} should be refused"
            );
        }
    }

    #[test]
    fn ordinary_browsing_schemes_are_allowed() {
        for url in [
            "https://example.com",
            "http://localhost:3000/app",
            "file:///tmp/build/index.html",
            "data:text/html,<h1>hi</h1>",
        ] {
            assert!(validate_scheme(url).is_ok(), "{url} should be allowed");
        }
    }

    #[test]
    fn nameless_roleless_nodes_emit_nothing() {
        let node = serde_json::json!({ "nodeId": "1", "childIds": [] });
        assert!(format_ax_node(&node, 0).is_none());
    }

    #[test]
    fn newlines_in_names_never_break_the_one_line_per_node_shape() {
        let node = ax("1", "text", "first\nsecond", &[]);
        let line = format_ax_node(&node, 0).unwrap();
        assert!(!line.contains('\n'));
        assert!(line.contains("first second"));
    }

    #[test]
    fn truncation_is_announced_rather_than_silent() {
        let nodes: Vec<_> = (0..500)
            .map(|i| ax(&i.to_string(), "text", "a long-ish accessible name here", &[]))
            .collect();
        let out = format_ax_tree(&nodes, 200);
        assert!(out.contains("truncated"), "got {out:?}");
    }
}
