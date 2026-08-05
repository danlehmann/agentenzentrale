//! Agent abstraction. `AgentBackend` is the seam that lets Q talk to any
//! coding agent instead of being coupled to opencode. `opencode` is the first
//! implementation; future agents implement the same trait and register a
//! `Worker.kind`.

pub mod opencode;
pub mod render;

use async_trait::async_trait;
use futures::stream::BoxStream;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("connection error: {0}")]
    Io(#[from] reqwest::Error),
    #[error("worker returned status {0}")]
    Status(reqwest::StatusCode),
    #[error("worker response was invalid: {0}")]
    Decode(String),
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub title: Option<String>,
    pub raw: Value,
}

impl Session {
    pub fn title_display(&self) -> String {
        self.title
            .clone()
            .unwrap_or_else(|| match self.raw.get("title") {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v
                    .get("text")
                    .and_then(|t| t.as_str().map(String::from))
                    .unwrap_or_else(|| "(untitled)".into()),
                None => "(untitled)".into(),
            })
    }
}

/// A single message in a session: an envelope plus its parts.
#[derive(Debug, Clone)]
pub struct SessionMessage {
    pub info: Value,
    pub parts: Vec<Value>,
}

/// Concatenate the user-visible text of all `text`-typed parts.
pub fn message_text(msg: &SessionMessage) -> String {
    msg.parts
        .iter()
        .filter_map(|p| {
            let t = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if t == "text" {
                p.get("text").and_then(|v| v.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The role of a message, derived from its info envelope.
pub fn message_role(msg: &SessionMessage) -> String {
    msg.info
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("assistant")
        .to_string()
}

#[async_trait]
pub trait AgentBackend: Send + Sync {
    async fn list_sessions(&self) -> Result<Vec<Session>, AgentError>;
    async fn create_session(&self, title: Option<&str>) -> Result<Session, AgentError>;
    async fn get_session(&self, id: &str) -> Result<Session, AgentError>;
    async fn list_messages(
        &self,
        session_id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<SessionMessage>, AgentError>;

    /// Send a plain-text user message without waiting for the reply. The agent
    /// processes it in the background; the UI observes progress via polling or
    /// events. This keeps the web request fast instead of blocking for a turn.
    async fn send_text_async(
        &self,
        session_id: &str,
        text: &str,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> Result<(), AgentError>;

    /// Abort the currently-running turn in a session.
    async fn abort(&self, session_id: &str) -> Result<(), AgentError>;

    /// Open a stream of session events (SSE). Yields raw event payloads.
    async fn events(&self) -> Result<BoxStream<'static, Result<String, AgentError>>, AgentError>;
}
