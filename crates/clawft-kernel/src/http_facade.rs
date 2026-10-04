//! HTTP REST facade with SSE streaming for WeftOS.
//!
//! Closes Cognitum Seed gaps #6, #7, #8:
//!
//! - **Gap #6**: `GET /events` SSE delta stream from [`KernelEventLog`].
//! - **Gap #7**: `POST /custody/witness` external chain-event injection.
//! - **Gap #8**: REST API facade wrapping kernel Unix-socket RPC methods.
//!
//! # Architecture
//!
//! This module is **transport-agnostic**: it defines typed request/response
//! structs, an SSE event formatter, and a dispatch layer that maps HTTP
//! routes to kernel operations. The actual HTTP server binding (axum, hyper,
//! etc.) is left to the binary crate or `clawft-services`.
//!
//! # Feature Gate
//!
//! Gated behind `http-api`. Not included in the default feature set.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::console::{BootEvent, KernelEventLog, LogLevel};

// ── SSE Event Types (Gap #6) ──────────────────────────────────

/// Known SSE event types streamed on `GET /events`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SseEventType {
    /// Kernel initialized / boot complete.
    Init,
    /// Content ingested into vector store.
    Ingest,
    /// Content deleted from vector store.
    Delete,
    /// Vector store compaction completed.
    Compact,
    /// New event appended to the ExoChain.
    ChainAppend,
    /// Agent process spawned.
    AgentSpawn,
    /// Agent process stopped.
    AgentStop,
    /// Periodic heartbeat (keep-alive).
    Heartbeat,
}

impl SseEventType {
    /// SSE `event:` field value.
    pub fn as_str(&self) -> &'static str {
        match self {
            SseEventType::Init => "init",
            SseEventType::Ingest => "ingest",
            SseEventType::Delete => "delete",
            SseEventType::Compact => "compact",
            SseEventType::ChainAppend => "chain_append",
            SseEventType::AgentSpawn => "agent_spawn",
            SseEventType::AgentStop => "agent_stop",
            SseEventType::Heartbeat => "heartbeat",
        }
    }
}

impl std::fmt::Display for SseEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single SSE message ready for wire formatting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SseMessage {
    /// Event type for the SSE `event:` field.
    pub event_type: SseEventType,
    /// JSON payload for the SSE `data:` field.
    pub data: serde_json::Value,
    /// Wall-clock timestamp.
    pub timestamp: String,
}

impl SseMessage {
    /// Create a new SSE message.
    pub fn new(event_type: SseEventType, data: serde_json::Value) -> Self {
        Self {
            event_type,
            data,
            timestamp: Utc::now().to_rfc3339(),
        }
    }

    /// Format as an SSE wire frame: `event: {type}\ndata: {json}\n\n`.
    pub fn to_sse_frame(&self) -> String {
        let json = serde_json::to_string(&SsePayload {
            data: &self.data,
            timestamp: &self.timestamp,
        })
        .unwrap_or_else(|_| "{}".to_string());
        format!("event: {}\ndata: {}\n\n", self.event_type.as_str(), json)
    }

    /// Create a heartbeat message (empty payload).
    pub fn heartbeat() -> Self {
        Self::new(SseEventType::Heartbeat, serde_json::json!({}))
    }
}

/// Internal helper for SSE data serialization.
#[derive(Serialize)]
struct SsePayload<'a> {
    data: &'a serde_json::Value,
    timestamp: &'a str,
}

/// SSE heartbeat comment frame (`:heartbeat\n\n`).
pub const SSE_HEARTBEAT_COMMENT: &str = ":heartbeat\n\n";

/// Default heartbeat interval in seconds.
pub const SSE_HEARTBEAT_INTERVAL_SECS: u64 = 15;

/// Classify a kernel boot event into an SSE event type.
///
/// Maps event messages to the appropriate SSE event type based on
/// content patterns. Returns `None` for events that should not be
/// streamed (e.g., debug-level noise).
pub fn classify_boot_event(event: &BootEvent) -> Option<SseEventType> {
    let msg = &event.message;
    if msg.contains("[agent]") && msg.contains("spawned") {
        Some(SseEventType::AgentSpawn)
    } else if msg.contains("[agent]") && msg.contains("stopped") {
        Some(SseEventType::AgentStop)
    } else if msg.contains("ingest") || msg.contains("ingested") {
        Some(SseEventType::Ingest)
    } else if msg.contains("delete") || msg.contains("deleted") {
        Some(SseEventType::Delete)
    } else if msg.contains("compact") {
        Some(SseEventType::Compact)
    } else if msg.contains("chain") && msg.contains("append") {
        Some(SseEventType::ChainAppend)
    } else if msg.contains("booting") || msg.contains("ready") {
        Some(SseEventType::Init)
    } else {
        // Generic events still get streamed as init (catch-all).
        // Callers can filter further.
        None
    }
}

/// Snapshot the event log and produce SSE messages for any new events
/// since the given cursor position.
///
/// Returns `(messages, new_cursor)` where `new_cursor` is the total
/// event count after this snapshot. Pass `new_cursor` as the cursor
/// on the next poll to get only fresh events.
pub fn poll_events(event_log: &KernelEventLog, cursor: usize) -> (Vec<SseMessage>, usize) {
    let all = event_log.tail(0);
    let total = all.len();
    if cursor >= total {
        return (Vec::new(), total);
    }
    let new_events = &all[cursor..];
    let messages: Vec<SseMessage> = new_events
        .iter()
        .filter(|e| e.level != LogLevel::Debug)
        .map(|e| {
            let event_type = classify_boot_event(e).unwrap_or(SseEventType::Init);
            SseMessage::new(
                event_type,
                serde_json::json!({
                    "phase": e.phase.tag(),
                    "level": format!("{:?}", e.level).to_lowercase(),
                    "message": e.message,
                }),
            )
        })
        .collect();
    (messages, total)
}

// ── External Witness Injection (Gap #7) ───────────────────────

/// Request body for `POST /custody/witness`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitnessRequest {
    /// Chain event type (e.g., `"audit.external"`, `"attestation"`).
    pub event_type: String,
    /// Arbitrary JSON payload for the chain event.
    pub payload: serde_json::Value,
    /// Hex-encoded Ed25519 signature over `event_type || canonical(payload)`.
    pub signature: String,
}

/// Response body for `POST /custody/witness`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitnessResponse {
    /// Whether the witness event was accepted.
    pub accepted: bool,
    /// Hex-encoded hash of the appended chain event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_hash: Option<String>,
    /// Sequence number of the appended event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// Error message if rejected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl WitnessResponse {
    /// Create a successful response.
    pub fn accepted(hash_hex: String, sequence: u64) -> Self {
        Self {
            accepted: true,
            chain_hash: Some(hash_hex),
            sequence: Some(sequence),
            error: None,
        }
    }

    /// Create a rejection response.
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            accepted: false,
            chain_hash: None,
            sequence: None,
            error: Some(reason.into()),
        }
    }
}

/// Validate a hex-encoded Ed25519 signature.
///
/// Verifies `signature` over `event_type || canonical_json(payload)`
/// against the provided verifying key bytes.
///
/// Returns `Ok(())` on valid signature, `Err(reason)` otherwise.
#[cfg(feature = "exochain")]
pub fn verify_witness_signature(
    event_type: &str,
    payload: &serde_json::Value,
    signature_hex: &str,
    verifying_key_bytes: &[u8; 32],
) -> Result<(), String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let sig_bytes = hex_decode(signature_hex)
        .map_err(|e| format!("invalid signature hex: {e}"))?;
    if sig_bytes.len() != 64 {
        return Err(format!(
            "signature must be 64 bytes, got {}",
            sig_bytes.len()
        ));
    }
    let sig = Signature::from_bytes(
        sig_bytes
            .as_slice()
            .try_into()
            .map_err(|_| "signature length mismatch")?,
    );
    let vk = VerifyingKey::from_bytes(verifying_key_bytes)
        .map_err(|e| format!("invalid verifying key: {e}"))?;

    // Message = event_type bytes || canonical JSON bytes
    let payload_bytes = serde_json::to_vec(payload).unwrap_or_default();
    let mut message = Vec::with_capacity(event_type.len() + payload_bytes.len());
    message.extend_from_slice(event_type.as_bytes());
    message.extend_from_slice(&payload_bytes);

    vk.verify(&message, &sig)
        .map_err(|e| format!("signature verification failed: {e}"))
}

/// Decode a hex string to bytes.
fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err("odd-length hex string".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .map_err(|e| format!("invalid hex at offset {i}: {e}"))
        })
        .collect()
}

// ── REST API Facade (Gap #8) ──────────────────────────────────

/// HTTP method for route matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Delete,
}

impl HttpMethod {
    /// Parse from an HTTP method string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "GET" => Some(HttpMethod::Get),
            "POST" => Some(HttpMethod::Post),
            "DELETE" => Some(HttpMethod::Delete),
            _ => None,
        }
    }
}

/// A matched facade route with its target RPC method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacadeRoute {
    /// SSE event stream.
    Events,
    /// External witness injection.
    CustodyWitness,
    /// Mapped RPC call with method name and optional extracted params.
    Rpc {
        method: &'static str,
        params: Option<RpcParam>,
    },
    /// No match.
    NotFound,
}

/// Extracted route parameters (e.g., `:pid` from `/api/agents/:pid`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcParam {
    /// A PID extracted from the path.
    Pid(u64),
}

/// Match an HTTP method + path to a facade route.
///
/// # Route Table
///
/// | HTTP | Path | RPC Method |
/// |------|------|------------|
/// | GET | `/events` | SSE stream |
/// | POST | `/custody/witness` | witness injection |
/// | GET | `/api/status` | `kernel.status` |
/// | GET | `/api/processes` | `kernel.ps` |
/// | GET | `/api/services` | `kernel.services` |
/// | GET | `/api/chain/status` | `chain.status` |
/// | GET | `/api/chain/events` | `kernel.logs` |
/// | GET | `/api/vectors/status` | `ecc.status` |
/// | POST | `/api/vectors/search` | `ecc.search` |
/// | GET | `/api/ecc/calibration` | `ecc.calibrate` |
/// | GET | `/api/custody/attest` | `custody.attest` |
/// | GET | `/api/fleet/snapshot` | `fleet.snapshot` |
/// | POST | `/api/agents/spawn` | `agent.spawn` |
/// | DELETE | `/api/agents/:pid` | `agent.stop` |
pub fn match_facade_route(method: HttpMethod, path: &str) -> FacadeRoute {
    // Strip query string for matching.
    let path = path.split('?').next().unwrap_or(path);
    // Normalize trailing slash.
    let path = path.trim_end_matches('/');

    match (method, path) {
        // Gap #6: SSE stream
        (HttpMethod::Get, "/events") => FacadeRoute::Events,

        // Gap #7: Witness injection
        (HttpMethod::Post, "/custody/witness") => FacadeRoute::CustodyWitness,

        // Gap #8: REST facade
        (HttpMethod::Get, "/api/status") => FacadeRoute::Rpc {
            method: "kernel.status",
            params: None,
        },
        (HttpMethod::Get, "/api/processes") => FacadeRoute::Rpc {
            method: "kernel.ps",
            params: None,
        },
        (HttpMethod::Get, "/api/services") => FacadeRoute::Rpc {
            method: "kernel.services",
            params: None,
        },
        (HttpMethod::Get, "/api/chain/status") => FacadeRoute::Rpc {
            method: "chain.status",
            params: None,
        },
        (HttpMethod::Get, "/api/chain/events") => FacadeRoute::Rpc {
            method: "kernel.logs",
            params: None,
        },
        (HttpMethod::Get, "/api/vectors/status") => FacadeRoute::Rpc {
            method: "ecc.status",
            params: None,
        },
        (HttpMethod::Post, "/api/vectors/search") => FacadeRoute::Rpc {
            method: "ecc.search",
            params: None,
        },
        (HttpMethod::Get, "/api/ecc/calibration") => FacadeRoute::Rpc {
            method: "ecc.calibrate",
            params: None,
        },
        (HttpMethod::Get, "/api/ecc/coherence") => FacadeRoute::Rpc {
            method: "ecc.coherence",
            params: None,
        },
        (HttpMethod::Get, "/api/custody/attest") => FacadeRoute::Rpc {
            method: "custody.attest",
            params: None,
        },
        // Fleet manager: one read-only snapshot of everything the daemon knows
        // about the fleet (card fleet-p1).
        (HttpMethod::Get, "/api/fleet/snapshot") => FacadeRoute::Rpc {
            method: "fleet.snapshot",
            params: None,
        },
        (HttpMethod::Post, "/api/agents/spawn") => FacadeRoute::Rpc {
            method: "agent.spawn",
            params: None,
        },
        // DELETE /api/agents/:pid
        (HttpMethod::Delete, p) if p.starts_with("/api/agents/") => {
            let pid_str = &p["/api/agents/".len()..];
            match pid_str.parse::<u64>() {
                Ok(pid) => FacadeRoute::Rpc {
                    method: "agent.stop",
                    params: Some(RpcParam::Pid(pid)),
                },
                Err(_) => FacadeRoute::NotFound,
            }
        }

        _ => FacadeRoute::NotFound,
    }
}

/// Build RPC params JSON from the HTTP request body and route params.
///
/// For `GET` requests with query strings, extracts `count` parameter.
/// For `DELETE /api/agents/:pid`, injects the PID into stop params.
/// For `POST` endpoints, passes the body through.
pub fn build_rpc_params(
    route: &FacadeRoute,
    body: &[u8],
    query: &str,
) -> serde_json::Value {
    match route {
        FacadeRoute::Rpc {
            method: "kernel.logs",
            ..
        } => {
            // Extract ?count=N from query string
            let count = parse_query_param(query, "count")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(50);
            serde_json::json!({ "count": count })
        }
        FacadeRoute::Rpc {
            method: "agent.stop",
            params: Some(RpcParam::Pid(pid)),
        } => {
            serde_json::json!({ "pid": pid, "graceful": true })
        }
        FacadeRoute::Rpc { .. } => {
            // Try parsing body as JSON, fall back to empty object
            serde_json::from_slice(body).unwrap_or(serde_json::json!({}))
        }
        _ => serde_json::json!({}),
    }
}

/// Parse a single query-string parameter by name.
fn parse_query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            if k == key { Some(v) } else { None }
        })
}

// ── Facade dispatch result ──────────────────────────────────────

/// HTTP status code + JSON body returned by the facade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FacadeResponse {
    /// HTTP status code (200, 400, 404, 500, etc.).
    pub status: u16,
    /// JSON response body.
    pub body: serde_json::Value,
}

impl FacadeResponse {
    /// 200 OK with JSON body.
    pub fn ok(body: serde_json::Value) -> Self {
        Self { status: 200, body }
    }

    /// 400 Bad Request.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            body: serde_json::json!({ "error": message.into() }),
        }
    }

    /// 404 Not Found.
    pub fn not_found() -> Self {
        Self {
            status: 404,
            body: serde_json::json!({ "error": "not found" }),
        }
    }

    /// 500 Internal Server Error.
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            body: serde_json::json!({ "error": message.into() }),
        }
    }

    /// 401 Unauthorized.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: 401,
            body: serde_json::json!({ "error": message.into() }),
        }
    }
}

// ── Content-Type headers ────────────────────────────────────────

/// Content-Type for SSE responses.
pub const CONTENT_TYPE_SSE: &str = "text/event-stream";

/// Content-Type for JSON responses.
pub const CONTENT_TYPE_JSON: &str = "application/json";

/// Required SSE response headers as `(name, value)` pairs.
pub fn sse_response_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Content-Type", CONTENT_TYPE_SSE),
        ("Cache-Control", "no-cache"),
        ("Connection", "keep-alive"),
        ("X-Accel-Buffering", "no"),
    ]
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::{BootEvent, BootPhase, KernelEventLog};

    // ── SSE formatting tests ────────────────────────────────

    #[test]
    fn sse_message_format() {
        let msg = SseMessage::new(
            SseEventType::AgentSpawn,
            serde_json::json!({"pid": 42, "agent_id": "coder-1"}),
        );
        let frame = msg.to_sse_frame();
        assert!(frame.starts_with("event: agent_spawn\n"));
        assert!(frame.contains("data: "));
        assert!(frame.ends_with("\n\n"));
        // Data should be valid JSON
        let data_line = frame
            .lines()
            .find(|l| l.starts_with("data: "))
            .unwrap();
        let json_str = &data_line["data: ".len()..];
        let parsed: serde_json::Value = serde_json::from_str(json_str).unwrap();
        assert!(parsed.get("data").is_some());
        assert!(parsed.get("timestamp").is_some());
    }

    #[test]
    fn sse_heartbeat_comment() {
        assert_eq!(SSE_HEARTBEAT_COMMENT, ":heartbeat\n\n");
    }

    #[test]
    fn sse_heartbeat_message() {
        let msg = SseMessage::heartbeat();
        assert_eq!(msg.event_type, SseEventType::Heartbeat);
        let frame = msg.to_sse_frame();
        assert!(frame.starts_with("event: heartbeat\n"));
    }

    #[test]
    fn sse_event_type_display() {
        assert_eq!(SseEventType::Init.as_str(), "init");
        assert_eq!(SseEventType::ChainAppend.as_str(), "chain_append");
        assert_eq!(SseEventType::AgentSpawn.as_str(), "agent_spawn");
        assert_eq!(SseEventType::Heartbeat.as_str(), "heartbeat");
    }

    // ── Event classification tests ──────────────────────────

    #[test]
    fn classify_agent_spawn_event() {
        let event = BootEvent::info(BootPhase::Ready, "[agent] spawned coder-1 (PID 5)");
        assert_eq!(classify_boot_event(&event), Some(SseEventType::AgentSpawn));
    }

    #[test]
    fn classify_agent_stop_event() {
        let event = BootEvent::info(BootPhase::Ready, "[agent] stopped PID 5");
        assert_eq!(classify_boot_event(&event), Some(SseEventType::AgentStop));
    }

    #[test]
    fn classify_ingest_event() {
        let event = BootEvent::info(BootPhase::Ready, "content ingested into store");
        assert_eq!(classify_boot_event(&event), Some(SseEventType::Ingest));
    }

    #[test]
    fn classify_compact_event() {
        let event = BootEvent::info(BootPhase::Ready, "vector store compaction complete");
        assert_eq!(classify_boot_event(&event), Some(SseEventType::Compact));
    }

    #[test]
    fn classify_init_event() {
        let event = BootEvent::info(BootPhase::Init, "WeftOS booting...");
        assert_eq!(classify_boot_event(&event), Some(SseEventType::Init));
    }

    #[test]
    fn classify_unknown_event() {
        let event = BootEvent::info(BootPhase::Ready, "some random message");
        assert_eq!(classify_boot_event(&event), None);
    }

    // ── Event polling tests ─────────────────────────────────

    #[test]
    fn poll_events_empty_log() {
        let log = KernelEventLog::new();
        let (messages, cursor) = poll_events(&log, 0);
        assert!(messages.is_empty());
        assert_eq!(cursor, 0);
    }

    #[test]
    fn poll_events_with_cursor() {
        let log = KernelEventLog::new();
        log.info("agent", "spawned coder-1 (PID 5)");
        log.info("test", "second event");

        // First poll: get both
        let (msgs, cursor) = poll_events(&log, 0);
        assert_eq!(msgs.len(), 2);
        assert_eq!(cursor, 2);

        // Second poll with updated cursor: nothing new
        let (msgs, cursor2) = poll_events(&log, cursor);
        assert!(msgs.is_empty());
        assert_eq!(cursor2, 2);

        // Add one more event
        log.info("agent", "stopped PID 5");
        let (msgs, cursor3) = poll_events(&log, cursor2);
        assert_eq!(msgs.len(), 1);
        assert_eq!(cursor3, 3);
    }

    // ── Witness types tests ─────────────────────────────────

    #[test]
    fn witness_response_accepted() {
        let resp = WitnessResponse::accepted("abcd1234".into(), 42);
        assert!(resp.accepted);
        assert_eq!(resp.chain_hash.as_deref(), Some("abcd1234"));
        assert_eq!(resp.sequence, Some(42));
        assert!(resp.error.is_none());
    }

    #[test]
    fn witness_response_rejected() {
        let resp = WitnessResponse::rejected("bad signature");
        assert!(!resp.accepted);
        assert!(resp.chain_hash.is_none());
        assert_eq!(resp.error.as_deref(), Some("bad signature"));
    }

    #[test]
    fn witness_request_serde() {
        let req = WitnessRequest {
            event_type: "audit.external".into(),
            payload: serde_json::json!({"source": "client-A"}),
            signature: "deadbeef".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let restored: WitnessRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.event_type, "audit.external");
        assert_eq!(restored.signature, "deadbeef");
    }

    // ── Hex decode tests ────────────────────────────────────

    #[test]
    fn hex_decode_valid() {
        let bytes = hex_decode("deadbeef").unwrap();
        assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn hex_decode_empty() {
        let bytes = hex_decode("").unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn hex_decode_odd_length() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn hex_decode_invalid_chars() {
        assert!(hex_decode("zzzz").is_err());
    }

    // ── Route matching tests ────────────────────────────────

    #[test]
    fn route_events() {
        assert_eq!(
            match_facade_route(HttpMethod::Get, "/events"),
            FacadeRoute::Events
        );
    }

    #[test]
    fn route_events_with_trailing_slash() {
        assert_eq!(
            match_facade_route(HttpMethod::Get, "/events/"),
            FacadeRoute::Events
        );
    }

    #[test]
    fn route_custody_witness() {
        assert_eq!(
            match_facade_route(HttpMethod::Post, "/custody/witness"),
            FacadeRoute::CustodyWitness
        );
    }

    #[test]
    fn route_api_status() {
        let route = match_facade_route(HttpMethod::Get, "/api/status");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "kernel.status",
                params: None
            }
        );
    }

    #[test]
    fn route_api_processes() {
        let route = match_facade_route(HttpMethod::Get, "/api/processes");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "kernel.ps",
                params: None
            }
        );
    }

    #[test]
    fn route_api_fleet_snapshot_is_get_only() {
        assert_eq!(
            match_facade_route(HttpMethod::Get, "/api/fleet/snapshot"),
            FacadeRoute::Rpc {
                method: "fleet.snapshot",
                params: None
            }
        );
        // A write verb is never reachable through the facade route table.
        assert_eq!(
            match_facade_route(HttpMethod::Post, "/api/fleet/snapshot"),
            FacadeRoute::NotFound
        );
        assert_eq!(
            match_facade_route(HttpMethod::Post, "/api/fleet/location"),
            FacadeRoute::NotFound
        );
    }

    #[test]
    fn route_api_chain_status() {
        let route = match_facade_route(HttpMethod::Get, "/api/chain/status");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "chain.status",
                params: None
            }
        );
    }

    #[test]
    fn route_api_chain_events() {
        let route = match_facade_route(HttpMethod::Get, "/api/chain/events");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "kernel.logs",
                params: None
            }
        );
    }

    #[test]
    fn route_api_chain_events_with_query() {
        let route = match_facade_route(HttpMethod::Get, "/api/chain/events?count=20");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "kernel.logs",
                params: None
            }
        );
    }

    #[test]
    fn route_api_vectors_status() {
        let route = match_facade_route(HttpMethod::Get, "/api/vectors/status");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "ecc.status",
                params: None
            }
        );
    }

    #[test]
    fn route_api_vectors_search() {
        let route = match_facade_route(HttpMethod::Post, "/api/vectors/search");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "ecc.search",
                params: None
            }
        );
    }

    #[test]
    fn route_api_agents_spawn() {
        let route = match_facade_route(HttpMethod::Post, "/api/agents/spawn");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "agent.spawn",
                params: None
            }
        );
    }

    #[test]
    fn route_api_agents_delete() {
        let route = match_facade_route(HttpMethod::Delete, "/api/agents/42");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "agent.stop",
                params: Some(RpcParam::Pid(42))
            }
        );
    }

    #[test]
    fn route_api_agents_delete_invalid_pid() {
        let route = match_facade_route(HttpMethod::Delete, "/api/agents/notanumber");
        assert_eq!(route, FacadeRoute::NotFound);
    }

    #[test]
    fn route_not_found() {
        assert_eq!(
            match_facade_route(HttpMethod::Get, "/nonexistent"),
            FacadeRoute::NotFound
        );
    }

    #[test]
    fn route_wrong_method() {
        // POST to a GET-only route
        assert_eq!(
            match_facade_route(HttpMethod::Post, "/api/status"),
            FacadeRoute::NotFound
        );
    }

    #[test]
    fn route_ecc_calibration() {
        let route = match_facade_route(HttpMethod::Get, "/api/ecc/calibration");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "ecc.calibrate",
                params: None
            }
        );
    }

    #[test]
    fn route_ecc_coherence() {
        let route = match_facade_route(HttpMethod::Get, "/api/ecc/coherence");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "ecc.coherence",
                params: None
            }
        );
    }

    #[test]
    fn route_custody_attest() {
        let route = match_facade_route(HttpMethod::Get, "/api/custody/attest");
        assert_eq!(
            route,
            FacadeRoute::Rpc {
                method: "custody.attest",
                params: None
            }
        );
    }

    // ── RPC params building tests ───────────────────────────

    #[test]
    fn build_params_kernel_logs_with_count() {
        let route = FacadeRoute::Rpc {
            method: "kernel.logs",
            params: None,
        };
        let params = build_rpc_params(&route, b"", "count=25");
        assert_eq!(params["count"], 25);
    }

    #[test]
    fn build_params_kernel_logs_default_count() {
        let route = FacadeRoute::Rpc {
            method: "kernel.logs",
            params: None,
        };
        let params = build_rpc_params(&route, b"", "");
        assert_eq!(params["count"], 50);
    }

    #[test]
    fn build_params_agent_stop() {
        let route = FacadeRoute::Rpc {
            method: "agent.stop",
            params: Some(RpcParam::Pid(99)),
        };
        let params = build_rpc_params(&route, b"", "");
        assert_eq!(params["pid"], 99);
        assert_eq!(params["graceful"], true);
    }

    #[test]
    fn build_params_from_body() {
        let route = FacadeRoute::Rpc {
            method: "agent.spawn",
            params: None,
        };
        let body = br#"{"agent_id":"coder-1"}"#;
        let params = build_rpc_params(&route, body, "");
        assert_eq!(params["agent_id"], "coder-1");
    }

    #[test]
    fn build_params_invalid_body_fallback() {
        let route = FacadeRoute::Rpc {
            method: "agent.spawn",
            params: None,
        };
        let params = build_rpc_params(&route, b"not json", "");
        assert_eq!(params, serde_json::json!({}));
    }

    // ── Query param parsing tests ───────────────────────────

    #[test]
    fn parse_query_param_found() {
        assert_eq!(parse_query_param("count=20&level=info", "count"), Some("20"));
        assert_eq!(parse_query_param("count=20&level=info", "level"), Some("info"));
    }

    #[test]
    fn parse_query_param_not_found() {
        assert_eq!(parse_query_param("count=20", "level"), None);
    }

    #[test]
    fn parse_query_param_empty() {
        assert_eq!(parse_query_param("", "count"), None);
    }

    // ── HttpMethod parse tests ──────────────────────────────

    #[test]
    fn http_method_parse() {
        assert_eq!(HttpMethod::parse("GET"), Some(HttpMethod::Get));
        assert_eq!(HttpMethod::parse("get"), Some(HttpMethod::Get));
        assert_eq!(HttpMethod::parse("POST"), Some(HttpMethod::Post));
        assert_eq!(HttpMethod::parse("DELETE"), Some(HttpMethod::Delete));
        assert_eq!(HttpMethod::parse("PATCH"), None);
    }

    // ── FacadeResponse tests ────────────────────────────────

    #[test]
    fn facade_response_ok() {
        let resp = FacadeResponse::ok(serde_json::json!({"status": "running"}));
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn facade_response_not_found() {
        let resp = FacadeResponse::not_found();
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn facade_response_bad_request() {
        let resp = FacadeResponse::bad_request("missing field");
        assert_eq!(resp.status, 400);
        assert_eq!(resp.body["error"], "missing field");
    }

    #[test]
    fn facade_response_unauthorized() {
        let resp = FacadeResponse::unauthorized("invalid token");
        assert_eq!(resp.status, 401);
    }

    // ── SSE headers test ────────────────────────────────────

    #[test]
    fn sse_headers() {
        let headers = sse_response_headers();
        assert!(headers.iter().any(|(k, v)| *k == "Content-Type" && *v == "text/event-stream"));
        assert!(headers.iter().any(|(k, _)| *k == "Cache-Control"));
    }

    // ── Content type constants ──────────────────────────────

    #[test]
    fn content_types() {
        assert_eq!(CONTENT_TYPE_SSE, "text/event-stream");
        assert_eq!(CONTENT_TYPE_JSON, "application/json");
    }
}
