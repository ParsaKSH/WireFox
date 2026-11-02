//! Runs a real server and node in-process and drives them over SOCKS5 and the API.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use argon2::password_hash::PasswordHasher;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wirefox_server::config::ServerConfig;
use wirefox_server::Server;

const API_KEY: &str = "test-api-key";
const ADMIN_PASSWORD: &str = "admin-password";

struct Fixture {
    server: Server,
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start_server() -> Fixture {
    let dir = std::env::temp_dir().join(format!("wirefox-e2e-{}", wirefox_common::random_token(8)));
    std::fs::create_dir_all(&dir).unwrap();
    let admin_hash = argon2::Argon2::default().hash_password(ADMIN_PASSWORD.as_bytes()).unwrap().to_string();
    let config = format!(
        r#"
        data_dir = "data"
        [tunnel]
        listen = "127.0.0.1:0"
        [socks]
        listen = ["127.0.0.1:0"]
        tls_listen = ["127.0.0.1:0"]
        connect_timeout_secs = 5
        [api]
        listen = "127.0.0.1:0"
        admin_username = "admin"
        admin_password_hash = "{admin_hash}"
        api_key_hashes = ["{}"]
        "#,
        wirefox_common::sha256_hex(API_KEY.as_bytes())
    );
    let path = dir.join("server.toml");
    std::fs::write(&path, config).unwrap();
    let server = wirefox_server::start(ServerConfig::load(&path).unwrap()).await.unwrap();
    Fixture { server, dir }
}

/// Minimal HTTP/1.1 client, enough for the JSON API.
async fn api(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
    let request = format!(
        "{method} /api/v1{path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n{auth}\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let status = response[9..12].parse().unwrap();
    let json = response.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str(b).ok()).unwrap_or(Value::Null);
    (status, json)
}

async fn start_node(fixture: &Fixture, node_config: &str, pool_size: u32) {
    let config = format!("{node_config}\npool_size = {pool_size}\nallow_local = true\n");
    let node = wirefox_node::Node::new(wirefox_node::NodeConfig::parse(&config).unwrap()).unwrap();
    tokio::spawn(node.run());
    for _ in 0..100 {
        let online = fixture.server.store.nodes.read().values().any(|n| n.online_session().is_some());
        if online {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("node never came online");
}

async fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let (mut r, mut w) = stream.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

/// SOCKS5 CONNECT; returns the stream and the reply code.
async fn socks_connect(proxy: SocketAddr, user: &str, pass: &str, target: SocketAddr) -> (TcpStream, u8) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 2]).await.unwrap();
    let mut buf = [0u8; 2];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(buf, [5, 2]);
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(pass.len() as u8);
    auth.extend_from_slice(pass.as_bytes());
    s.write_all(&auth).await.unwrap();
    s.read_exact(&mut buf).await.unwrap();
    if buf[1] != 0 {
        return (s, 0xFF);
    }
    let SocketAddr::V4(v4) = target else { panic!("test targets are IPv4") };
    let mut req = vec![5, 1, 0, 1];
    req.extend_from_slice(&v4.ip().octets());
    req.extend_from_slice(&v4.port().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut head = [0u8; 4];
    s.read_exact(&mut head).await.unwrap();
    let rest = if head[3] == 4 { 18 } else { 6 };
    let mut skip = vec![0u8; rest];
    s.read_exact(&mut skip).await.unwrap();
    (s, head[1])
}

async fn assert_echo(stream: &mut TcpStream) {
    let payload = wirefox_common::random_token(4096);
    stream.write_all(payload.as_bytes()).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    stream.read_exact(&mut back).await.unwrap();
    assert_eq!(back, payload.as_bytes());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_end_to_end() {
    let fixture = start_server().await;
    let api_addr = fixture.server.api_addr;
    let socks = fixture.server.socks_addrs[0];
    let target = echo_server().await;

    // API auth.
    assert_eq!(api(api_addr, "GET", "/status", None, None).await.0, 401);
    assert_eq!(api(api_addr, "GET", "/status", Some("wrong"), None).await.0, 401);
    let (status, login) =
        api(api_addr, "POST", "/auth/login", None, Some(json!({"username": "admin", "password": ADMIN_PASSWORD})))
            .await;
    assert_eq!(status, 200);
    let session = login["token"].as_str().unwrap().to_string();
    assert_eq!(api(api_addr, "GET", "/status", Some(&session), None).await.0, 200);

    // Create a node and a user through the API, then bring the node up.
    let (status, created) = api(api_addr, "POST", "/nodes", Some(API_KEY), Some(json!({"id": "n1"}))).await;
    assert_eq!(status, 200, "{created}");
    let node_config = created["node_config"].as_str().unwrap();
    assert!(node_config.contains(&fixture.server.fingerprint));
    let (status, _) =
        api(api_addr, "POST", "/users", Some(API_KEY), Some(json!({"username": "alice", "password": "pw"}))).await;
    assert_eq!(status, 200);
    start_node(&fixture, node_config, 2).await;

    // Plain round trip.
    let (mut stream, code) = socks_connect(socks, "alice", "pw", target).await;
    assert_eq!(code, 0);
    assert_echo(&mut stream).await;
    drop(stream);

    // Wrong password.
    assert_eq!(socks_connect(socks, "alice", "nope", target).await.1, 0xFF);

    // A burst well past the pool size has to be served from on-demand connections.
    let burst: Vec<_> = (0..40)
        .map(|_| {
            tokio::spawn(async move {
                let (mut stream, code) = socks_connect(socks, "alice", "pw", target).await;
                assert_eq!(code, 0);
                assert_echo(&mut stream).await;
            })
        })
        .collect();
    for task in burst {
        task.await.unwrap();
    }
    // A second burst right away, with leftovers from the first one in the pool.
    let second: Vec<_> =
        (0..10).map(|_| tokio::spawn(async move { socks_connect(socks, "alice", "pw", target).await.1 })).collect();
    for task in second {
        assert_eq!(task.await.unwrap(), 0);
    }

    // Disabling the user cuts its live stream.
    let (mut live, code) = socks_connect(socks, "alice", "pw", target).await;
    assert_eq!(code, 0);
    assert_echo(&mut live).await;
    let (status, _) = api(api_addr, "PATCH", "/users/alice", Some(API_KEY), Some(json!({"enabled": false}))).await;
    assert_eq!(status, 200);
    let mut buf = [0u8; 16];
    let n = tokio::time::timeout(Duration::from_secs(5), live.read(&mut buf)).await.unwrap().unwrap_or(0);
    assert_eq!(n, 0, "stream should be closed");
    assert_eq!(socks_connect(socks, "alice", "pw", target).await.1, 0xFF);

    // An egress the node can't resolve is reported back, and traffic stops rather
    // than leaving through some other interface.
    api(api_addr, "PATCH", "/users/alice", Some(API_KEY), Some(json!({"enabled": true}))).await;
    let (status, _) =
        api(api_addr, "PATCH", "/nodes/n1", Some(API_KEY), Some(json!({"egress": {"interface": "nope0"}}))).await;
    assert_eq!(status, 200);
    let mut error = Value::Null;
    for _ in 0..50 {
        let (_, node) = api(api_addr, "GET", "/nodes/n1", Some(API_KEY), None).await;
        error = node["session"]["egress_error"].clone();
        if !error.is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(error.as_str().unwrap().contains("nope0"), "{error}");
    assert_ne!(socks_connect(socks, "alice", "pw", target).await.1, 0);

    // Clearing the override restores the node's own egress.
    let (status, _) = api(api_addr, "PATCH", "/nodes/n1", Some(API_KEY), Some(json!({"egress": null}))).await;
    assert_eq!(status, 200);
    let mut code = 0xFF;
    for _ in 0..50 {
        code = socks_connect(socks, "alice", "pw", target).await.1;
        if code == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(code, 0);
    let (mut stream, _) = socks_connect(socks, "alice", "pw", target).await;
    assert_echo(&mut stream).await;

    // Traffic was counted.
    let (_, user) = api(api_addr, "GET", "/users/alice", Some(API_KEY), None).await;
    assert!(user["traffic"]["bytes_up"].as_u64().unwrap() > 0);
    assert!(user["traffic"]["total_connections"].as_u64().unwrap() >= 52);

    // Logging out invalidates the session token.
    assert_eq!(api(api_addr, "POST", "/auth/logout", Some(&session), None).await.0, 200);
    assert_eq!(api(api_addr, "GET", "/status", Some(&session), None).await.0, 401);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rotated_token_disconnects_node() {
    let fixture = start_server().await;
    let api_addr = fixture.server.api_addr;
    let (_, created) = api(api_addr, "POST", "/nodes", Some(API_KEY), Some(json!({"id": "n1"}))).await;
    start_node(&fixture, created["node_config"].as_str().unwrap(), 1).await;

    let (status, _) = api(api_addr, "POST", "/nodes/n1/rotate-token", Some(API_KEY), None).await;
    assert_eq!(status, 200);
    // The node keeps retrying with its old token, which must not get it back in.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (_, node) = api(api_addr, "GET", "/nodes/n1", Some(API_KEY), None).await;
    assert_eq!(node["online"], false);
}
