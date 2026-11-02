//! Nodes, users and live connections, kept in memory and persisted to a JSON
//! file. Traffic counters are plain atomics so the data path never takes a lock.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use wirefox_common::proto::EgressSpec;

use crate::tunnel::NodeSession;

/// Traffic counters are flushed to disk at least this often.
const SAVE_INTERVAL: Duration = Duration::from_secs(60);

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Default)]
pub struct Traffic {
    /// Client to target.
    pub up: AtomicU64,
    /// Target to client.
    pub down: AtomicU64,
    pub active: AtomicU64,
    pub total: AtomicU64,
}

#[derive(Serialize, Clone, Copy)]
pub struct TrafficSnapshot {
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub active_connections: u64,
    pub total_connections: u64,
}

impl Traffic {
    fn with_totals(up: u64, down: u64) -> Self {
        Traffic { up: AtomicU64::new(up), down: AtomicU64::new(down), ..Default::default() }
    }

    pub fn snapshot(&self) -> TrafficSnapshot {
        TrafficSnapshot {
            bytes_up: self.up.load(Ordering::Relaxed),
            bytes_down: self.down.load(Ordering::Relaxed),
            active_connections: self.active.load(Ordering::Relaxed),
            total_connections: self.total.load(Ordering::Relaxed),
        }
    }

    pub fn reset(&self) {
        self.up.store(0, Ordering::Relaxed);
        self.down.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
    }

    /// Count one more active connection until the returned guard is dropped.
    pub fn enter(self: &Arc<Self>) -> ActiveGuard {
        self.active.fetch_add(1, Ordering::AcqRel);
        ActiveGuard(self.clone())
    }

    /// Like [`Traffic::enter`], but refuses if that would exceed `limit`.
    pub fn try_enter(self: &Arc<Self>, limit: Option<u32>) -> Option<ActiveGuard> {
        // Count first, then check what was there before: concurrent callers each
        // see a distinct value, so they can't all squeeze in under the limit.
        let before = self.active.fetch_add(1, Ordering::AcqRel);
        let guard = ActiveGuard(self.clone());
        limit.is_none_or(|max| before < u64::from(max)).then_some(guard)
    }
}

pub struct ActiveGuard(Arc<Traffic>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NodeCfg {
    pub name: String,
    /// Hex SHA-256 of the node's token.
    pub token_hash: String,
    pub enabled: bool,
    /// Egress pushed to the node, overriding its own config.
    #[serde(default)]
    pub egress: Option<EgressSpec>,
    pub created_at: u64,
}

impl NodeCfg {
    pub fn token_matches(&self, token: &str) -> bool {
        let expected = hex::decode(&self.token_hash).unwrap_or_default();
        wirefox_common::ct_eq(&wirefox_common::sha256(token.as_bytes()), &expected)
    }
}

pub struct NodeEntry {
    pub id: String,
    pub cfg: RwLock<NodeCfg>,
    pub stats: Arc<Traffic>,
    pub session: RwLock<Option<Arc<NodeSession>>>,
}

impl NodeEntry {
    pub fn online_session(&self) -> Option<Arc<NodeSession>> {
        self.session.read().clone().filter(|s| !s.cancel.is_cancelled())
    }

    /// Drop the node's control session, if any. Its data connections go with it.
    pub fn disconnect(&self) {
        if let Some(session) = self.session.write().take() {
            session.cancel.cancel();
        }
    }
}

/// Salted SHA-256 of a SOCKS password. These are checked on every connection,
/// so a slow KDF is not an option here; the admin password uses Argon2.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SocksPassword {
    pub salt: String,
    pub hash: String,
}

impl SocksPassword {
    pub fn new(password: &str) -> Self {
        let salt = wirefox_common::random_token(16);
        let hash = hex::encode(Self::digest(&salt, password));
        SocksPassword { salt, hash }
    }

    fn digest(salt: &str, password: &str) -> [u8; 32] {
        wirefox_common::sha256(format!("{salt}{password}").as_bytes())
    }

    pub fn verify(&self, password: &str) -> bool {
        let expected = hex::decode(&self.hash).unwrap_or_default();
        wirefox_common::ct_eq(&Self::digest(&self.salt, password), &expected)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UserCfg {
    pub password: SocksPassword,
    pub enabled: bool,
    /// Nodes this user may exit through. Empty means any node.
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub max_connections: Option<u32>,
    #[serde(default)]
    pub note: Option<String>,
    pub created_at: u64,
}

pub struct UserEntry {
    pub username: String,
    pub cfg: RwLock<UserCfg>,
    pub stats: Arc<Traffic>,
}

pub struct ConnEntry {
    pub id: u64,
    pub user: String,
    pub node: String,
    pub target: String,
    pub client: SocketAddr,
    pub started_at: u64,
    pub up: AtomicU64,
    pub down: AtomicU64,
    pub cancel: CancellationToken,
}

#[derive(Serialize)]
pub struct ConnSnapshot {
    pub id: u64,
    pub user: String,
    pub node: String,
    pub target: String,
    pub client: SocketAddr,
    pub started_at: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

impl ConnEntry {
    pub fn snapshot(&self) -> ConnSnapshot {
        ConnSnapshot {
            id: self.id,
            user: self.user.clone(),
            node: self.node.clone(),
            target: self.target.clone(),
            client: self.client,
            started_at: self.started_at,
            bytes_up: self.up.load(Ordering::Relaxed),
            bytes_down: self.down.load(Ordering::Relaxed),
        }
    }
}

/// Keeps a connection listed in [`Store::conns`] until dropped.
pub struct ConnHandle {
    store: Arc<Store>,
    pub entry: Arc<ConnEntry>,
}

impl Drop for ConnHandle {
    fn drop(&mut self) {
        self.store.conns.lock().remove(&self.entry.id);
    }
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedState {
    nodes: Vec<PersistedNode>,
    users: Vec<PersistedUser>,
}

#[derive(Serialize, Deserialize)]
struct PersistedNode {
    id: String,
    #[serde(flatten)]
    cfg: NodeCfg,
    #[serde(default)]
    bytes_up: u64,
    #[serde(default)]
    bytes_down: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedUser {
    username: String,
    #[serde(flatten)]
    cfg: UserCfg,
    #[serde(default)]
    bytes_up: u64,
    #[serde(default)]
    bytes_down: u64,
}

pub struct Store {
    path: PathBuf,
    pub nodes: RwLock<HashMap<String, Arc<NodeEntry>>>,
    pub users: RwLock<HashMap<String, Arc<UserEntry>>>,
    pub conns: Mutex<HashMap<u64, Arc<ConnEntry>>>,
    next_conn_id: AtomicU64,
    dirty: AtomicBool,
    save_lock: tokio::sync::Mutex<()>,
}

impl Store {
    pub fn load(path: PathBuf) -> Result<Arc<Self>> {
        let state: PersistedState = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => PersistedState::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let nodes = state
            .nodes
            .into_iter()
            .map(|n| {
                let entry = NodeEntry {
                    id: n.id.clone(),
                    cfg: RwLock::new(n.cfg),
                    stats: Arc::new(Traffic::with_totals(n.bytes_up, n.bytes_down)),
                    session: RwLock::new(None),
                };
                (n.id, Arc::new(entry))
            })
            .collect();
        let users = state
            .users
            .into_iter()
            .map(|u| {
                let entry = UserEntry {
                    username: u.username.clone(),
                    cfg: RwLock::new(u.cfg),
                    stats: Arc::new(Traffic::with_totals(u.bytes_up, u.bytes_down)),
                };
                (u.username, Arc::new(entry))
            })
            .collect();
        Ok(Arc::new(Store {
            path,
            nodes: RwLock::new(nodes),
            users: RwLock::new(users),
            conns: Mutex::new(HashMap::new()),
            next_conn_id: AtomicU64::new(1),
            dirty: AtomicBool::new(false),
            save_lock: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn node(&self, id: &str) -> Option<Arc<NodeEntry>> {
        self.nodes.read().get(id).cloned()
    }

    pub fn user(&self, name: &str) -> Option<Arc<UserEntry>> {
        self.users.read().get(name).cloned()
    }

    /// Returns false if a node with this id already exists.
    pub fn insert_node(&self, id: String, cfg: NodeCfg) -> bool {
        let mut nodes = self.nodes.write();
        if nodes.contains_key(&id) {
            return false;
        }
        let entry =
            NodeEntry { id: id.clone(), cfg: RwLock::new(cfg), stats: Default::default(), session: Default::default() };
        nodes.insert(id, Arc::new(entry));
        true
    }

    /// Returns false if a user with this name already exists.
    pub fn insert_user(&self, username: String, cfg: UserCfg) -> bool {
        let mut users = self.users.write();
        if users.contains_key(&username) {
            return false;
        }
        let entry = UserEntry { username: username.clone(), cfg: RwLock::new(cfg), stats: Default::default() };
        users.insert(username, Arc::new(entry));
        true
    }

    /// Check SOCKS credentials. Disabled users fail too.
    pub fn authenticate(&self, username: &str, password: &str) -> Option<Arc<UserEntry>> {
        let Some(user) = self.user(username) else {
            // Hash anyway so unknown and known usernames take the same time.
            SocksPassword::digest("", password);
            return None;
        };
        let ok = {
            let cfg = user.cfg.read();
            cfg.password.verify(password) && cfg.enabled
        };
        ok.then_some(user)
    }

    /// The least loaded online node this user may use.
    pub fn pick_node(&self, user: &UserEntry) -> Option<(Arc<NodeEntry>, Arc<NodeSession>)> {
        let allowed = user.cfg.read();
        let nodes = self.nodes.read();
        let usable = |node: &Arc<NodeEntry>| {
            if !node.cfg.read().enabled {
                return None;
            }
            node.online_session().map(|session| (node.clone(), session))
        };
        let load = |(node, _): &(Arc<NodeEntry>, Arc<NodeSession>)| node.stats.active.load(Ordering::Relaxed);
        if allowed.nodes.is_empty() {
            nodes.values().filter_map(usable).min_by_key(load)
        } else {
            allowed.nodes.iter().filter_map(|id| nodes.get(id)).filter_map(usable).min_by_key(load)
        }
    }

    /// Whether the user may still use this node: both exist and are enabled, and
    /// the node is on the user's list. Checked after registering a connection,
    /// so an admin change can't slip in between.
    pub fn still_allowed(&self, user: &Arc<UserEntry>, node: &Arc<NodeEntry>) -> bool {
        let same_user = self.user(&user.username).is_some_and(|u| Arc::ptr_eq(&u, user));
        let same_node = self.node(&node.id).is_some_and(|n| Arc::ptr_eq(&n, node));
        let user_ok = {
            let cfg = user.cfg.read();
            cfg.enabled && (cfg.nodes.is_empty() || cfg.nodes.contains(&node.id))
        };
        same_user && same_node && user_ok && node.cfg.read().enabled
    }

    pub fn register_conn(self: &Arc<Self>, user: &str, node: &str, target: String, client: SocketAddr) -> ConnHandle {
        let entry = Arc::new(ConnEntry {
            id: self.next_conn_id.fetch_add(1, Ordering::Relaxed),
            user: user.to_string(),
            node: node.to_string(),
            target,
            client,
            started_at: now_secs(),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            cancel: CancellationToken::new(),
        });
        self.conns.lock().insert(entry.id, entry.clone());
        ConnHandle { store: self.clone(), entry }
    }

    /// Cancel every live connection matching `pred`. Returns how many there were.
    pub fn kill_conns(&self, pred: impl Fn(&ConnEntry) -> bool) -> usize {
        let conns = self.conns.lock();
        conns.values().filter(|c| pred(c)).inspect(|c| c.cancel.cancel()).count()
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    fn snapshot(&self) -> PersistedState {
        let mut nodes: Vec<_> = self
            .nodes
            .read()
            .values()
            .map(|n| PersistedNode {
                id: n.id.clone(),
                cfg: n.cfg.read().clone(),
                bytes_up: n.stats.up.load(Ordering::Relaxed),
                bytes_down: n.stats.down.load(Ordering::Relaxed),
            })
            .collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut users: Vec<_> = self
            .users
            .read()
            .values()
            .map(|u| PersistedUser {
                username: u.username.clone(),
                cfg: u.cfg.read().clone(),
                bytes_up: u.stats.up.load(Ordering::Relaxed),
                bytes_down: u.stats.down.load(Ordering::Relaxed),
            })
            .collect();
        users.sort_by(|a, b| a.username.cmp(&b.username));
        PersistedState { nodes, users }
    }

    pub async fn save(&self) -> Result<()> {
        let _saving = self.save_lock.lock().await;
        // Clear first: a change made while we write sets it again.
        self.dirty.store(false, Ordering::Release);
        let json = serde_json::to_vec_pretty(&self.snapshot())?;
        let path = self.path.clone();
        let result = tokio::task::spawn_blocking(move || wirefox_common::files::write_atomic_private(&path, &json))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r.with_context(|| format!("writing {}", self.path.display())));
        if result.is_err() {
            self.mark_dirty();
        }
        result
    }

    /// Save soon after config changes, and periodically for the traffic counters.
    pub async fn autosave(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        let mut last_save = tokio::time::Instant::now();
        loop {
            tick.tick().await;
            if self.dirty.load(Ordering::Acquire) || last_save.elapsed() >= SAVE_INTERVAL {
                last_save = tokio::time::Instant::now();
                if let Err(e) = self.save().await {
                    tracing::error!("saving state failed: {e:#}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_cfg(password: &str) -> UserCfg {
        UserCfg {
            password: SocksPassword::new(password),
            enabled: true,
            nodes: vec![],
            max_connections: None,
            note: None,
            created_at: 0,
        }
    }

    #[test]
    fn socks_password() {
        let p = SocksPassword::new("hunter2");
        assert!(p.verify("hunter2"));
        assert!(!p.verify("hunter3"));
        assert!(!p.verify(""));
        assert_ne!(SocksPassword::new("hunter2").salt, p.salt);
    }

    #[test]
    fn active_limit() {
        let t = Arc::new(Traffic::default());
        let a = t.try_enter(Some(2)).unwrap();
        let _b = t.try_enter(Some(2)).unwrap();
        assert!(t.try_enter(Some(2)).is_none());
        drop(a);
        assert!(t.try_enter(Some(2)).is_some());
        assert!(t.try_enter(None).is_some());
        assert_eq!(t.active.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn authenticate_respects_enabled() {
        let store = Store::load(std::env::temp_dir().join("wirefox-never-written.json")).unwrap();
        assert!(store.insert_user("alice".into(), user_cfg("pw")));
        assert!(!store.insert_user("alice".into(), user_cfg("other")));
        assert!(store.authenticate("alice", "pw").is_some());
        assert!(store.authenticate("alice", "nope").is_none());
        assert!(store.authenticate("bob", "pw").is_none());
        store.user("alice").unwrap().cfg.write().enabled = false;
        assert!(store.authenticate("alice", "pw").is_none());
    }

    #[test]
    fn conn_handle_unregisters() {
        let store = Store::load(std::env::temp_dir().join("wirefox-never-written.json")).unwrap();
        let handle = store.register_conn("alice", "n1", "example.com:443".into(), "127.0.0.1:1".parse().unwrap());
        assert_eq!(store.kill_conns(|c| c.user == "alice"), 1);
        assert!(handle.entry.cancel.is_cancelled());
        drop(handle);
        assert!(store.conns.lock().is_empty());
    }

    #[tokio::test]
    async fn save_and_reload() {
        let path = std::env::temp_dir().join(format!("wirefox-state-{}/state.json", wirefox_common::random_token(8)));
        let store = Store::load(path.clone()).unwrap();
        store.insert_user("alice".into(), user_cfg("pw"));
        store.user("alice").unwrap().stats.up.store(42, Ordering::Relaxed);
        store.save().await.unwrap();

        let reloaded = Store::load(path.clone()).unwrap();
        let alice = reloaded.user("alice").unwrap();
        assert_eq!(alice.stats.up.load(Ordering::Relaxed), 42);
        assert!(reloaded.authenticate("alice", "pw").is_some());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
