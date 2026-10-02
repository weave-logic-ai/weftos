//! Optional UDP forwarder: the ESP32 CSI feed from the node's LAN into a
//! container's feed port. A relay of whole datagrams, nothing parsed.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

/// Largest datagram forwarded (the feed packets are 32 and 48 bytes).
pub const MAX_DATAGRAM: usize = 512;

/// Forwarder settings.
#[derive(Debug, Clone)]
pub struct UdpForwardConfig {
    /// Address to receive the feed on.
    pub listen: SocketAddr,
    /// The container's feed port.
    pub dest: SocketAddr,
    /// Only datagrams from this source address are relayed (the ESP32);
    /// `None` accepts any source.
    pub allowed_source: Option<IpAddr>,
}

/// A running forwarder.
pub struct UdpForwarder {
    local: SocketAddr,
    forwarded: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    task: JoinHandle<()>,
}

impl UdpForwarder {
    /// Bind and start relaying.
    pub async fn spawn(cfg: UdpForwardConfig) -> std::io::Result<Self> {
        let sock = UdpSocket::bind(cfg.listen).await?;
        let local = sock.local_addr()?;
        let out = UdpSocket::bind(if cfg.dest.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).await?;
        let forwarded = Arc::new(AtomicU64::new(0));
        let dropped = Arc::new(AtomicU64::new(0));
        let (f, d) = (forwarded.clone(), dropped.clone());
        let task = tokio::spawn(async move {
            // One byte over the cap, so an oversize datagram is detected.
            let mut buf = [0u8; MAX_DATAGRAM + 1];
            loop {
                let Ok((n, src)) = sock.recv_from(&mut buf).await else {
                    return;
                };
                let source_ok = cfg.allowed_source.is_none_or(|a| a == src.ip());
                if n > MAX_DATAGRAM || n == 0 || !source_ok {
                    d.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if out.send_to(&buf[..n], cfg.dest).await.is_ok() {
                    f.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        Ok(Self {
            local,
            forwarded,
            dropped,
            task,
        })
    }

    /// Address the feed is received on.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Datagrams relayed.
    pub fn forwarded(&self) -> u64 {
        self.forwarded.load(Ordering::Relaxed)
    }

    /// Datagrams dropped (oversize, empty, wrong source).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for UdpForwarder {
    fn drop(&mut self) {
        self.task.abort();
    }
}
