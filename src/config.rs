use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "agentenzentrale",
    version,
    about = "Agentenzentrale — unified HTTPS control plane and web UI for coding-agent worker machines."
)]
pub struct Cli {
    /// Listen address, e.g. 0.0.0.0:8443
    #[arg(long, default_value = "0.0.0.0:8443", env = "AGENTENZENTRALE_ADDR")]
    pub addr: String,

    /// Directory for the SQLite database, state, and secrets
    #[arg(long, default_value = "./data", env = "AGENTENZENTRALE_DATA_DIR")]
    pub data_dir: String,

    /// TLS certificate (PEM). If omitted and `--tls` is on, a self-signed cert is generated.
    #[arg(long, env = "AGENTENZENTRALE_CERT")]
    pub cert: Option<String>,

    /// TLS private key (PEM)
    #[arg(long, env = "AGENTENZENTRALE_KEY")]
    pub key: Option<String>,

    /// Serve HTTPS. Disable to run plain HTTP behind a TLS-terminating reverse proxy.
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true, env = "AGENTENZENTRALE_TLS")]
    pub tls: bool,

    /// Public base URL advertised to browsers, e.g. https://q.example.com
    #[arg(long, env = "AGENTENZENTRALE_PUBLIC_URL")]
    pub public_url: Option<String>,
}

impl Cli {
    pub fn parse_or_die() -> Self {
        Cli::parse()
    }

    pub fn addr(&self) -> anyhow::Result<SocketAddr> {
        self.addr
            .parse()
            .context("invalid --addr (expected host:port)")
    }

    pub fn cert_path(&self) -> Option<PathBuf> {
        self.cert.as_ref().map(PathBuf::from)
    }

    pub fn key_path(&self) -> Option<PathBuf> {
        self.key.as_ref().map(PathBuf::from)
    }

    pub fn data_dir(&self) -> &Path {
        Path::new(&self.data_dir)
    }
}
