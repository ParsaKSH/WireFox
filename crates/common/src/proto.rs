//! Wire protocol between the server and the exit-node.
//!
//! Every tunnel connection is TLS 1.3, dialed by the node (it's a reverse
//! tunnel). Right after the handshake the node sends a [`Hello`]:
//!
//! ```text
//! MAGIC(4) VERSION(1) KIND(1) ID_LEN(1) ID TOKEN_LEN(1) TOKEN
//! ```
//!
//! and the server answers with a single status byte from [`hello_status`].
//!
//! * A **control** connection then carries length-prefixed JSON frames
//!   (`LEN: u32 BE`, then the JSON body) of [`ServerMsg`] and [`NodeMsg`].
//! * A **data** connection sits idle in the server's pool until a client needs
//!   it. The server writes `CMD(1) TARGET`, the node dials the target and
//!   answers `REP(1) BOUND_ADDR`, and from then on the connection is a plain
//!   byte pipe. Each data connection carries exactly one stream, so there is no
//!   multiplexing overhead and no head-of-line blocking between streams.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAGIC: [u8; 4] = *b"WFX1";
pub const PROTO_VERSION: u8 = 1;
pub const MAX_CONTROL_FRAME: usize = 1 << 20;

/// TLS SNI the node sends by default. It has no security role: the server
/// certificate is pinned by fingerprint.
pub const DEFAULT_SERVER_NAME: &str = "wirefox";

/// How often the server pings over the control connection.
pub const PING_INTERVAL: Duration = Duration::from_secs(15);
/// Either side gives up on a control connection that has been silent this long
/// (three missed pings).
pub const CONTROL_IDLE_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ConnKind {
    Control = 1,
    Data = 2,
}

impl TryFrom<u8> for ConnKind {
    type Error = u8;

    fn try_from(v: u8) -> Result<Self, u8> {
        match v {
            1 => Ok(ConnKind::Control),
            2 => Ok(ConnKind::Data),
            other => Err(other),
        }
    }
}

pub mod hello_status {
    pub const OK: u8 = 0;
    pub const BAD_AUTH: u8 = 1;
    pub const BAD_VERSION: u8 = 2;
    pub const DISABLED: u8 = 3;
    pub const NO_SESSION: u8 = 4;
    pub const BAD_REQUEST: u8 = 5;
    pub const POOL_FULL: u8 = 6;

    pub fn describe(code: u8) -> &'static str {
        match code {
            OK => "ok",
            BAD_AUTH => "authentication failed",
            BAD_VERSION => "protocol version mismatch",
            DISABLED => "node is disabled",
            NO_SESSION => "no control session",
            BAD_REQUEST => "malformed hello",
            POOL_FULL => "too many idle connections",
            _ => "unknown error",
        }
    }
}

/// Data-connection command sent by the server.
pub const CMD_CONNECT: u8 = 1;

/// Reply codes are the SOCKS5 `REP` values, so the server can pass them through.
pub mod rep {
    use std::io;

    pub const SUCCEEDED: u8 = 0x00;
    pub const GENERAL_FAILURE: u8 = 0x01;
    pub const NOT_ALLOWED: u8 = 0x02;
    pub const NETWORK_UNREACHABLE: u8 = 0x03;
    pub const HOST_UNREACHABLE: u8 = 0x04;
    pub const CONNECTION_REFUSED: u8 = 0x05;
    pub const TTL_EXPIRED: u8 = 0x06;
    pub const COMMAND_NOT_SUPPORTED: u8 = 0x07;
    pub const ADDRESS_NOT_SUPPORTED: u8 = 0x08;

    pub fn from_io_error(e: &io::Error) -> u8 {
        match e.kind() {
            io::ErrorKind::ConnectionRefused => CONNECTION_REFUSED,
            io::ErrorKind::TimedOut => TTL_EXPIRED,
            io::ErrorKind::NetworkUnreachable => NETWORK_UNREACHABLE,
            io::ErrorKind::HostUnreachable | io::ErrorKind::NotFound => HOST_UNREACHABLE,
            io::ErrorKind::AddrNotAvailable => ADDRESS_NOT_SUPPORTED,
            io::ErrorKind::PermissionDenied => NOT_ALLOWED,
            _ => GENERAL_FAILURE,
        }
    }
}

const ATYP_V4: u8 = 1;
const ATYP_DOMAIN: u8 = 3;
const ATYP_V6: u8 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    Ip(IpAddr),
    Domain(String),
}

/// Destination address in SOCKS5 encoding: `ATYP ADDR PORT`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub host: Host,
    pub port: u16,
}

impl Target {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        match &self.host {
            Host::Ip(IpAddr::V4(ip)) => {
                buf.push(ATYP_V4);
                buf.extend_from_slice(&ip.octets());
            }
            Host::Ip(IpAddr::V6(ip)) => {
                buf.push(ATYP_V6);
                buf.extend_from_slice(&ip.octets());
            }
            Host::Domain(d) => {
                // Domains come from SOCKS requests or a config file; neither can
                // exceed 255 bytes and still be a valid name.
                debug_assert!(d.len() <= 255, "domain too long for SOCKS5 encoding");
                buf.push(ATYP_DOMAIN);
                buf.push(d.len() as u8);
                buf.extend_from_slice(d.as_bytes());
            }
        }
        buf.extend_from_slice(&self.port.to_be_bytes());
    }

    pub async fn read_from<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> io::Result<Self> {
        let host = match r.read_u8().await? {
            ATYP_V4 => {
                let mut b = [0u8; 4];
                r.read_exact(&mut b).await?;
                Host::Ip(IpAddr::V4(Ipv4Addr::from(b)))
            }
            ATYP_V6 => {
                let mut b = [0u8; 16];
                r.read_exact(&mut b).await?;
                Host::Ip(IpAddr::V6(Ipv6Addr::from(b)))
            }
            ATYP_DOMAIN => {
                let name = read_short_string(r).await?;
                // Some clients send literal IPs as "domains".
                match name.parse::<IpAddr>() {
                    Ok(ip) => Host::Ip(ip),
                    Err(_) => Host::Domain(name),
                }
            }
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unsupported address type")),
        };
        let port = r.read_u16().await?;
        Ok(Target { host, port })
    }

    pub fn from_socket_addr(addr: SocketAddr) -> Self {
        Target { host: Host::Ip(addr.ip()), port: addr.port() }
    }

    pub fn unspecified() -> Self {
        Target { host: Host::Ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), port: 0 }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Host::Ip(IpAddr::V6(ip)) => write!(f, "[{ip}]:{}", self.port),
            Host::Ip(ip) => write!(f, "{ip}:{}", self.port),
            Host::Domain(d) => write!(f, "{d}:{}", self.port),
        }
    }
}

/// Reads a `LEN(1) BYTES` UTF-8 string, the format SOCKS5 uses for domains and credentials.
pub async fn read_short_string<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> io::Result<String> {
    let len = r.read_u8().await? as usize;
    let mut b = vec![0u8; len];
    r.read_exact(&mut b).await?;
    String::from_utf8(b).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "string is not utf-8"))
}

#[derive(Clone, Debug)]
pub struct Hello {
    pub kind: ConnKind,
    pub node_id: String,
    pub token: String,
}

/// A rejected hello: the status byte to send back, and why.
#[derive(Debug)]
pub struct HelloError {
    pub status: u8,
    pub error: io::Error,
}

impl Hello {
    pub async fn write_to<W: AsyncWrite + Unpin + ?Sized>(&self, w: &mut W) -> io::Result<()> {
        if self.node_id.len() > 255 || self.token.len() > 255 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "node id or token too long"));
        }
        let mut buf = Vec::with_capacity(8 + self.node_id.len() + self.token.len());
        buf.extend_from_slice(&MAGIC);
        buf.push(PROTO_VERSION);
        buf.push(self.kind as u8);
        buf.push(self.node_id.len() as u8);
        buf.extend_from_slice(self.node_id.as_bytes());
        buf.push(self.token.len() as u8);
        buf.extend_from_slice(self.token.as_bytes());
        w.write_all(&buf).await?;
        w.flush().await
    }

    pub async fn read_from<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> Result<Self, HelloError> {
        let bad_request = |error| HelloError { status: hello_status::BAD_REQUEST, error };
        let invalid = |msg: String| bad_request(io::Error::new(io::ErrorKind::InvalidData, msg));

        let mut head = [0u8; 6];
        r.read_exact(&mut head).await.map_err(bad_request)?;
        if head[..4] != MAGIC {
            return Err(invalid("bad magic".into()));
        }
        if head[4] != PROTO_VERSION {
            return Err(HelloError {
                status: hello_status::BAD_VERSION,
                error: io::Error::new(io::ErrorKind::InvalidData, format!("unsupported protocol version {}", head[4])),
            });
        }
        let kind = ConnKind::try_from(head[5]).map_err(|k| invalid(format!("unknown connection kind {k}")))?;
        let node_id = read_short_string(r).await.map_err(bad_request)?;
        let token = read_short_string(r).await.map_err(bad_request)?;
        Ok(Hello { kind, node_id, token })
    }
}

pub async fn write_connect<W: AsyncWrite + Unpin + ?Sized>(w: &mut W, target: &Target) -> io::Result<()> {
    let mut buf = Vec::with_capacity(32);
    buf.push(CMD_CONNECT);
    target.encode(&mut buf);
    w.write_all(&buf).await?;
    w.flush().await
}

pub async fn read_connect<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> io::Result<Target> {
    let cmd = r.read_u8().await?;
    if cmd != CMD_CONNECT {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown command {cmd}")));
    }
    Target::read_from(r).await
}

pub async fn write_reply<W: AsyncWrite + Unpin + ?Sized>(w: &mut W, rep: u8, bound: &Target) -> io::Result<()> {
    let mut buf = Vec::with_capacity(32);
    buf.push(rep);
    bound.encode(&mut buf);
    w.write_all(&buf).await?;
    w.flush().await
}

pub async fn read_reply<R: AsyncRead + Unpin + ?Sized>(r: &mut R) -> io::Result<(u8, Target)> {
    let rep = r.read_u8().await?;
    let bound = Target::read_from(r).await?;
    Ok((rep, bound))
}

/// Which interface and/or source address the node uses for outbound traffic.
/// Both may be set: the interface pins the route, the source IP pins the address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ip: Option<IpAddr>,
}

impl EgressSpec {
    pub fn is_default(&self) -> bool {
        self.interface.is_none() && self.source_ip.is_none()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InterfaceInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_name: Option<String>,
    pub index: Option<u32>,
    pub up: bool,
    pub addrs: Vec<IpAddr>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NodeInfo {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub version: String,
    pub pool_size: u32,
    /// Egress from the node's own config file.
    pub local_egress: EgressSpec,
    /// Egress currently in use (differs from `local_egress` while an override is applied).
    pub active_egress: EgressSpec,
    pub interfaces: Vec<InterfaceInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Ping {
        seq: u64,
    },
    /// Open `count` more data connections; the pool ran dry.
    Demand {
        count: u32,
    },
    /// Override the egress. `None` goes back to the node's own config.
    SetEgress {
        egress: Option<EgressSpec>,
    },
    /// Ask the node to send its interface list again.
    RefreshInterfaces,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeMsg {
    Info(NodeInfo),
    Pong {
        seq: u64,
    },
    Interfaces {
        interfaces: Vec<InterfaceInfo>,
    },
    /// Answer to `SetEgress`: the egress now in use, and the error if the
    /// requested one could not be applied.
    EgressApplied {
        egress: EgressSpec,
        error: Option<String>,
    },
}

pub async fn write_frame<W, T>(w: &mut W, msg: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin + ?Sized,
    T: Serialize,
{
    let body = serde_json::to_vec(msg).map_err(io::Error::other)?;
    if body.len() > MAX_CONTROL_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "control frame too large"));
    }
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(&body);
    w.write_all(&buf).await?;
    w.flush().await
}

pub async fn read_frame<R, T>(r: &mut R) -> io::Result<T>
where
    R: AsyncRead + Unpin + ?Sized,
    T: DeserializeOwned,
{
    let len = r.read_u32().await? as usize;
    if len > MAX_CONTROL_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "control frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn target_roundtrip() {
        for t in [
            Target { host: Host::Ip("1.2.3.4".parse().unwrap()), port: 80 },
            Target { host: Host::Ip("::1".parse().unwrap()), port: 443 },
            Target { host: Host::Domain("example.com".into()), port: 8080 },
        ] {
            let mut buf = Vec::new();
            t.encode(&mut buf);
            let back = Target::read_from(&mut buf.as_slice()).await.unwrap();
            assert_eq!(t, back);
        }
    }

    #[tokio::test]
    async fn literal_ip_domain_becomes_ip() {
        let mut buf = vec![ATYP_DOMAIN, 8];
        buf.extend_from_slice(b"10.0.0.1");
        buf.extend_from_slice(&443u16.to_be_bytes());
        let t = Target::read_from(&mut buf.as_slice()).await.unwrap();
        assert_eq!(t.host, Host::Ip("10.0.0.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn unknown_atyp_is_rejected() {
        let buf = [9u8, 0, 0];
        let err = Target::read_from(&mut buf.as_slice()).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn hello_roundtrip() {
        let h = Hello { kind: ConnKind::Data, node_id: "node-1".into(), token: "abc".into() };
        let mut buf = Vec::new();
        h.write_to(&mut buf).await.unwrap();
        let back = Hello::read_from(&mut buf.as_slice()).await.unwrap();
        assert_eq!(back.kind, ConnKind::Data);
        assert_eq!(back.node_id, "node-1");
        assert_eq!(back.token, "abc");
    }

    #[tokio::test]
    async fn hello_rejects_bad_magic_and_version() {
        let mut buf = Vec::new();
        Hello { kind: ConnKind::Control, node_id: "n".into(), token: "t".into() }.write_to(&mut buf).await.unwrap();

        let mut bad_magic = buf.clone();
        bad_magic[0] = b'X';
        let err = Hello::read_from(&mut bad_magic.as_slice()).await.unwrap_err();
        assert_eq!(err.status, hello_status::BAD_REQUEST);

        let mut bad_version = buf.clone();
        bad_version[4] = PROTO_VERSION + 1;
        let err = Hello::read_from(&mut bad_version.as_slice()).await.unwrap_err();
        assert_eq!(err.status, hello_status::BAD_VERSION);

        let mut bad_kind = buf;
        bad_kind[5] = 9;
        let err = Hello::read_from(&mut bad_kind.as_slice()).await.unwrap_err();
        assert_eq!(err.status, hello_status::BAD_REQUEST);
    }

    #[tokio::test]
    async fn connect_roundtrip() {
        let target = Target { host: Host::Domain("example.org".into()), port: 443 };
        let mut buf = Vec::new();
        write_connect(&mut buf, &target).await.unwrap();
        assert_eq!(read_connect(&mut buf.as_slice()).await.unwrap(), target);
    }

    #[tokio::test]
    async fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &ServerMsg::Demand { count: 3 }).await.unwrap();
        let back: ServerMsg = read_frame(&mut buf.as_slice()).await.unwrap();
        assert!(matches!(back, ServerMsg::Demand { count: 3 }));
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected() {
        let buf = ((MAX_CONTROL_FRAME + 1) as u32).to_be_bytes();
        let err = read_frame::<_, ServerMsg>(&mut buf.as_slice()).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
