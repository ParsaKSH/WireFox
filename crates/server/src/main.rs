use std::io::{BufRead, IsTerminal};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use argon2::password_hash::PasswordHasher;
use clap::{Parser, Subcommand};
use tracing::info;
use wirefox_server::config::ServerConfig;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(
    name = "wirefox-server",
    version,
    about = "WireFox server: SOCKS5 front end, tunnel hub for exit-nodes, and control API"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server.
    Run {
        /// Path to the config file.
        #[arg(short, long, default_value = "server.toml")]
        config: PathBuf,
    },
    /// Write a starter config with a fresh admin password, API key and certificate.
    Init {
        /// Directory to write server.toml into.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Where to keep state and the certificate (relative paths are relative to DIR).
        #[arg(long, default_value = "data")]
        data_dir: PathBuf,
        /// Address nodes use to reach this server, e.g. relay.example.com:7443.
        #[arg(long)]
        public_addr: Option<String>,
    },
    /// Hash a password for `api.admin_password_hash` (read from stdin).
    HashPassword,
    /// Generate an API key and the hash that goes in `api.api_key_hashes`.
    GenApiKey,
    /// Print the certificate fingerprint that nodes pin (`server_fingerprint`).
    Fingerprint {
        /// Path to the config file.
        #[arg(short, long, default_value = "server.toml")]
        config: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Run { config } => run(config),
        Cmd::Init { dir, data_dir, public_addr } => init(dir, data_dir, public_addr),
        Cmd::HashPassword => {
            let password = read_password()?;
            println!("{}", hash_password(&password)?);
            Ok(())
        }
        Cmd::GenApiKey => {
            let key = wirefox_common::random_token(32);
            println!("API key: {key}");
            println!("hash:    {}", wirefox_common::sha256_hex(key.as_bytes()));
            Ok(())
        }
        Cmd::Fingerprint { config } => {
            let cfg = ServerConfig::load(&config)?;
            println!("{}", wirefox_common::tls::load(&cfg.cert_path(), &cfg.key_path())?.fingerprint());
            Ok(())
        }
    }
}

fn read_password() -> Result<String> {
    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password("Password: ").context("reading password");
    }
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn hash_password(password: &str) -> Result<String> {
    if password.is_empty() {
        bail!("the password is empty");
    }
    argon2::Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
}

fn init(dir: PathBuf, data_dir: PathBuf, public_addr: Option<String>) -> Result<()> {
    let config_path = dir.join("server.toml");
    if config_path.exists() {
        bail!("{} already exists", config_path.display());
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let data_path = dir.join(&data_dir);
    std::fs::create_dir_all(&data_path).with_context(|| format!("creating {}", data_path.display()))?;
    let admin_password = wirefox_common::random_token(12);
    let api_key = wirefox_common::random_token(32);
    let public_addr_line = match &public_addr {
        Some(addr) => format!("public_addr = {}", toml_string(addr)),
        None => "# public_addr = \"relay.example.com:7443\"   # used in generated node configs".into(),
    };
    let config = format!(
        r#"data_dir = {data_dir}

[tls]
# cert = "server.crt"   # default: generated in data_dir on first start
# key = "server.key"
sans = ["wirefox"]

[tunnel]
listen = "0.0.0.0:7443"
{public_addr_line}

[socks]
listen = ["0.0.0.0:1080"]
tls_listen = ["0.0.0.0:1443"]
handshake_timeout_secs = 10
connect_timeout_secs = 15
# How connections are spread over nodes: round-robin, least-ping, least-load, random
balance = "least-load"

[api]
listen = "127.0.0.1:9090"
tls = false
# Leave out both admin_* lines to allow only API keys.
admin_username = "admin"
admin_password_hash = "{password_hash}"
api_key_hashes = ["{api_key_hash}"]
session_ttl_secs = 43200
cors_origins = []
"#,
        data_dir = toml_string(&data_dir.display().to_string()),
        password_hash = hash_password(&admin_password)?,
        api_key_hash = wirefox_common::sha256_hex(api_key.as_bytes()),
    );
    wirefox_common::files::write_private(&config_path, config.as_bytes())
        .with_context(|| format!("writing {}", config_path.display()))?;
    let cert = wirefox_common::tls::load_or_generate(
        &data_path.join("server.crt"),
        &data_path.join("server.key"),
        &["wirefox".to_string()],
    )?;

    println!("Wrote {}", config_path.display());
    println!();
    println!("admin username:      admin");
    println!("admin password:      {admin_password}");
    println!("API key:             {api_key}");
    println!("server fingerprint:  {}", cert.fingerprint());
    println!();
    println!("The password and API key are not stored anywhere; save them now.");
    Ok(())
}

/// A TOML string literal, quoted and escaped.
fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn run(config_path: PathBuf) -> Result<()> {
    wirefox_common::init_tracing("info");
    let cfg = ServerConfig::load(&config_path)?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async move {
        let server = wirefox_server::start(cfg).await?;
        info!(
            version = wirefox_common::VERSION,
            nodes = server.store.nodes.read().len(),
            users = server.store.users.read().len(),
            fingerprint = %server.fingerprint,
            "wirefox-server started"
        );
        shutdown_signal().await;
        info!("shutting down");
        server.store.save().await
    })
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("installing SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(windows)]
async fn shutdown_signal() {
    use tokio::signal::windows;
    // Closing the console window, logging off and shutting down all end the
    // process shortly after these fire, so the final save has to happen now.
    let (Ok(mut close), Ok(mut shutdown), Ok(mut logoff)) =
        (windows::ctrl_close(), windows::ctrl_shutdown(), windows::ctrl_logoff())
    else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = close.recv() => {}
        _ = shutdown.recv() => {}
        _ = logoff.recv() => {}
    }
}
