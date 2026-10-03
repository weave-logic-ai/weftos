//! Is a managed server reachable beyond loopback? The adapter passes
//! `--host 127.0.0.1`, but the launcher and the server are not ours, so
//! the binding is checked, not assumed: connect to every non-loopback
//! address of this machine on the server's port.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::task::JoinSet;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// Non-loopback unicast addresses of this machine's interfaces (IPv6
/// link-local addresses are skipped: they need a scope to connect to).
pub fn local_addrs() -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we only read and then
    // release with freeifaddrs; every pointer is checked for null first.
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let a = &*cur;
            if !a.ifa_addr.is_null() {
                match i32::from((*a.ifa_addr).sa_family) {
                    libc::AF_INET => {
                        let s = &*(a.ifa_addr as *const libc::sockaddr_in);
                        out.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(s.sin_addr.s_addr))));
                    }
                    libc::AF_INET6 => {
                        let s = &*(a.ifa_addr as *const libc::sockaddr_in6);
                        out.push(IpAddr::V6(Ipv6Addr::from(s.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            cur = a.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out.retain(|ip| {
        !ip.is_loopback()
            && !ip.is_unspecified()
            && !matches!(ip, IpAddr::V6(v) if v.segments()[0] & 0xffc0 == 0xfe80)
    });
    out.sort();
    out.dedup();
    out
}

/// Whether something accepts connections on `ip:port`.
pub async fn port_in_use(ip: IpAddr, port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect(SocketAddr::new(ip, port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// The local non-loopback addresses `port` answers on (empty: loopback only).
pub async fn reachable_beyond_loopback(port: u16) -> Vec<IpAddr> {
    let mut set = JoinSet::new();
    for ip in local_addrs() {
        set.spawn(async move { port_in_use(ip, port).await.then_some(ip) });
    }
    let mut out = Vec::new();
    while let Some(r) = set.join_next().await {
        if let Ok(Some(ip)) = r {
            out.push(ip);
        }
    }
    out.sort();
    out
}
