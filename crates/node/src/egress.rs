//! Outbound connections pinned to an interface and/or a source address.
//!
//! * Linux: `SO_BINDTODEVICE`. Kernels before 5.7 only allow it with
//!   `CAP_NET_RAW`; without it we fall back to binding the interface's address.
//! * Windows: `IP_UNICAST_IF` / `IPV6_UNICAST_IF`.
//! * macOS: `IP_BOUND_IF` / `IPV6_BOUND_IF`.

use std::collections::HashSet;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use socket2::{Domain, Protocol, SockRef, Socket, TcpKeepalive, Type};
use tokio::net::{TcpSocket, TcpStream};
use tokio::task::JoinSet;
use tracing::warn;
use wirefox_common::proto::{EgressSpec, Host, InterfaceInfo, Target};

/// Interface addresses can change (DHCP, PPP reconnects), so they're re-read this often.
const PLAN_TTL: Duration = Duration::from_secs(5);
/// Happy Eyeballs (RFC 8305): start the next address if the current one hasn't connected yet.
const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// An [`EgressSpec`] resolved against the current interfaces.
#[derive(Debug, Default)]
struct Plan {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    device: Option<String>,
    #[cfg(any(windows, target_os = "macos", target_os = "ios"))]
    if_index: Option<u32>,
    /// `source_ip` from the spec; always bound.
    source_ip: Option<IpAddr>,
    /// Interface addresses, bound when the socket can't be pinned to the device.
    iface_v4: Option<IpAddr>,
    iface_v6: Option<IpAddr>,
    allow_v4: bool,
    allow_v6: bool,
    /// Every address of this machine, to refuse proxying to ourselves.
    own_addrs: HashSet<IpAddr>,
}

type PlanResult = Result<Arc<Plan>, (io::ErrorKind, String)>;

struct CachedPlan {
    generation: u64,
    built: Instant,
    plan: PlanResult,
}

impl CachedPlan {
    fn get(&self) -> io::Result<Arc<Plan>> {
        self.plan.clone().map_err(|(kind, msg)| io::Error::new(kind, msg))
    }
}

pub struct Dialer {
    local: EgressSpec,
    override_spec: RwLock<Option<EgressSpec>>,
    /// Bumped whenever the effective spec changes, so a slow rebuild for an old
    /// spec can't overwrite the plan of a new one.
    generation: AtomicU64,
    cache: RwLock<CachedPlan>,
    refreshing: Mutex<()>,
    connect_timeout: Duration,
    allow_local: bool,
}

impl Dialer {
    /// Never fails: an interface that isn't up yet only makes dials fail until it is.
    pub fn new(local: EgressSpec, connect_timeout: Duration, allow_local: bool) -> Self {
        let plan = build_plan(&local);
        if let Err(e) = &plan {
            warn!(egress = %local, error = %e, "egress is not usable yet, will keep retrying");
        }
        Dialer {
            local,
            override_spec: RwLock::new(None),
            generation: AtomicU64::new(0),
            cache: RwLock::new(CachedPlan { generation: 0, built: Instant::now(), plan: to_plan_result(plan) }),
            refreshing: Mutex::new(()),
            connect_timeout,
            allow_local,
        }
    }

    pub fn local_spec(&self) -> &EgressSpec {
        &self.local
    }

    pub fn effective(&self) -> EgressSpec {
        self.override_spec.read().clone().unwrap_or_else(|| self.local.clone())
    }

    /// Why the effective spec can't be used right now (say its interface is
    /// gone), or `None` if dials can go out.
    pub fn error(&self) -> Option<String> {
        self.plan().err().map(|e| e.to_string())
    }

    /// Apply a server-pushed override (`None` goes back to the local config).
    /// The spec takes effect even if it can't be resolved yet: dials then fail
    /// until the interface shows up, instead of quietly leaving some other way.
    /// Returns the resolution error, if any, for reporting.
    pub fn set_override(&self, spec: Option<EgressSpec>) -> Option<String> {
        let effective = spec.clone().unwrap_or_else(|| self.local.clone());
        let plan = to_plan_result(build_plan(&effective));
        let error = plan.as_ref().err().map(|(_, msg)| msg.clone());
        let mut cache = self.cache.write();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self.override_spec.write() = spec;
        *cache = CachedPlan { generation, built: Instant::now(), plan };
        error
    }

    fn plan(&self) -> io::Result<Arc<Plan>> {
        let generation = self.generation.load(Ordering::Acquire);
        let stale = {
            let cache = self.cache.read();
            if cache.generation != generation {
                None
            } else if cache.built.elapsed() < PLAN_TTL {
                return cache.get();
            } else {
                cache.plan.clone().ok()
            }
        };
        let _refreshing = match (self.refreshing.try_lock(), stale) {
            (Some(guard), _) => guard,
            // Someone else is already refreshing; the old plan is fine meanwhile.
            (None, Some(plan)) => return Ok(plan),
            (None, None) => {
                let guard = self.refreshing.lock();
                let cache = self.cache.read();
                if cache.generation == generation && cache.built.elapsed() < PLAN_TTL {
                    return cache.get();
                }
                guard
            }
        };
        let plan = to_plan_result(build_plan(&self.effective()));
        let mut cache = self.cache.write();
        if self.generation.load(Ordering::Acquire) == generation {
            *cache = CachedPlan { generation, built: Instant::now(), plan: plan.clone() };
        }
        plan.map_err(|(kind, msg)| io::Error::new(kind, msg))
    }

    pub async fn resolve(&self, target: &Target) -> io::Result<Vec<SocketAddr>> {
        match &target.host {
            Host::Ip(ip) => Ok(vec![SocketAddr::new(*ip, target.port)]),
            Host::Domain(d) => Ok(tokio::net::lookup_host((d.as_str(), target.port)).await?.collect()),
        }
    }

    pub async fn dial(&self, target: &Target) -> io::Result<TcpStream> {
        let deadline = tokio::time::Instant::now() + self.connect_timeout;
        let addrs = tokio::time::timeout_at(deadline, self.resolve(target))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "DNS lookup timed out"))??;
        tokio::time::timeout_at(deadline, self.dial_addrs(addrs))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timed out"))?
    }

    /// Connect to whichever address answers first, racing them Happy Eyeballs style.
    pub async fn dial_addrs(&self, addrs: Vec<SocketAddr>) -> io::Result<TcpStream> {
        let plan = self.plan()?;
        let mut refused = None;
        let mut candidates = Vec::with_capacity(addrs.len());
        for addr in addrs {
            // `::ffff:a.b.c.d` is just the IPv4 host a.b.c.d.
            let addr = SocketAddr::new(addr.ip().to_canonical(), addr.port());
            if (addr.is_ipv4() && !plan.allow_v4) || (addr.is_ipv6() && !plan.allow_v6) {
                continue;
            }
            if !self.allow_local && is_local_target(&plan, addr.ip()) {
                refused = Some(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "connecting to this node or to link-local addresses is not allowed",
                ));
                continue;
            }
            candidates.push(addr);
        }
        if candidates.is_empty() {
            return Err(refused.unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::AddrNotAvailable, "no address of a family the egress can reach")
            }));
        }

        let mut queue = interleave_families(candidates).into_iter();
        let mut attempts = JoinSet::new();
        let mut last_err = None;
        loop {
            if attempts.is_empty() {
                let Some(addr) = queue.next() else { break };
                attempts.spawn(connect_one(addr, plan.clone()));
            }
            tokio::select! {
                Some(joined) = attempts.join_next() => match joined {
                    // Returning drops `attempts`, which aborts the others.
                    Ok(Ok(stream)) => return Ok(stream),
                    Ok(Err(e)) => {
                        last_err = Some(e);
                        if let Some(addr) = queue.next() {
                            attempts.spawn(connect_one(addr, plan.clone()));
                        }
                    }
                    Err(e) => last_err = Some(io::Error::other(e)),
                },
                _ = tokio::time::sleep(ATTEMPT_DELAY), if queue.len() > 0 => {
                    if let Some(addr) = queue.next() {
                        attempts.spawn(connect_one(addr, plan.clone()));
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| io::Error::other("connect failed")))
    }
}

fn to_plan_result(plan: io::Result<Plan>) -> PlanResult {
    plan.map(Arc::new).map_err(|e| (e.kind(), e.to_string()))
}

/// The node itself, plus link-local destinations: those include the cloud
/// metadata services (169.254.169.254, fd00:ec2::254), which hand out the
/// machine's credentials.
fn is_local_target(plan: &Plan, ip: IpAddr) -> bool {
    const AWS_METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);
    let link_local = match ip {
        IpAddr::V4(v4) => v4.is_link_local() || v4.octets()[0] == 0,
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80 || v6 == AWS_METADATA_V6,
    };
    link_local || ip.is_loopback() || ip.is_unspecified() || plan.own_addrs.contains(&ip)
}

/// Alternate address families, keeping the resolver's order within each family.
fn interleave_families(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let first_is_v6 = addrs.first().is_some_and(SocketAddr::is_ipv6);
    let (primary, secondary): (Vec<_>, Vec<_>) = addrs.into_iter().partition(|a| a.is_ipv6() == first_is_v6);
    let mut out = Vec::with_capacity(primary.len() + secondary.len());
    let (mut primary, mut secondary) = (primary.into_iter(), secondary.into_iter());
    loop {
        match (primary.next(), secondary.next()) {
            (None, None) => return out,
            (a, b) => out.extend(a.into_iter().chain(b)),
        }
    }
}

async fn connect_one(addr: SocketAddr, plan: Arc<Plan>) -> io::Result<TcpStream> {
    let sock = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    sock.set_nonblocking(true)?;
    sock.set_tcp_nodelay(true)?;
    let ka = TcpKeepalive::new().with_time(Duration::from_secs(60)).with_interval(Duration::from_secs(10));
    SockRef::from(&sock).set_tcp_keepalive(&ka)?;

    let pinned = bind_to_interface(&sock, addr, &plan)?;
    let source = match plan.source_ip {
        Some(ip) => Some(ip),
        // A socket pinned to its interface gets the right source address from the kernel.
        None if pinned => None,
        None if addr.is_ipv4() => plan.iface_v4,
        None => plan.iface_v6,
    };
    if let Some(ip) = source.filter(|ip| ip.is_ipv4() == addr.is_ipv4()) {
        sock.bind(&SocketAddr::new(ip, 0).into())?;
    }
    TcpSocket::from_std_stream(sock.into()).connect(addr).await
}

/// Pin the socket to the planned interface. Returns whether that worked.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn bind_to_interface(sock: &Socket, _addr: SocketAddr, plan: &Plan) -> io::Result<bool> {
    use std::sync::atomic::AtomicBool;
    static WARNED: AtomicBool = AtomicBool::new(false);

    let Some(dev) = &plan.device else { return Ok(false) };
    match sock.bind_device(Some(dev.as_bytes())) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            if !WARNED.swap(true, Ordering::Relaxed) {
                warn!(
                    interface = %dev,
                    "SO_BINDTODEVICE not permitted (kernels before 5.7 need CAP_NET_RAW); binding the \
                     interface address instead, which only routes correctly with matching policy routing"
                );
            }
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

#[cfg(windows)]
fn bind_to_interface(sock: &Socket, addr: SocketAddr, plan: &Plan) -> io::Result<bool> {
    let Some(index) = plan.if_index else { return Ok(false) };
    win::set_unicast_if(sock, addr.is_ipv4(), index)?;
    Ok(true)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn bind_to_interface(sock: &Socket, addr: SocketAddr, plan: &Plan) -> io::Result<bool> {
    let Some(index) = plan.if_index.and_then(std::num::NonZeroU32::new) else { return Ok(false) };
    if addr.is_ipv4() {
        sock.bind_device_by_index_v4(Some(index))?;
    } else {
        sock.bind_device_by_index_v6(Some(index))?;
    }
    Ok(true)
}

#[cfg(not(any(target_os = "linux", target_os = "android", windows, target_os = "macos", target_os = "ios")))]
fn bind_to_interface(_sock: &Socket, _addr: SocketAddr, _plan: &Plan) -> io::Result<bool> {
    Ok(false)
}

fn build_plan(spec: &EgressSpec) -> io::Result<Plan> {
    let all = if_addrs::get_if_addrs().unwrap_or_default();
    let mut plan = Plan {
        source_ip: spec.source_ip,
        allow_v4: true,
        allow_v6: true,
        own_addrs: all.iter().map(|i| i.ip().to_canonical()).collect(),
        ..Default::default()
    };

    if let Some(name) = &spec.interface {
        let matching: Vec<_> = all.iter().filter(|i| iface_matches(i, name)).collect();
        if matching.is_empty() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("interface {name:?} not found")));
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            plan.device = Some(matching[0].name.clone());
        }
        #[cfg(any(windows, target_os = "macos", target_os = "ios"))]
        {
            plan.if_index = matching.iter().find_map(|i| i.index);
        }

        // Link-local addresses can't reach routed destinations. Prefer global
        // IPv6 over ULA, but keep ULA: the egress may well be a private network.
        let usable = || matching.iter().filter(|i| !i.is_link_local()).map(|i| i.ip());
        plan.iface_v4 = usable().find(IpAddr::is_ipv4);
        plan.iface_v6 = usable().filter(IpAddr::is_ipv6).min_by_key(is_unique_local);
        plan.allow_v4 = plan.iface_v4.is_some();
        plan.allow_v6 = plan.iface_v6.is_some();
        if !plan.allow_v4 && !plan.allow_v6 {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                format!("interface {name:?} has no usable address"),
            ));
        }
        if let Some(ip) = spec.source_ip {
            if !matching.iter().any(|i| i.ip() == ip) {
                return Err(io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    format!("source_ip {ip} is not assigned to interface {name:?}"),
                ));
            }
        }
    }

    if let Some(ip) = spec.source_ip {
        plan.allow_v4 = ip.is_ipv4();
        plan.allow_v6 = ip.is_ipv6();
    }
    Ok(plan)
}

fn is_unique_local(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00)
}

fn iface_matches(i: &if_addrs::Interface, name: &str) -> bool {
    #[cfg(windows)]
    {
        i.name.eq_ignore_ascii_case(name) || i.adapter_name.eq_ignore_ascii_case(name)
    }
    #[cfg(not(windows))]
    {
        i.name == name
    }
}

/// Local interfaces grouped by name, as reported to the server.
pub fn list_interfaces() -> Vec<InterfaceInfo> {
    let mut out: Vec<InterfaceInfo> = Vec::new();
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        if let Some(existing) = out.iter_mut().find(|e| e.name == i.name) {
            existing.addrs.push(i.ip());
            continue;
        }
        #[cfg(windows)]
        let adapter_name = Some(i.adapter_name.clone());
        #[cfg(not(windows))]
        let adapter_name = None;
        out.push(InterfaceInfo {
            name: i.name.clone(),
            adapter_name,
            index: i.index,
            up: i.is_oper_up(),
            addrs: vec![i.ip()],
        });
    }
    out
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::io::AsRawSocket;

    use socket2::Socket;
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, WSAGetLastError, IPPROTO_IP, IPPROTO_IPV6, IPV6_UNICAST_IF, IP_UNICAST_IF, SOCKET_ERROR,
    };

    pub fn set_unicast_if(sock: &Socket, v4: bool, index: u32) -> io::Result<()> {
        // IPv4 wants the index in network byte order, IPv6 in host order.
        let (level, opt, value) =
            if v4 { (IPPROTO_IP, IP_UNICAST_IF, index.to_be()) } else { (IPPROTO_IPV6, IPV6_UNICAST_IF, index) };
        // SAFETY: `value` outlives the call and `optlen` matches its size.
        let rc = unsafe {
            setsockopt(
                sock.as_raw_socket() as _,
                level,
                opt,
                (&value as *const u32).cast(),
                std::mem::size_of::<u32>() as i32,
            )
        };
        if rc == SOCKET_ERROR {
            Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn interleaves_families() {
        let input = vec![addr("[::1]:1"), addr("[::2]:1"), addr("1.1.1.1:1"), addr("[::3]:1"), addr("2.2.2.2:1")];
        let expected = vec![addr("[::1]:1"), addr("1.1.1.1:1"), addr("[::2]:1"), addr("2.2.2.2:1"), addr("[::3]:1")];
        assert_eq!(interleave_families(input), expected);
        assert!(interleave_families(vec![]).is_empty());
    }

    #[test]
    fn unknown_interface_is_an_error() {
        let spec = EgressSpec { interface: Some("does-not-exist0".into()), source_ip: None };
        assert_eq!(build_plan(&spec).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn ula_detection() {
        assert!(is_unique_local(&"fd00::1".parse().unwrap()));
        assert!(!is_unique_local(&"2001:db8::1".parse().unwrap()));
        assert!(!is_unique_local(&"10.0.0.1".parse().unwrap()));
    }

    #[tokio::test]
    async fn refuses_own_addresses_including_mapped_loopback() {
        let dialer = Dialer::new(EgressSpec::default(), Duration::from_secs(2), false);
        for target in
            ["127.0.0.1:9", "[::ffff:127.0.0.1]:9", "0.0.0.0:9", "[::1]:9", "169.254.169.254:80", "[fe80::1]:80"]
        {
            let err = dialer.dial_addrs(vec![addr(target)]).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{target}");
        }
    }

    #[tokio::test]
    async fn falls_back_to_next_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        let dialer = Dialer::new(EgressSpec::default(), Duration::from_secs(2), true);
        // Port 1 on loopback refuses, so the dialer has to move on to the listener.
        let stream = dialer.dial_addrs(vec![addr("127.0.0.1:1"), target]).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap(), target);
    }

    #[tokio::test]
    async fn unresolvable_override_fails_closed() {
        let dialer = Dialer::new(EgressSpec::default(), Duration::from_secs(2), true);
        assert_eq!(dialer.error(), None);
        let missing = EgressSpec { interface: Some("does-not-exist0".into()), source_ip: None };
        assert!(dialer.set_override(Some(missing.clone())).is_some());
        assert_eq!(dialer.effective(), missing);
        assert!(dialer.error().unwrap().contains("does-not-exist0"));
        let err = dialer.dial_addrs(vec![addr("127.0.0.1:9")]).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        let src = EgressSpec { interface: None, source_ip: Some("127.0.0.1".parse().unwrap()) };
        assert_eq!(dialer.set_override(Some(src.clone())), None);
        assert_eq!(dialer.effective(), src);
        assert_eq!(dialer.set_override(None), None);
        assert!(dialer.effective().is_default());
        assert_eq!(dialer.error(), None);
    }
}
