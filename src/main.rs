//! Q — Agentenzentrale. An HTTPS control plane and web UI for coding-agent
//! worker machines (opencode first, more pluggable later).

mod agent;
mod auth;
mod config;
mod crypto;
mod db;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use config::Cli;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let cli = Cli::parse_or_die();
    let addr: SocketAddr = cli.addr()?;
    let data_dir = cli.data_dir().to_path_buf();

    // Install a TLS crypto provider before any rustls usage (required by rustls 0.23).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Master secret for encrypting worker passwords at rest.
    let key = Arc::new(crypto::SecretKey::load_or_create(&data_dir)?);

    // Database (workers, users, sessions, invites).
    let db_path = data_dir.join("q.sqlite");
    let db = Arc::new(db::Db::open(&db_path, &key)?);

    let state = web::AppState {
        db,
        key,
        config: Arc::new(cli.clone()),
        limiter: Arc::new(auth::LoginLimiter::new()),
    };

    let app = web::router(state)
        .nest_service("/static", ServeDir::new("static"))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .into_make_service_with_connect_info::<SocketAddr>();

    if cli.tls {
        let (cert, key) = ensure_certs(&data_dir, cli.cert_path(), cli.key_path())?;
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
        tracing::info!("listening on https://{addr}");
        axum_server::bind_rustls(addr, tls).serve(app).await?;
    } else {
        tracing::info!("listening on http://{addr} (TLS disabled)");
        axum_server::bind(addr).serve(app).await?;
    }
    Ok(())
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Resolve a certificate/key pair, generating a self-signed one on first run
/// if none were provided (only when the `tls-bootstrap` feature is enabled).
#[cfg(feature = "tls-bootstrap")]
fn ensure_certs(
    data_dir: &std::path::Path,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    if let (Some(c), Some(k)) = (cert, key) {
        return Ok((c, k));
    }
    let cp = data_dir.join("selfsigned.crt");
    let kp = data_dir.join("selfsigned.key");
    if !cp.exists() || !kp.exists() {
        let cert = rcgen::generate_simple_self_signed(vec![
            "localhost".to_string(),
            "agentenzentrale".to_string(),
        ])?;
        std::fs::create_dir_all(data_dir)?;
        std::fs::write(&cp, cert.cert.pem())?;
        std::fs::write(&kp, cert.key_pair.serialize_pem())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&kp, std::fs::Permissions::from_mode(0o600))?;
            std::fs::set_permissions(&cp, std::fs::Permissions::from_mode(0o600))?;
        }
        tracing::warn!("generated a self-signed certificate; browsers will warn");
    }
    Ok((cp, kp))
}

#[cfg(not(feature = "tls-bootstrap"))]
fn ensure_certs(
    _data_dir: &std::path::Path,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    match (cert, key) {
        (Some(c), Some(k)) => Ok((c, k)),
        _ => anyhow::bail!(
            "HTTPS requested but no --cert/--key given and the `tls-bootstrap` feature is disabled"
        ),
    }
}
