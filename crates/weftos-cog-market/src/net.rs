//! Address rules shared by the cog host and the console: which peers count as "on the tailnet".

use std::net::IpAddr;

/// True for Tailscale addresses: IPv4 `100.64.0.0/10` (CGNAT range) and IPv6 `fd7a:115c:a1e0::/48`.
/// The mesh fan-out only dials these, and the console only offers to switch to these.
pub fn is_tailnet_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..=127).contains(&o[1])
        }
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// Parse and check in one step.
pub fn tailnet_ip(s: &str) -> Option<IpAddr> {
    s.parse::<IpAddr>().ok().filter(is_tailnet_ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailnet_ranges_only() {
        for ok in ["100.64.0.1", "100.109.125.18", "100.127.255.254", "fd7a:115c:a1e0::1", "fd7a:115c:a1e0:ab12::9"] {
            assert!(tailnet_ip(ok).is_some(), "{ok}");
        }
        for bad in ["100.63.255.255", "100.128.0.1", "10.0.0.1", "192.168.1.5", "127.0.0.1", "8.8.8.8", "fd7a:115c:a1e1::1", "::1", "fe80::1", "not-an-ip", "", "100.64.0.1:9480", "evil.example"] {
            assert!(tailnet_ip(bad).is_none(), "{bad}");
        }
    }
}
