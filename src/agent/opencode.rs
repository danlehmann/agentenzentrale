//! The opencode agent backend. Talks to a worker's `opencode serve` HTTP API
//! (OpenAPI 3.1, basic-auth protected). One `OpencodeBackend` per worker.

use async_trait::async_trait;
use eventsource_stream::Eventsource as _;
use futures::stream::{BoxStream, StreamExt};
use reqwest::{Method, RequestBuilder};
use serde_json::json;

use super::{AgentBackend, AgentError, Session, SessionMessage};

pub struct OpencodeBackend {
    client: reqwest::Client,
    base: String,
    username: String,
    password: String,
}

impl OpencodeBackend {
    pub fn new(url: String, username: String, password: String) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("agentenzentrale/0.1")
            .timeout(std::time::Duration::from_secs(300))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        let base = url.trim_end_matches('/').to_string();
        OpencodeBackend {
            client,
            base,
            username,
            password,
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn authed(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, self.endpoint(path))
            .basic_auth(self.username.clone(), Some(&self.password))
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response, AgentError> {
        if !resp.status().is_success() {
            return Err(AgentError::Status(resp.status()));
        }
        Ok(resp)
    }

    fn parse_session(v: &serde_json::Value) -> Session {
        Session {
            id: v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            title: v.get("title").and_then(|x| x.as_str()).map(String::from),
            raw: v.clone(),
        }
    }

    fn parse_message(v: &serde_json::Value) -> SessionMessage {
        SessionMessage {
            info: v.get("info").cloned().unwrap_or_else(|| v.clone()),
            parts: v
                .get("parts")
                .and_then(|p| p.as_array().cloned())
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl AgentBackend for OpencodeBackend {
    async fn list_sessions(&self) -> Result<Vec<Session>, AgentError> {
        let resp = Self::check(self.authed(Method::GET, "/session").send().await?).await?;
        let value: serde_json::Value = resp.json().await?;
        let arr = value
            .as_array()
            .ok_or_else(|| AgentError::Decode("expected session array".into()))?;
        Ok(arr.iter().map(Self::parse_session).collect())
    }

    async fn create_session(&self, title: Option<&str>) -> Result<Session, AgentError> {
        let body = json!({ "title": title });
        let resp = Self::check(
            self.authed(Method::POST, "/session")
                .json(&body)
                .send()
                .await?,
        )
        .await?;
        let value: serde_json::Value = resp.json().await?;
        Ok(Self::parse_session(&value))
    }

    async fn get_session(&self, id: &str) -> Result<Session, AgentError> {
        let resp = Self::check(
            self.authed(Method::GET, &format!("/session/{id}"))
                .send()
                .await?,
        )
        .await?;
        let value: serde_json::Value = resp.json().await?;
        Ok(Self::parse_session(&value))
    }

    async fn list_messages(
        &self,
        session_id: &str,
        limit: Option<u32>,
    ) -> Result<Vec<SessionMessage>, AgentError> {
        let mut path = format!("/session/{session_id}/message");
        if let Some(l) = limit {
            path.push_str(&format!("?limit={l}"));
        }
        let resp = Self::check(self.authed(Method::GET, &path).send().await?).await?;
        let value: serde_json::Value = resp.json().await?;
        let arr = value
            .as_array()
            .ok_or_else(|| AgentError::Decode("expected message array".into()))?;
        Ok(arr.iter().map(Self::parse_message).collect())
    }

    async fn send_text_async(
        &self,
        session_id: &str,
        text: &str,
        agent: Option<&str>,
        model: Option<&str>,
    ) -> Result<(), AgentError> {
        let mut body = json!({ "parts": [{ "type": "text", "text": text }] });
        if let Some(obj) = body.as_object_mut() {
            if let Some(agent) = agent {
                obj.insert("agent".into(), json!(agent));
            }
            if let Some(model) = model {
                obj.insert("model".into(), json!(model));
            }
        }
        self.authed(Method::POST, &format!("/session/{session_id}/prompt_async"))
            .json(&body)
            .send()
            .await
            .map_err(AgentError::Io)?;
        Ok(())
    }

    async fn abort(&self, session_id: &str) -> Result<(), AgentError> {
        Self::check(
            self.authed(Method::POST, &format!("/session/{session_id}/abort"))
                .send()
                .await?,
        )
        .await?;
        Ok(())
    }

    async fn events(&self) -> Result<BoxStream<'static, Result<String, AgentError>>, AgentError> {
        let resp = Self::check(self.authed(Method::GET, "/event").send().await?).await?;
        let stream = resp.bytes_stream().eventsource().map(|ev| match ev {
            Ok(ev) => Ok(ev.data),
            Err(e) => Err(AgentError::Decode(e.to_string())),
        });
        Ok(Box::pin(stream))
    }
}
