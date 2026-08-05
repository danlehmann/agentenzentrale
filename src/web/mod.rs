//! HTTP routes, handlers, and Askama template structs.

use std::convert::Infallible;
use std::sync::Arc;

use askama::Template;
use axum::extract::{ConnectInfo, Form, Path, State};
use axum::http::header::{HeaderValue, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{Html, IntoResponse, Redirect, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde::Deserialize;

use crate::agent::opencode::OpencodeBackend;
use crate::agent::render::render_message;
use crate::agent::{message_role, AgentBackend, SessionMessage};
use crate::auth::{check_csrf, AdminUser, CurrentUser, LoginLimiter};
use crate::config::Cli;
use crate::crypto::SecretKey;
use crate::db::{Db, Worker};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
    pub key: Arc<SecretKey>,
    pub config: Arc<Cli>,
    pub limiter: Arc<LoginLimiter>,
}

impl AppState {
    pub fn backend_for(&self, w: &Worker) -> anyhow::Result<Box<dyn AgentBackend>> {
        let password = self.key.decrypt(&w.password_enc)?;
        match w.kind.as_str() {
            "opencode" => Ok(Box::new(OpencodeBackend::new(
                w.url.clone(),
                w.username.clone(),
                password,
            ))),
            other => anyhow::bail!("unsupported agent kind: {other}"),
        }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(dashboard))
        .route("/login", get(login_page).post(login_submit))
        .route("/setup", get(setup_page).post(setup_submit))
        .route("/logout", post(logout))
        .route("/workers/new", get(workers_new_page))
        .route("/workers", post(workers_submit))
        .route(
            "/workers/:id/edit",
            get(worker_edit_page).post(worker_update),
        )
        .route("/workers/:id/delete", post(workers_delete))
        .route("/workers/:id/test", post(workers_test))
        .route("/invites", get(invites_page).post(invites_create))
        .route("/invite/:token", get(invite_page).post(invite_claim))
        .route("/w/:id", get(worker_sessions))
        .route("/w/:id/sessions", post(worker_new_session))
        .route("/w/:id/s/:sid", get(session_page))
        .route("/w/:id/s/:sid/thread", get(session_thread))
        .route("/w/:id/s/:sid/message", post(session_message))
        .route("/w/:id/s/:sid/abort", post(session_abort))
        .route("/w/:id/events", get(worker_events))
        .route("/w/:id/tools", get(worker_tools))
        .route("/w/:id/s/:sid/status", get(session_status))
        .with_state(state)
}

type HandlerResult = Result<Response, StatusCode>;

fn server_error(e: impl std::fmt::Display) -> StatusCode {
    tracing::error!("request failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

fn tpl<T: Template>(t: T) -> HandlerResult {
    match t.render() {
        Ok(s) => Ok(Html(s).into_response()),
        Err(e) => {
            tracing::error!("template rendering failed: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// A Redirect with an extra Set-Cookie header (used for login/logout).
fn redirect_with_cookie(path: &str, cookie: &str) -> HandlerResult {
    let mut resp = Redirect::to(path).into_response();
    if let Ok(v) = HeaderValue::from_str(cookie) {
        resp.headers_mut().insert(SET_COOKIE, v);
    }
    Ok(resp)
}

// ---------- auth pages ----------

async fn dashboard(State(st): State<AppState>, user: CurrentUser) -> HandlerResult {
    let workers = st.db.list_workers().map_err(server_error)?;
    let views: Vec<WorkerView> = workers
        .iter()
        .map(|w| WorkerView {
            id: w.id.clone(),
            name: w.name.clone(),
            kind: w.kind.clone(),
            url: w.url.clone(),
        })
        .collect();
    tpl(IndexTemplate {
        is_admin: user.user.is_admin,
        username: user.user.username.clone(),
        workers: views,
    })
}

async fn login_page(State(st): State<AppState>) -> HandlerResult {
    let count = st.db.count_users().map_err(server_error)?;
    if count == 0 {
        return Ok(Redirect::to("/setup").into_response());
    }
    tpl(LoginTemplate { error: None })
}

async fn login_submit(
    State(st): State<AppState>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    Form(form): Form<LoginForm>,
) -> HandlerResult {
    let ip = addr.ip().to_string();
    if st.limiter.check(&ip).is_err() {
        return tpl(LoginTemplate {
            error: Some("Too many attempts. Try again in a few minutes.".into()),
        });
    }
    let Some(user) = st
        .db
        .user_by_username(&form.username)
        .map_err(server_error)?
    else {
        st.limiter.record_failure(&ip);
        return tpl(LoginTemplate {
            error: Some("Invalid username or password.".into()),
        });
    };
    if !crate::auth::verify_password(&form.password, &user.password_hash) {
        st.limiter.record_failure(&ip);
        return tpl(LoginTemplate {
            error: Some("Invalid username or password.".into()),
        });
    }
    st.limiter.record_success(&ip);
    let (token, _csrf) = st.db.create_session(&user.id).map_err(server_error)?;
    let secure = if st.config.tls { "; Secure" } else { "" };
    let cookie =
        format!("q_session={token}; HttpOnly{secure}; SameSite=Lax; Path=/; Max-Age=86400");
    redirect_with_cookie("/", &cookie)
}

async fn setup_page(State(st): State<AppState>) -> HandlerResult {
    let count = st.db.count_users().map_err(server_error)?;
    if count > 0 {
        return Ok(Redirect::to("/").into_response());
    }
    tpl(SetupTemplate { error: None })
}

async fn setup_submit(State(st): State<AppState>, Form(form): Form<SetupForm>) -> HandlerResult {
    let count = st.db.count_users().map_err(server_error)?;
    if count > 0 {
        return Ok(Redirect::to("/").into_response());
    }
    if form.username.trim().is_empty() || form.password.is_empty() {
        return tpl(SetupTemplate {
            error: Some("Username and password are required.".into()),
        });
    }
    if form.password != form.password2 {
        return tpl(SetupTemplate {
            error: Some("Passwords do not match.".into()),
        });
    }
    let hash = crate::auth::hash_password(&form.password).map_err(server_error)?;
    st.db
        .create_user(form.username.trim(), &hash, true)
        .map_err(server_error)?;
    Ok(Redirect::to("/login").into_response())
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> HandlerResult {
    if let Some(tok) = crate::auth::token_from_headers(&headers) {
        let _ = st.db.delete_session(&tok);
    }
    let secure = if st.config.tls { "; Secure" } else { "" };
    let cookie = format!("q_session=; HttpOnly{secure}; SameSite=Lax; Path=/; Max-Age=0");
    redirect_with_cookie("/login", &cookie)
}

// ---------- worker management (admin) ----------

async fn workers_new_page(_st: State<AppState>, admin: AdminUser) -> HandlerResult {
    let cur = admin.0;
    tpl(WorkersNewTemplate {
        is_admin: cur.user.is_admin,
        username: cur.user.username.clone(),
        csrf: cur.csrf.clone(),
        error: None,
    })
}

async fn workers_submit(
    State(st): State<AppState>,
    admin: AdminUser,
    Form(form): Form<WorkerForm>,
) -> HandlerResult {
    let cur = admin.0;
    let show_error = || {
        tpl(WorkersNewTemplate {
            is_admin: cur.user.is_admin,
            username: cur.user.username.clone(),
            csrf: cur.csrf.clone(),
            error: Some("Fill in name, URL and password.".into()),
        })
    };
    if !check_csrf(&cur, &form._csrf)
        || form.name.trim().is_empty()
        || form.url.trim().is_empty()
        || form.password.is_empty()
    {
        return show_error();
    }
    let kind = if form.kind.is_empty() {
        "opencode".to_string()
    } else {
        form.kind.clone()
    };
    let username = if form.username.trim().is_empty() {
        "opencode".to_string()
    } else {
        form.username.trim().to_string()
    };
    st.db
        .create_worker(
            form.name.trim(),
            &kind,
            form.url.trim(),
            &username,
            &form.password,
            &st.key,
        )
        .map_err(server_error)?;
    Ok(Redirect::to("/").into_response())
}

async fn workers_delete(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> HandlerResult {
    if !check_csrf(&admin.0, &form._csrf) {
        return Ok((StatusCode::BAD_REQUEST, "bad csrf").into_response());
    }
    st.db.delete_worker(&id).map_err(server_error)?;
    Ok(Redirect::to("/").into_response())
}

async fn worker_edit_page(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<String>,
) -> HandlerResult {
    let cur = admin.0;
    let Some(w) = st.db.worker_by_id(&id).map_err(server_error)? else {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    };
    tpl(WorkersEditTemplate {
        is_admin: cur.user.is_admin,
        username: cur.user.username.clone(),
        csrf: cur.csrf.clone(),
        id: w.id,
        name: w.name,
        kind: w.kind,
        url: w.url,
        username_field: w.username,
        error: None,
    })
}

async fn worker_update(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<String>,
    Form(form): Form<WorkerEditForm>,
) -> HandlerResult {
    let cur = admin.0;
    let reload = |error: &str| {
        tpl(WorkersEditTemplate {
            is_admin: cur.user.is_admin,
            username: cur.user.username.clone(),
            csrf: cur.csrf.clone(),
            id: id.clone(),
            name: form.name.clone(),
            kind: form.kind.clone(),
            url: form.url.clone(),
            username_field: form.username.clone(),
            error: Some(error.to_string()),
        })
    };
    if !check_csrf(&cur, &form._csrf) {
        return reload("Invalid CSRF token.");
    }
    if form.name.trim().is_empty() || form.url.trim().is_empty() {
        return reload("Name and URL are required.");
    }
    if st.db.worker_by_id(&id).map_err(server_error)?.is_none() {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    }
    let kind = if form.kind.is_empty() {
        "opencode".to_string()
    } else {
        form.kind.clone()
    };
    let username = if form.username.trim().is_empty() {
        "opencode".to_string()
    } else {
        form.username.trim().to_string()
    };
    st.db
        .update_worker(
            &id,
            form.name.trim(),
            &kind,
            form.url.trim(),
            &username,
            &form.password,
            &st.key,
        )
        .map_err(server_error)?;
    Ok(Redirect::to("/").into_response())
}

async fn workers_test(
    State(st): State<AppState>,
    admin: AdminUser,
    Path(id): Path<String>,
) -> HandlerResult {
    let _ = admin;
    let Some(w) = st.db.worker_by_id(&id).map_err(server_error)? else {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let body = match backend.list_sessions().await {
        Ok(ss) => format!("ok: reachable, {} session(s)", ss.len()),
        Err(e) => format!("error: {e}"),
    };
    Ok(Html(body).into_response())
}

// ---------- invites (admin) ----------

fn base_url(st: &AppState) -> String {
    st.config
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:"))
        .trim_end_matches('/')
        .to_string()
}

async fn invites_page(State(st): State<AppState>, admin: AdminUser) -> HandlerResult {
    let cur = admin.0;
    let pending = st.db.list_invites().map_err(server_error)?;
    let pending: Vec<PendingInvite> = pending
        .iter()
        .map(|(_by, at)| PendingInvite {
            created_at: at.clone(),
        })
        .collect();
    tpl(InvitesTemplate {
        is_admin: cur.user.is_admin,
        username: cur.user.username.clone(),
        csrf: cur.csrf.clone(),
        new_link: None,
        pending,
    })
}

async fn invites_create(
    State(st): State<AppState>,
    admin: AdminUser,
    Form(form): Form<CsrfForm>,
) -> HandlerResult {
    let cur = admin.0;
    if !check_csrf(&cur, &form._csrf) {
        return Ok((StatusCode::BAD_REQUEST, "bad csrf").into_response());
    }
    let token = st.db.create_invite(&cur.user.id).map_err(server_error)?;
    let link = format!("{}/invite/{}", base_url(&st), token);
    let pending = st.db.list_invites().map_err(server_error)?;
    let pending: Vec<PendingInvite> = pending
        .iter()
        .map(|(_by, at)| PendingInvite {
            created_at: at.clone(),
        })
        .collect();
    tpl(InvitesTemplate {
        is_admin: cur.user.is_admin,
        username: cur.user.username.clone(),
        csrf: cur.csrf.clone(),
        new_link: Some(link),
        pending,
    })
}

async fn invite_page(State(st): State<AppState>, Path(token): Path<String>) -> HandlerResult {
    if !st.db.invite_valid(&token).map_err(server_error)? {
        return Ok((StatusCode::NOT_FOUND, "invite invalid or already used").into_response());
    }
    tpl(InviteClaimTemplate { token, error: None })
}

async fn invite_claim(
    State(st): State<AppState>,
    Path(token): Path<String>,
    Form(form): Form<SetupForm>,
) -> HandlerResult {
    if !st.db.invite_valid(&token).map_err(server_error)? {
        return Ok((StatusCode::NOT_FOUND, "invite invalid or already used").into_response());
    }
    if form.username.trim().is_empty() || form.password.is_empty() {
        return tpl(InviteClaimTemplate {
            token: token.clone(),
            error: Some("Username and password are required.".into()),
        });
    }
    if form.password != form.password2 {
        return tpl(InviteClaimTemplate {
            token: token.clone(),
            error: Some("Passwords do not match.".into()),
        });
    }
    let hash = crate::auth::hash_password(&form.password).map_err(server_error)?;
    st.db
        .create_user(form.username.trim(), &hash, false)
        .map_err(server_error)?;
    st.db.consume_invite(&token).map_err(server_error)?;
    Ok(Redirect::to("/login").into_response())
}

// ---------- worker sessions ----------

async fn worker_sessions(
    State(st): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
) -> HandlerResult {
    let Some(w) = st.db.worker_by_id(&id).map_err(server_error)? else {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    };
    let wv = WorkerView {
        id: w.id.clone(),
        name: w.name.clone(),
        kind: w.kind.clone(),
        url: w.url.clone(),
    };
    let worker_ref = WorkerView {
        id: w.id.clone(),
        name: w.name.clone(),
        kind: w.kind.clone(),
        url: w.url.clone(),
    };
    let sessions = match st.backend_for(&w) {
        Ok(b) => match b.list_sessions().await {
            Ok(s) => s,
            Err(e) => {
                return tpl(WorkerSessionsTemplate {
                    is_admin: user.user.is_admin,
                    username: user.user.username.clone(),
                    csrf: user.csrf.clone(),
                    worker: worker_ref,
                    sessions: vec![],
                    error: Some(format!("Could not reach worker: {e}")),
                })
            }
        },
        Err(e) => {
            return tpl(WorkerSessionsTemplate {
                is_admin: user.user.is_admin,
                username: user.user.username.clone(),
                csrf: user.csrf.clone(),
                worker: worker_ref,
                sessions: vec![],
                error: Some(e.to_string()),
            })
        }
    };
    let views: Vec<SessionView> = sessions
        .iter()
        .map(|s| SessionView {
            id: s.id.clone(),
            title: s.title_display(),
        })
        .collect();
    tpl(WorkerSessionsTemplate {
        is_admin: user.user.is_admin,
        username: user.user.username.clone(),
        csrf: user.csrf.clone(),
        worker: wv,
        sessions: views,
        error: None,
    })
}

async fn worker_new_session(
    State(st): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    Form(form): Form<SessionForm>,
) -> HandlerResult {
    if !check_csrf(&user, &form._csrf) {
        return Ok((StatusCode::BAD_REQUEST, "bad csrf").into_response());
    }
    let Some(w) = st.db.worker_by_id(&id).map_err(server_error)? else {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let s = backend
        .create_session(Some(form.title.trim()))
        .await
        .map_err(server_error)?;
    Ok(Redirect::to(&format!("/w/{id}/s/{}", s.id)).into_response())
}

// ---------- session chat ----------

async fn session_page(
    State(st): State<AppState>,
    user: CurrentUser,
    Path((wid, sid)): Path<(String, String)>,
) -> HandlerResult {
    let Some(w) = st.db.worker_by_id(&wid).map_err(server_error)? else {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let session = backend.get_session(&sid).await.map_err(server_error)?;
    let title = session.title_display();
    let messages = backend
        .list_messages(&sid, Some(200))
        .await
        .map_err(server_error)?;
    let thread = SessionThreadTemplate {
        messages: build_thread(&messages),
        error: None,
    }
    .render()
    .map_err(server_error)?;
    tpl(SessionTemplate {
        csrf: user.csrf.clone(),
        worker: WorkerView {
            id: w.id.clone(),
            name: w.name.clone(),
            kind: w.kind.clone(),
            url: w.url.clone(),
        },
        session_id: sid,
        session_title: title,
        thread,
    })
}

async fn session_thread(
    State(st): State<AppState>,
    _user: CurrentUser,
    Path((wid, sid)): Path<(String, String)>,
) -> HandlerResult {
    let Some(w) = st.db.worker_by_id(&wid).map_err(server_error)? else {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let messages = backend
        .list_messages(&sid, Some(200))
        .await
        .map_err(server_error)?;
    tpl(SessionThreadTemplate {
        messages: build_thread(&messages),
        error: None,
    })
}

async fn session_message(
    State(st): State<AppState>,
    user: CurrentUser,
    Path((wid, sid)): Path<(String, String)>,
    Form(form): Form<MessageForm>,
) -> HandlerResult {
    if !check_csrf(&user, &form._csrf) {
        return Ok((StatusCode::BAD_REQUEST, "bad csrf").into_response());
    }
    let text = form.text.trim().to_string();
    if text.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "empty message").into_response());
    }
    let Some(w) = st.db.worker_by_id(&wid).map_err(server_error)? else {
        return Ok((StatusCode::NOT_FOUND, "worker not found").into_response());
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let agent = form.agent.filter(|s| !s.is_empty());
    let model = form.model.filter(|s| !s.is_empty());
    // Fire-and-forget: don't block the HTTP request for the whole agent turn.
    if let Err(e) = backend
        .send_text_async(&sid, &text, agent.as_deref(), model.as_deref())
        .await
    {
        let messages = backend
            .list_messages(&sid, Some(200))
            .await
            .unwrap_or_default();
        return tpl(SessionThreadTemplate {
            messages: build_thread(&messages),
            error: Some(format!("Failed to send: {e}")),
        });
    }
    let messages = backend
        .list_messages(&sid, Some(200))
        .await
        .map_err(server_error)?;
    tpl(SessionThreadTemplate {
        messages: build_thread(&messages),
        error: None,
    })
}

async fn session_abort(
    State(st): State<AppState>,
    user: CurrentUser,
    Path((wid, sid)): Path<(String, String)>,
    Form(form): Form<CsrfForm>,
) -> HandlerResult {
    if !check_csrf(&user, &form._csrf) {
        return Ok((StatusCode::BAD_REQUEST, "bad csrf").into_response());
    }
    if let Some(w) = st.db.worker_by_id(&wid).map_err(server_error)? {
        if let Ok(backend) = st.backend_for(&w) {
            let _ = backend.abort(&sid).await;
        }
    }
    Ok(Redirect::to(&format!("/w/{wid}/s/{sid}")).into_response())
}

async fn worker_events(
    State(st): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let Some(w) = st.db.worker_by_id(&id).map_err(server_error)? else {
        return Err(StatusCode::NOT_FOUND);
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let stream = backend.events().await.map_err(server_error)?;
    let s = stream.map(|ev| {
        let sse = match ev {
            Ok(data) => Event::default().event("agent").data(data),
            Err(e) => Event::default().event("error").data(e.to_string()),
        };
        Ok::<_, Infallible>(sse)
    });
    Ok(Sse::new(s).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15))))
}

async fn worker_tools(
    State(st): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let Some(w) = st.db.worker_by_id(&id).map_err(server_error)? else {
        return Err(StatusCode::NOT_FOUND);
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let agents = backend.list_agents().await.unwrap_or_default();
    let models = backend.list_models().await.unwrap_or_default();
    Ok(Json(serde_json::json!({ "agents": agents, "models": models })))
}

async fn session_status(
    State(st): State<AppState>,
    _user: CurrentUser,
    Path((wid, sid)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let Some(w) = st.db.worker_by_id(&wid).map_err(server_error)? else {
        return Err(StatusCode::NOT_FOUND);
    };
    let backend = st.backend_for(&w).map_err(server_error)?;
    let busy = matches!(
        backend.session_status(&sid).await,
        Ok(crate::agent::SessionActivity::Busy)
    );
    Ok(Json(serde_json::json!({ "busy": busy })))
}

fn build_thread(messages: &[SessionMessage]) -> Vec<MessageView> {
    messages
        .iter()
        .filter_map(|m| {
            let html = render_message(m);
            // Skip turns with nothing user-visible (reasoning-only, empty).
            if html.trim().is_empty() {
                return None;
            }
            Some(MessageView {
                role: message_role(m),
                html,
            })
        })
        .collect()
}

// ---------- forms ----------

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct SetupForm {
    username: String,
    password: String,
    password2: String,
}

#[derive(Deserialize)]
struct CsrfForm {
    _csrf: String,
}

#[derive(Deserialize)]
struct WorkerForm {
    name: String,
    kind: String,
    url: String,
    username: String,
    password: String,
    _csrf: String,
}

#[derive(Deserialize)]
struct WorkerEditForm {
    name: String,
    kind: String,
    url: String,
    username: String,
    password: String,
    _csrf: String,
}

#[derive(Deserialize)]
struct SessionForm {
    title: String,
    _csrf: String,
}

#[derive(Deserialize)]
struct MessageForm {
    text: String,
    agent: Option<String>,
    model: Option<String>,
    _csrf: String,
}

// ---------- template structs ----------

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "setup.html")]
struct SetupTemplate {
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    is_admin: bool,
    username: String,
    workers: Vec<WorkerView>,
}

#[derive(Clone)]
struct WorkerView {
    id: String,
    name: String,
    kind: String,
    url: String,
}

#[derive(Template)]
#[template(path = "workers_new.html")]
struct WorkersNewTemplate {
    is_admin: bool,
    username: String,
    csrf: String,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "workers_edit.html")]
struct WorkersEditTemplate {
    is_admin: bool,
    username: String,
    csrf: String,
    id: String,
    name: String,
    kind: String,
    url: String,
    username_field: String,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "invites.html")]
struct InvitesTemplate {
    is_admin: bool,
    username: String,
    csrf: String,
    new_link: Option<String>,
    pending: Vec<PendingInvite>,
}

struct PendingInvite {
    created_at: String,
}

#[derive(Template)]
#[template(path = "invite_claim.html")]
struct InviteClaimTemplate {
    token: String,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "worker_sessions.html")]
struct WorkerSessionsTemplate {
    is_admin: bool,
    username: String,
    csrf: String,
    worker: WorkerView,
    sessions: Vec<SessionView>,
    error: Option<String>,
}

#[derive(Clone)]
struct SessionView {
    id: String,
    title: String,
}

#[derive(Template)]
#[template(path = "session.html")]
struct SessionTemplate {
    csrf: String,
    worker: WorkerView,
    session_id: String,
    session_title: String,
    thread: String,
}

#[derive(Template)]
#[template(path = "session_thread.html")]
struct SessionThreadTemplate {
    messages: Vec<MessageView>,
    error: Option<String>,
}

struct MessageView {
    role: String,
    html: String,
}
