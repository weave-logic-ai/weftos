//! Core module ABI v1, shared verbatim by guest and native runner.
//! One reactor instance/store for the entire runner lifetime. All frames are
//! UTF-8 JSON, at most MAX_FRAME bytes; no allocation is requested by a guest.
//! exchange(op, input_ptr, input_len, output_ptr, output_capacity) -> length.
//! Negative lengths are fatal transport errors. NEXT returns 0 on an idle tick.
//! Imports never grant a destination/path supplied in a frame. WASI supplies
//! random/clocks and capability-scoped filesystem access, without env/network.
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 1024 * 1024;
pub const BOOT: i32 = 0;
pub const PARENT: i32 = 1;
pub const NEXT: i32 = 2;
pub const REPLY: i32 = 3;
pub const ABI: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boot {
    pub abi: u32,
    pub project_id: String,
    pub root_sha256: String,
    pub user_pubkey: String,
    pub spawn_nonce: Option<String>,
    pub parent_policy: serde_json::Value,
    pub pid: u32,
    pub socket: String,
    pub runtime_dir: String,
    pub artifact_sha256: String,
    pub depth: u32,
    pub parent: String,
}
