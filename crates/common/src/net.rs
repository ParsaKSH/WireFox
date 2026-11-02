use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use socket2::{Domain, SockRef, Socket, TcpKeepalive, Type};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};

/// Per-direction relay buffer. TLS hands out at most a few records per read, so
/// going much higher than this only costs memory (which matters on routers).
pub const RELAY_BUF: usize = 64 * 1024;

/// Disable Nagle and turn on TCP keepalive so dead peers and NAT-dropped idle
/// tunnel connections get noticed.
pub fn tune_tcp(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let ka = TcpKeepalive::new().with_time(Duration::from_secs(30)).with_interval(Duration::from_secs(10));
    #[cfg(not(windows))]
    let ka = ka.with_retries(3);
    SockRef::from(stream).set_tcp_keepalive(&ka)
}

/// Bind a listener with the same semantics on every OS: IPv6 sockets are
/// IPv6-only, so `0.0.0.0:p` and `[::]:p` can be listed side by side.
pub fn bind_listener(addr: SocketAddr) -> io::Result<TcpListener> {
    let sock = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
    if addr.is_ipv6() {
        sock.set_only_v6(true)?;
    }
    #[cfg(unix)]
    sock.set_reuse_address(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    sock.listen(1024)?;
    TcpListener::from_std(sock.into())
}

/// Accept the next connection. Transient errors (usually running out of file
/// descriptors) are logged and retried after a short pause instead of spinning.
pub async fn accept(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok(conn) => return conn,
            Err(e) => {
                tracing::warn!(error = %e, "accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// Copy both ways until both sides are closed. Returns `(a_to_b, b_to_a)`.
pub async fn relay<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    tokio::io::copy_bidirectional_with_sizes(a, b, RELAY_BUF, RELAY_BUF).await
}
