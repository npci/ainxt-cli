//! Error type for the Chrome DevTools Protocol client.

/// Every failure mode of launching, connecting to, or driving Chrome.
#[derive(Debug, thiserror::Error)]
pub enum ChromeError {
    /// No Chrome binary was found at any known install location.
    #[error(
        "Chrome not found. Looked in the standard install locations; \
         set AINXT_CHROME_BINARY to its path."
    )]
    BinaryNotFound,

    /// The Chrome process failed to start.
    #[error("failed to launch Chrome: {0}")]
    Launch(#[source] std::io::Error),

    /// Chrome started but never opened its DevTools port within the deadline.
    #[error(
        "Chrome did not expose a DevTools endpoint on port {port} within {secs}s. \
         Chrome 136+ refuses --remote-debugging-port when the profile is the \
         default user-data-dir; ainxt uses a dedicated copy to avoid this."
    )]
    DevToolsTimeout { port: u16, secs: u64 },

    /// The WebSocket transport to Chrome failed.
    #[error("DevTools websocket error: {0}")]
    WebSocket(#[source] Box<tokio_tungstenite::tungstenite::Error>),

    /// The connection closed while a command was still in flight.
    #[error("DevTools connection closed while awaiting a response")]
    ConnectionClosed,

    /// A URL used a scheme that grants more than page browsing.
    #[error(
        "refusing to open `{url}`: the `{scheme}:` scheme reaches browser \
         internals rather than a web page"
    )]
    BlockedScheme { scheme: String, url: String },

    /// Chrome returned an error for a CDP command.
    #[error("CDP command `{method}` failed: {message}")]
    Command { method: String, message: String },

    /// A CDP response did not match the shape this client expects.
    #[error("unexpected CDP response for `{method}`: {detail}")]
    Protocol { method: String, detail: String },

    /// Seeding the dedicated profile from the real one failed.
    #[error("could not seed Chrome profile from {source_dir}: {detail}")]
    ProfileSeed { source_dir: String, detail: String },

    /// Talking to the DevTools HTTP endpoint failed.
    #[error("DevTools HTTP endpoint error: {0}")]
    Http(#[source] reqwest::Error),

    /// An I/O failure outside of process launch.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<tokio_tungstenite::tungstenite::Error> for ChromeError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(Box::new(e))
    }
}

impl From<reqwest::Error> for ChromeError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

/// Result alias used throughout this crate.
pub type Result<T> = std::result::Result<T, ChromeError>;
