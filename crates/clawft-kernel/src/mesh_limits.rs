//! Connection limits for the mesh listener (P3-K1): caps and timeouts
//! applied under a strict admission gate.

use std::sync::Arc;

/// Most concurrent inbound connections served at once.
pub const MAX_CONNECTIONS: usize = 1024;
/// Time a peer gets to finish the Noise handshake.
pub const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Per-IP concurrent connection cap (only under a strict gate).
pub const MAX_CONNECTIONS_PER_IP: usize = 64;
/// Time a peer gets to send its first frame (strict gates only).
pub const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Close an admitted connection idle this long (strict gates only).
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

/// Releases a per-IP slot on drop.
pub(crate) struct IpSlot {
    ip: std::net::IpAddr,
    map: Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>,
}

impl IpSlot {
    pub(crate) fn acquire(
        map: &Arc<std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, usize>>>,
        ip: std::net::IpAddr,
        max: usize,
    ) -> Option<Self> {
        let mut m = map.lock().unwrap();
        let n = m.entry(ip).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(Self { ip, map: Arc::clone(map) })
    }
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut m = self.map.lock().unwrap();
        if let Some(n) = m.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
    }
}


/// Connection limits applied by a strict gate (see [`AdmissionGate::strict`]).
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Silence allowed before the first frame.
    pub first_frame: std::time::Duration,
    /// Silence allowed on an admitted connection.
    pub idle: std::time::Duration,
    /// Concurrent connections per source IP.
    pub per_ip: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            first_frame: FIRST_FRAME_TIMEOUT,
            idle: IDLE_TIMEOUT,
            per_ip: MAX_CONNECTIONS_PER_IP,
        }
    }
}

