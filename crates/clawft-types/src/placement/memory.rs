//! The `mem.unified` shared-pool rule (ADR-099 section 2; ADR-101 section 2).
//!
//! On Apple silicon the GPU and ANE use the system memory pool. A node that
//! advertises `mem.unified` therefore has **one** pool: GPU weights, KV cache
//! and host memory all draw from it, and accelerator `mem_bytes` attributes
//! (for example `accel.gpu.metal` with `unified = true`) describe that same
//! pool and must not be added to it. A node without `mem.unified` has a host
//! pool (`mem.system`) and a separate device pool (`mem.vram`, summed over
//! devices).
//!
//! Sources, by attribute (bytes): `mem.system.free` (falls back to `total`),
//! `mem.unified.free` (falls back to `mem.system`), `mem.vram.free` per
//! device. Accelerator memory attributes are informational only here, so a
//! discrete GPU advertising both `mem.vram` and `mem_bytes` is counted once.

use super::capability::{AttrValue, Capability};

/// Id marking a shared host/accelerator pool.
pub const MEM_UNIFIED: &str = "mem.unified";
/// Id for host memory.
pub const MEM_SYSTEM: &str = "mem.system";
/// Id for discrete device memory (one per device).
pub const MEM_VRAM: &str = "mem.vram";

/// Memory a workload asks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryDemand {
    /// Host memory (process heap, cog RAM, CPU-side buffers).
    pub host_bytes: u64,
    /// Accelerator memory (weights plus KV budget on a GPU/NPU).
    pub accel_bytes: u64,
}

/// Which pool ran short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPool {
    /// The single shared pool of a `mem.unified` node.
    Unified,
    /// Host memory on a discrete node.
    System,
    /// Device memory on a discrete node.
    Vram,
}

/// A reservation did not fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryShortfall {
    /// Pool that ran short.
    pub pool: MemoryPool,
    /// Bytes asked of that pool.
    pub need: u64,
    /// Bytes free in that pool.
    pub free: u64,
}

/// Free memory on one node, accounted per the unified rule. Reservations
/// subtract from the right pool, so already-placed workloads are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryLedger {
    unified: bool,
    host_free: u64,
    vram_free: u64,
}

fn bytes(cap: &Capability, attr: &str) -> Option<u64> {
    cap.attrs
        .get(attr)
        .and_then(AttrValue::as_f64)
        .map(|f| if f > 0.0 { f as u64 } else { 0 })
}

fn free_of(cap: &Capability) -> u64 {
    bytes(cap, "free")
        .or_else(|| bytes(cap, "total"))
        .unwrap_or(0)
}

impl MemoryLedger {
    /// Build from a node's advertised capabilities.
    pub fn from_capabilities(caps: &[Capability]) -> Self {
        let find = |id: &str| caps.iter().find(|c| c.id.as_str() == id);
        let system = find(MEM_SYSTEM).map(free_of).unwrap_or(0);
        match find(MEM_UNIFIED) {
            Some(u) => Self {
                unified: true,
                host_free: bytes(u, "free")
                    .or_else(|| bytes(u, "total"))
                    .unwrap_or(system),
                vram_free: 0,
            },
            None => Self {
                unified: false,
                host_free: system,
                vram_free: caps
                    .iter()
                    .filter(|c| c.id.as_str() == MEM_VRAM)
                    .map(free_of)
                    .fold(0u64, u64::saturating_add),
            },
        }
    }

    /// True if the node has one shared pool.
    pub fn is_unified(&self) -> bool {
        self.unified
    }

    /// Free host (or shared) bytes.
    pub fn host_free(&self) -> u64 {
        self.host_free
    }

    /// Free device bytes (always 0 on a unified node).
    pub fn vram_free(&self) -> u64 {
        self.vram_free
    }

    /// Check a demand without reserving it.
    pub fn check(&self, d: MemoryDemand) -> Result<(), MemoryShortfall> {
        if self.unified {
            let need = d.host_bytes.saturating_add(d.accel_bytes);
            return if need <= self.host_free {
                Ok(())
            } else {
                Err(MemoryShortfall {
                    pool: MemoryPool::Unified,
                    need,
                    free: self.host_free,
                })
            };
        }
        if d.host_bytes > self.host_free {
            return Err(MemoryShortfall {
                pool: MemoryPool::System,
                need: d.host_bytes,
                free: self.host_free,
            });
        }
        if d.accel_bytes > self.vram_free {
            return Err(MemoryShortfall {
                pool: MemoryPool::Vram,
                need: d.accel_bytes,
                free: self.vram_free,
            });
        }
        Ok(())
    }

    /// Reserve a demand, subtracting it from the right pool(s).
    pub fn reserve(&mut self, d: MemoryDemand) -> Result<(), MemoryShortfall> {
        self.check(d)?;
        if self.unified {
            self.host_free -= d.host_bytes.saturating_add(d.accel_bytes);
        } else {
            self.host_free -= d.host_bytes;
            self.vram_free -= d.accel_bytes;
        }
        Ok(())
    }
}
