//! The opencode agent backend. Talks to a worker's `opencode serve` HTTP API
//! (OpenAPI 3.1, basic-auth protected). One `OpencodeBackend` per worker.

use async_trait::async_trait;
use eventsource_stream::Eventsource as _;
use futures::stream::{BoxStream, StreamExt};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde_json::json;

use super::{
    AgentBackend, AgentError, AgentInfo, ModelInfo, Session, SessionActivity, SessionMessage,
};

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

    async fn list_agents(&self) -> Result<Vec<AgentInfo>, AgentError> {
        let resp = Self::check(self.authed(Method::GET, "/agent").send().await?).await?;
        let v: serde_json::Value = resp.json().await?;
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                let name = a.get("name")?.as_str()?.to_string();
                let model = a
                    .get("model")
                    .and_then(|m| m.get("modelID"))
                    .and_then(|m| m.as_str())
                    .map(String::from);
                Some(AgentInfo { name, model })
            })
            .collect())
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, AgentError> {
        let resp = Self::check(self.authed(Method::GET, "/provider").send().await?).await?;
        let v: serde_json::Value = resp.json().await?;
        let providers = v
            .get("all")
            .and_then(|x| x.as_array())
            .or_else(|| v.get("providers").and_then(|x| x.as_array()))
            .into_iter()
            .flatten();
        let mut out = Vec::new();
        for p in providers {
            if let Some(models) = p.get("models").and_then(|m| m.as_object()) {
                for (id, info) in models {
                    let context = info
                        .get("limit")
                        .and_then(|l| l.get("context"))
                        .and_then(|c| c.as_u64());
                    let reasoning = info
                        .get("reasoning")
                        .and_then(|r| r.as_bool())
                        .unwrap_or(false);
                    let name = info
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or(id)
                        .to_string();
                    out.push(ModelInfo {
                        id: id.clone(),
                        name,
                        context,
                        reasoning,
                    });
                }
            }
        }
        Ok(out)
    }

    async fn default_model(&self) -> Result<Option<String>, AgentError> {
        let resp = Self::check(self.authed(Method::GET, "/provider").send().await?).await?;
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("default")
            .and_then(|d| d.as_object())
            .and_then(|m| m.values().find_map(|x| x.as_str()))
            .map(String::from))
    }

    async fn session_status(&self, session_id: &str) -> Result<SessionActivity, AgentError> {
        let resp = Self::check(self.authed(Method::GET, "/session/status").send().await?).await?;
        let v: serde_json::Value = resp.json().await?;
        let status = v
            .get(session_id)
            .and_then(|s| s.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("idle");
        Ok(match status {
            "busy" | "retry" => SessionActivity::Busy,
            _ => SessionActivity::Idle,
        })
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
        let url = self.endpoint(&format!("/session/{session_id}/prompt_async"));
        let resp = self
            .client
            .request(Method::POST, url)
            .basic_auth(self.username.clone(), Some(&self.password))
            .json(&body)
            .send()
            .await
            .map_err(AgentError::Io)?;
        let status = resp.status();

        // Older opencode versions lack `prompt_async`. Fall back to the
        // standard `/message` endpoint, sent in the background so this request
        // still returns immediately (the UI observes progress via poll/SSE).
        if status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED {
            let client = self.client.clone();
            let username = self.username.clone();
            let password = self.password.clone();
            let url = self.endpoint(&format!("/session/{session_id}/message"));
            tokio::spawn(async move {
                let _ = client
                    .request(Method::POST, url)
                    .basic_auth(username, Some(&password))
                    .json(&body)
                    .send()
                    .await;
            });
        }
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
