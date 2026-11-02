//! Throughput benchmark through a SOCKS5 proxy.
//!
//!   wirefox-bench sink --listen 127.0.0.1:5201
//!   wirefox-bench run --socks 127.0.0.1:1080 --user u --pass p --target 127.0.0.1:5201 --streams 8

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const BUF: usize = 256 * 1024;

#[derive(Parser)]
#[command(name = "wirefox-bench", version, about = "Throughput benchmark through a SOCKS5 proxy")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Dir {
    /// Client sends, sink discards.
    Up,
    /// Sink sends, client discards.
    Down,
}

#[derive(Subcommand)]
enum Cmd {
    /// Target server: discards uploads / streams zeros for downloads.
    Sink {
        #[arg(long, default_value = "127.0.0.1:5201")]
        listen: SocketAddr,
    },
    /// Open streams through the SOCKS5 proxy and measure throughput.
    Run {
        #[arg(long)]
        socks: SocketAddr,
        #[arg(long)]
        user: String,
        #[arg(long)]
        pass: String,
        #[arg(long)]
        target: SocketAddr,
        #[arg(long, default_value_t = 4)]
        streams: usize,
        #[arg(long, default_value_t = 10)]
        secs: u64,
        #[arg(long, value_enum, default_value = "down")]
        dir: Dir,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Sink { listen } => sink(listen).await,
        Cmd::Run { socks, user, pass, target, streams, secs, dir } => {
            run(socks, user, pass, target, streams, Duration::from_secs(secs), dir).await
        }
    }
}

async fn sink(listen: SocketAddr) -> Result<()> {
    let l = TcpListener::bind(listen).await?;
    println!("sink listening on {listen}");
    loop {
        let (mut s, _) = l.accept().await?;
        s.set_nodelay(true)?;
        tokio::spawn(async move {
            let mut buf = vec![0u8; BUF];
            let mut mode = [0u8; 1];
            if s.read_exact(&mut mode).await.is_err() {
                return;
            }
            if mode[0] == b'D' {
                while s.write_all(&buf).await.is_ok() {}
            } else {
                while matches!(s.read(&mut buf).await, Ok(n) if n > 0) {}
            }
        });
    }
}

async fn socks_connect(socks: SocketAddr, user: &str, pass: &str, target: SocketAddr) -> Result<TcpStream> {
    anyhow::ensure!(user.len() <= 255 && pass.len() <= 255, "username and password must be at most 255 bytes");
    let mut s = TcpStream::connect(socks).await?;
    s.set_nodelay(true)?;
    s.write_all(&[5, 1, 2]).await?;
    let mut r = [0u8; 2];
    s.read_exact(&mut r).await?;
    if r != [5, 2] {
        bail!("proxy refused auth method: {r:?}");
    }
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(pass.len() as u8);
    auth.extend_from_slice(pass.as_bytes());
    s.write_all(&auth).await?;
    s.read_exact(&mut r).await?;
    if r[1] != 0 {
        bail!("proxy authentication failed");
    }
    let mut req = vec![5, 1, 0];
    match target {
        SocketAddr::V4(a) => {
            req.push(1);
            req.extend_from_slice(&a.ip().octets());
        }
        SocketAddr::V6(a) => {
            req.push(4);
            req.extend_from_slice(&a.ip().octets());
        }
    }
    req.extend_from_slice(&target.port().to_be_bytes());
    s.write_all(&req).await?;
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await?;
    if head[1] != 0 {
        bail!("proxy connect failed: rep={}", head[1]);
    }
    let rest = match head[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => s.read_u8().await? as usize + 2,
        t => bail!("bad atyp {t}"),
    };
    let mut skip = vec![0u8; rest];
    s.read_exact(&mut skip).await?;
    Ok(s)
}

async fn run(
    socks: SocketAddr,
    user: String,
    pass: String,
    target: SocketAddr,
    streams: usize,
    dur: Duration,
    dir: Dir,
) -> Result<()> {
    let total = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let t0 = Instant::now();
    let mut conns = Vec::new();
    for _ in 0..streams {
        conns.push(socks_connect(socks, &user, &pass, target).await?);
    }
    println!("{streams} streams connected in {:?}", t0.elapsed());

    let start = Instant::now();
    let mut tasks = Vec::new();
    for mut s in conns {
        let total = total.clone();
        let stop = stop.clone();
        tasks.push(tokio::spawn(async move {
            let mut buf = vec![0u8; BUF];
            match dir {
                Dir::Up => {
                    s.write_all(b"U").await?;
                    while !stop.load(Ordering::Relaxed) {
                        s.write_all(&buf).await?;
                        total.fetch_add(BUF as u64, Ordering::Relaxed);
                    }
                }
                Dir::Down => {
                    s.write_all(b"D").await?;
                    while !stop.load(Ordering::Relaxed) {
                        let n = s.read(&mut buf).await?;
                        if n == 0 {
                            break;
                        }
                        total.fetch_add(n as u64, Ordering::Relaxed);
                    }
                }
            }
            anyhow::Ok(())
        }));
    }

    let mut last = 0u64;
    for sec in 1..=dur.as_secs() {
        tokio::time::sleep_until((start + Duration::from_secs(sec)).into()).await;
        let now = total.load(Ordering::Relaxed);
        println!("[{sec:>3}s] {:>7.2} Gbit/s", (now - last) as f64 * 8.0 / 1e9);
        last = now;
    }
    stop.store(true, Ordering::Relaxed);
    let elapsed = start.elapsed().as_secs_f64();
    let bytes = total.load(Ordering::Relaxed);
    println!("total: {:.2} GB in {elapsed:.1}s = {:.2} Gbit/s", bytes as f64 / 1e9, bytes as f64 * 8.0 / 1e9 / elapsed);
    for t in tasks {
        t.abort();
    }
    Ok(())
}
