//! Where the certified-leaf listeners bind, apart from the main mesh listener.
//!
//! A machine can keep its mesh on `127.0.0.1` and still accept leaves on its
//! LAN address. This only decides *where* sockets bind: who is admitted is
//! still the leaf path's own certificate and signature checks.

use std::net::{IpAddr, SocketAddr};

/// Parsed `leaf_listen_addr` (an IP, or `ip:port`) after the wildcard rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafBindSpec {
    pub ip: IpAddr,
    /// Explicit leaf TCP port; discovery is this port minus one.
    pub port: Option<u16>,
}

/// The sockets the leaf path binds, and what `kernel status` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafBind {
    pub tcp: SocketAddr,
    pub discovery: SocketAddr,
}

impl LeafBindSpec {
    /// Parse `leaf_listen_addr`. A wildcard needs `any`.
    pub fn parse(addr: &str, any: bool) -> Result<Self, String> {
        let addr = addr.trim();
        let (ip, port) = if let Ok(sa) = addr.parse::<SocketAddr>() {
            (sa.ip(), Some(sa.port()))
        } else if let Ok(ip) = addr.trim_matches(['[', ']']).parse::<IpAddr>() {
            (ip, None)
        } else {
            return Err(format!(
                "leaf_listen_addr {addr:?} is neither an IP nor ip:port (example: 192.0.2.10 or 192.0.2.10:9491)"
            ));
        };
        if ip.is_unspecified() && !any {
            return Err(format!(
                "leaf_listen_addr {addr} binds every interface; set leaf_listen_any = true to allow it, \
                 or name the LAN address leaves should reach"
            ));
        }
        if let Some(p) = port
            && p < 2 {
                return Err(format!("leaf_listen_addr port {p} leaves no room for the discovery port below it"));
            }
        Ok(Self { ip, port })
    }

    /// Resolve against the main listener's bound address.
    pub fn resolve(spec: Option<&Self>, mesh: SocketAddr) -> Result<LeafBind, String> {
        let no_room = || "mesh port leaves no room for the leaf ports above it".to_string();
        let (ip, tcp_port) = match spec {
            Some(s) => (s.ip, match s.port {
                Some(p) => p,
                None => mesh.port().checked_add(2).ok_or_else(no_room)?,
            }),
            None => (mesh.ip(), mesh.port().checked_add(2).ok_or_else(no_room)?),
        };
        // Absent or an IP-only spec keeps main+1; an explicit port puts discovery just below it.
        let disc_port = match spec.and_then(|s| s.port) {
            Some(p) => p - 1,
            None => mesh.port() + 1,
        };
        Ok(LeafBind { tcp: SocketAddr::new(ip, tcp_port), discovery: SocketAddr::new(ip, disc_port) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh(s: &str) -> SocketAddr { s.parse().unwrap() }

    #[test]
    fn absent_spec_keeps_the_main_listener_ip_and_offsets() {
        let b = LeafBindSpec::resolve(None, mesh("127.0.0.1:9489")).unwrap();
        assert_eq!(b.tcp, mesh("127.0.0.1:9491"));
        assert_eq!(b.discovery, mesh("127.0.0.1:9490"));
    }

    #[test]
    fn ip_only_uses_main_port_offsets_on_that_ip() {
        let s = LeafBindSpec::parse("192.0.2.10", false).unwrap();
        let b = LeafBindSpec::resolve(Some(&s), mesh("127.0.0.1:9489")).unwrap();
        assert_eq!(b.tcp, mesh("192.0.2.10:9491"));
        assert_eq!(b.discovery, mesh("192.0.2.10:9490"));
    }

    #[test]
    fn ip_and_port_sets_tcp_and_discovery_is_one_below() {
        let s = LeafBindSpec::parse("192.0.2.10:9600", false).unwrap();
        let b = LeafBindSpec::resolve(Some(&s), mesh("127.0.0.1:9489")).unwrap();
        assert_eq!(b.tcp, mesh("192.0.2.10:9600"));
        assert_eq!(b.discovery, mesh("192.0.2.10:9599"));
    }

    #[test]
    fn wildcard_needs_the_opt_in() {
        assert!(LeafBindSpec::parse("0.0.0.0", false).unwrap_err().contains("leaf_listen_any"));
        assert!(LeafBindSpec::parse("[::]:9600", false).is_err());
        assert!(LeafBindSpec::parse("::", false).is_err());
        assert!(LeafBindSpec::parse("0.0.0.0:9600", true).is_ok());
        assert!(LeafBindSpec::parse("::", true).is_ok());
    }

    #[test]
    fn garbage_and_tiny_ports_are_refused() {
        assert!(LeafBindSpec::parse("lan.example", false).is_err());
        assert!(LeafBindSpec::parse("192.0.2.10:1", false).is_err());
    }
}
