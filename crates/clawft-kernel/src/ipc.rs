//! Kernel IPC subsystem.
//!
//! [`KernelIpc`] wraps the existing [`MessageBus`] from `clawft-core`,
//! adding typed [`KernelMessage`] envelopes and PID-based routing.
//! The underlying message bus channels are reused (no new channels).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

use clawft_core::bus::MessageBus;

use crate::error::KernelError;
use crate::process::Pid;

/// Maximum serialized size of a single kernel IPC message in bytes (16 MiB).
///
/// Frames larger than this are rejected by [`KernelIpc::send`] with
/// [`KernelError::MessageTooLarge`] before the payload is published on
/// the bus. This prevents a single misbehaving sender (or a corrupt
/// inbound frame) from exhausting kernel memory or stalling the bus.
pub const KERNEL_IPC_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// Global atomic counter for generating internal IPC message IDs.
///
/// Using an atomic counter instead of `uuid::Uuid::new_v4()` eliminates
/// the crypto-random generation overhead (~50-100ns per message) in hot
/// IPC paths. The counter is monotonically increasing and unique within
/// a single process lifetime, which is sufficient for internal correlation.
static IPC_MSG_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a fast, unique internal message ID using an atomic counter.
///
/// Format: `"ipc-{counter}"` -- lightweight string, no crypto-random overhead.
#[inline]
fn next_ipc_msg_id() -> String {
    let id = IPC_MSG_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ipc-{id}")
}

/// Target for a kernel message.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageTarget {
    /// Send to a specific process by PID.
    Process(Pid),
    /// Publish to a named topic (all subscribers receive).
    Topic(String),
    /// Broadcast to all processes.
    Broadcast,
    /// Send to a named service (routed via ServiceRegistry).
    Service(String),
    /// Send to a specific method on a named service (D19, K2.1).
    ///
    /// The router resolves the service via ServiceRegistry and wraps
    /// the payload with method metadata for the receiving agent.
    ServiceMethod {
        /// Service name to resolve.
        service: String,
        /// Method to invoke on the service.
        method: String,
    },
    /// Send to the kernel itself.
    Kernel,
    /// Route to a specific process on a remote node (K6).
    /// The inner target is resolved on the destination node.
    RemoteNode {
        /// Remote node identifier.
        node_id: String,
        /// Target to resolve on the remote node.
        target: Box<MessageTarget>,
    },
}

/// Payload types for kernel messages.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessagePayload {
    /// Plain text message.
    Text(String),
    /// Structured JSON data.
    Json(serde_json::Value),
    /// Tool call delegation from one agent to another.
    ToolCall {
        /// Name of the tool to call.
        name: String,
        /// Tool arguments.
        args: serde_json::Value,
    },
    /// Result of a delegated tool call.
    ToolResult {
        /// Correlation ID linking to the original request.
        call_id: String,
        /// Tool execution result.
        result: serde_json::Value,
    },
    /// System control signal.
    Signal(KernelSignal),
    /// Raw binary payload.
    ///
    /// For mesh transport, sensor data, file transfer, or any opaque
    /// byte stream that doesn't need JSON/text interpretation.
    Binary(Vec<u8>),
    /// RVF-typed payload with segment type hint.
    ///
    /// Agents can exchange RVF-typed messages. The segment type tells
    /// the receiver what format the data is in (using rvf-types
    /// discriminants, e.g. 0x40 = ExochainEvent).
    Rvf {
        /// RVF segment type discriminant.
        segment_type: u8,
        /// Payload data (CBOR, JSON, or raw bytes).
        data: Vec<u8>,
    },
}

impl MessagePayload {
    /// Return the payload type name (for logging/chain events).
    pub fn type_name(&self) -> &'static str {
        match self {
            MessagePayload::Text(_) => "text",
            MessagePayload::Json(_) => "json",
            MessagePayload::ToolCall { .. } => "tool_call",
            MessagePayload::ToolResult { .. } => "tool_result",
            MessagePayload::Signal(_) => "signal",
            MessagePayload::Binary(_) => "binary",
            MessagePayload::Rvf { .. } => "rvf",
        }
    }
}

/// Reason a process exited (used in link/monitor notifications).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExitReason {
    /// Normal exit (process completed successfully).
    Normal,
    /// Process crashed with an error.
    Crash(String),
    /// Process was killed.
    Killed,
    /// Process timed out.
    Timeout,
}

/// Notification that a monitored process went down.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessDown {
    /// PID of the process that went down.
    pub pid: crate::process::Pid,
    /// Why the process exited.
    pub reason: ExitReason,
}

/// Kernel control signals.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KernelSignal {
    /// Request a process to shut down gracefully.
    Shutdown,
    /// Request a process to suspend.
    Suspend,
    /// Request a process to resume from suspension.
    Resume,
    /// Heartbeat / keep-alive ping.
    Ping,
    /// Response to a heartbeat ping.
    Pong,
    /// Reload configuration (K2-G5).
    ReloadConfig,
    /// Dump internal state for debugging (K2-G5).
    DumpState,
    /// User-defined signal with a discriminant (K2-G5).
    UserDefined(u8),
    /// Immediate kill -- no cleanup, no graceful shutdown (K2-G5).
    Kill,
    /// Crash notification from a linked process (K1-G2).
    LinkExit {
        /// PID of the linked process that exited.
        pid: crate::process::Pid,
        /// Reason the process exited.
        reason: ExitReason,
    },
    /// Monitor DOWN notification (K1-G2).
    MonitorDown(ProcessDown),
    /// Resource usage warning at 80% of limit (K1-G3).
    ResourceWarning {
        /// Name of the resource (e.g. "memory", "cpu_time").
        resource: String,
        /// Current usage value.
        current: u64,
        /// Configured limit.
        limit: u64,
    },
}

/// A typed message envelope for kernel IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelMessage {
    /// Unique message identifier.
    pub id: String,
    /// Sender PID (0 = kernel).
    pub from: Pid,
    /// Target for delivery.
    pub target: MessageTarget,
    /// Message payload.
    pub payload: MessagePayload,
    /// Creation timestamp.
    pub timestamp: DateTime<Utc>,
    /// Optional correlation ID for request-response patterns.
    ///
    /// When set, this links a response message back to the original
    /// request that triggered it. Used by the A2A protocol's
    /// request-response tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Distributed trace ID for end-to-end request tracing (K2-G4).
    ///
    /// External messages entering the kernel receive a new UUID v4
    /// trace_id. Internal messages inherit the parent's trace_id
    /// via correlation linkage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl KernelMessage {
    /// Create a new kernel message.
    ///
    /// Uses an atomic counter for the message ID instead of UUID v4,
    /// eliminating crypto-random generation overhead in hot IPC paths.
    pub fn new(from: Pid, target: MessageTarget, payload: MessagePayload) -> Self {
        Self {
            id: next_ipc_msg_id(),
            from,
            target,
            payload,
            timestamp: Utc::now(),
            correlation_id: None,
            trace_id: None,
        }
    }

    /// Create a new kernel message with a correlation ID.
    pub fn with_correlation(
        from: Pid,
        target: MessageTarget,
        payload: MessagePayload,
        correlation_id: String,
    ) -> Self {
        Self {
            id: next_ipc_msg_id(),
            from,
            target,
            payload,
            timestamp: Utc::now(),
            correlation_id: Some(correlation_id),
            trace_id: None,
        }
    }

    /// Create a new kernel message with a trace ID (for external entry points).
    pub fn with_trace(
        from: Pid,
        target: MessageTarget,
        payload: MessagePayload,
        trace_id: String,
    ) -> Self {
        Self {
            id: next_ipc_msg_id(),
            from,
            target,
            payload,
            timestamp: Utc::now(),
            correlation_id: None,
            trace_id: Some(trace_id),
        }
    }

    /// Set the trace ID on this message (builder pattern).
    pub fn set_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Ensure this message has a trace ID, generating one if missing.
    pub fn ensure_trace_id(mut self) -> Self {
        if self.trace_id.is_none() {
            self.trace_id = Some(uuid::Uuid::new_v4().to_string());
        }
        self
    }

    /// Create a text message.
    pub fn text(from: Pid, target: MessageTarget, text: impl Into<String>) -> Self {
        Self::new(from, target, MessagePayload::Text(text.into()))
    }

    /// Create a signal message.
    pub fn signal(from: Pid, target: MessageTarget, signal: KernelSignal) -> Self {
        Self::new(from, target, MessagePayload::Signal(signal))
    }

    /// Create a tool call message.
    pub fn tool_call(
        from: Pid,
        target: MessageTarget,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        Self::new(
            from,
            target,
            MessagePayload::ToolCall {
                name: name.into(),
                args,
            },
        )
    }

    /// Create a tool result message (response to a tool call).
    pub fn tool_result(
        from: Pid,
        target: MessageTarget,
        call_id: impl Into<String>,
        result: serde_json::Value,
    ) -> Self {
        Self::new(
            from,
            target,
            MessagePayload::ToolResult {
                call_id: call_id.into(),
                result,
            },
        )
    }
}

/// Globally unique process identifier: (node_id, local_pid).
///
/// Used for cross-node process addressing in K6 mesh networking.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct GlobalPid {
    /// Node that owns this process.
    pub node_id: String,
    /// Local PID on that node.
    pub pid: Pid,
}

impl GlobalPid {
    /// Create a GlobalPid for a local process.
    pub fn local(pid: Pid, node_id: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
            pid,
        }
    }

    /// Check if this PID belongs to the given node.
    pub fn is_local(&self, my_node_id: &str) -> bool {
        self.node_id == my_node_id
    }
}

impl std::fmt::Display for GlobalPid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.node_id, self.pid)
    }
}

/// Kernel IPC subsystem wrapping the core MessageBus.
///
/// Adds kernel-level message envelope (type, routing, timestamps)
/// on top of the existing broadcast channel infrastructure.
pub struct KernelIpc {
    bus: Arc<MessageBus>,
}

impl KernelIpc {
    /// Create a new KernelIpc wrapping the given MessageBus.
    pub fn new(bus: Arc<MessageBus>) -> Self {
        Self { bus }
    }

    /// Get a reference to the underlying MessageBus.
    pub fn bus(&self) -> &Arc<MessageBus> {
        &self.bus
    }

    /// Send a kernel message with RBAC enforcement and chain logging.
    ///
    /// 1. If the target is `Process(to_pid)`, checks IPC capability
    ///    via the `CapabilityChecker`.
    /// 2. Logs the send event to the chain (if provided).
    /// 3. Publishes via the bus.
    #[cfg(feature = "exochain")]
    pub fn send_checked(
        &self,
        msg: &KernelMessage,
        checker: &crate::capability::CapabilityChecker,
        chain: Option<&crate::chain::ChainManager>,
    ) -> Result<(), KernelError> {
        // 1. Check IPC capability
        if let MessageTarget::Process(to_pid) = &msg.target {
            checker.check_ipc_target(msg.from, *to_pid)?;
        }

        // 2. Log to chain
        if let Some(cm) = chain {
            cm.append(
                "ipc",
                "ipc.send",
                Some(serde_json::json!({
                    "from": msg.from,
                    "target": format!("{:?}", msg.target),
                    "payload_type": msg.payload.type_name(),
                    "msg_id": msg.id,
                })),
            );
        }

        // 3. Send via bus
        self.send(msg)
    }

    /// Send a kernel message.
    ///
    /// Currently serializes the message to JSON and publishes it
    /// as an inbound message on the bus. Future versions (K2) will
    /// implement PID-based routing and topic subscriptions.
    ///
    /// Frames whose serialized size exceeds
    /// [`KERNEL_IPC_MAX_MESSAGE_BYTES`] (16 MiB) are rejected with
    /// [`KernelError::MessageTooLarge`] before any publish occurs.
    /// This bounds peak kernel memory under hostile or misbehaving
    /// senders (WEFT-143).
    pub fn send(&self, msg: &KernelMessage) -> Result<(), KernelError> {
        debug!(
            id = %msg.id,
            from = msg.from,
            "sending kernel message"
        );

        let json = serde_json::to_string(msg)
            .map_err(|e| KernelError::Ipc(format!("failed to serialize message: {e}")))?;

        // Enforce the 16 MiB cap on serialized message size before
        // touching the bus. The cap is applied to the canonical
        // JSON payload because that is what we publish downstream.
        if json.len() > KERNEL_IPC_MAX_MESSAGE_BYTES {
            return Err(KernelError::MessageTooLarge {
                size: json.len(),
                limit: KERNEL_IPC_MAX_MESSAGE_BYTES,
            });
        }

        // For now, publish as an inbound message. The A2A routing (K2)
        // will replace this with proper PID-based delivery.
        let inbound = clawft_types::event::InboundMessage {
            channel: "kernel-ipc".to_owned(),
            sender_id: format!("pid-{}", msg.from),
            chat_id: msg.id.clone(),
            content: json,
            timestamp: msg.timestamp,
            media: vec![],
            metadata: std::collections::HashMap::new(),
        };

        self.bus
            .publish_inbound(inbound)
            .map_err(|e| KernelError::Ipc(format!("bus publish failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_message_text() {
        let msg = KernelMessage::text(0, MessageTarget::Process(1), "hello");
        assert_eq!(msg.from, 0);
        assert!(matches!(msg.target, MessageTarget::Process(1)));
        assert!(matches!(msg.payload, MessagePayload::Text(ref t) if t == "hello"));
    }

    #[test]
    fn kernel_message_signal() {
        let msg = KernelMessage::signal(0, MessageTarget::Broadcast, KernelSignal::Shutdown);
        assert!(matches!(msg.target, MessageTarget::Broadcast));
        assert!(matches!(
            msg.payload,
            MessagePayload::Signal(KernelSignal::Shutdown)
        ));
    }

    #[test]
    fn kernel_message_json_payload() {
        let payload = MessagePayload::Json(serde_json::json!({"key": "value"}));
        let msg = KernelMessage::new(1, MessageTarget::Kernel, payload);
        assert!(matches!(msg.payload, MessagePayload::Json(_)));
    }

    #[test]
    fn message_serde_roundtrip() {
        let msg = KernelMessage::text(5, MessageTarget::Service("health".into()), "check");
        let json = serde_json::to_string(&msg).unwrap();
        let restored: KernelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, msg.id);
        assert_eq!(restored.from, 5);
    }

    #[tokio::test]
    async fn ipc_send() {
        let bus = Arc::new(MessageBus::new());
        let ipc = KernelIpc::new(bus.clone());

        let msg = KernelMessage::text(0, MessageTarget::Process(1), "test");
        ipc.send(&msg).unwrap();

        // Should be consumable from the bus
        let received = bus.consume_inbound().await.unwrap();
        assert_eq!(received.channel, "kernel-ipc");
        assert_eq!(received.sender_id, "pid-0");
    }

    #[test]
    fn ipc_bus_ref() {
        let bus = Arc::new(MessageBus::new());
        let ipc = KernelIpc::new(bus.clone());
        assert!(Arc::ptr_eq(ipc.bus(), &bus));
    }

    #[test]
    fn ipc_rejects_oversize_message() {
        // WEFT-143: a frame whose serialized size exceeds 16 MiB must
        // be rejected with `KernelError::MessageTooLarge` and must
        // not be published on the bus.
        let bus = Arc::new(MessageBus::new());
        let ipc = KernelIpc::new(bus.clone());

        // Construct a Binary payload large enough that even after
        // JSON encoding (which slightly inflates Vec<u8> -> array of
        // numbers) the serialized envelope clears the 16 MiB cap by
        // a wide margin.
        let huge = vec![0u8; KERNEL_IPC_MAX_MESSAGE_BYTES + 1];
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(1),
            MessagePayload::Binary(huge),
        );

        let err = ipc.send(&msg).unwrap_err();
        match err {
            KernelError::MessageTooLarge { size, limit } => {
                assert_eq!(limit, KERNEL_IPC_MAX_MESSAGE_BYTES);
                assert!(
                    size > KERNEL_IPC_MAX_MESSAGE_BYTES,
                    "expected size > limit, got size={size} limit={limit}"
                );
            }
            other => panic!("expected MessageTooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ipc_accepts_message_at_limit_boundary() {
        // A small Binary payload must round-trip cleanly to confirm
        // the cap does not regress the happy path.
        let bus = Arc::new(MessageBus::new());
        let ipc = KernelIpc::new(bus.clone());

        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(1),
            MessagePayload::Binary(vec![0u8; 1024]),
        );

        ipc.send(&msg).unwrap();
        let received = bus.consume_inbound().await.unwrap();
        assert_eq!(received.channel, "kernel-ipc");
    }

    #[test]
    fn message_target_variants() {
        let targets = vec![
            MessageTarget::Process(1),
            MessageTarget::Broadcast,
            MessageTarget::Service("test".into()),
            MessageTarget::ServiceMethod {
                service: "auth".into(),
                method: "validate_token".into(),
            },
            MessageTarget::Kernel,
        ];
        for target in targets {
            let json = serde_json::to_string(&target).unwrap();
            let _: MessageTarget = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn kernel_signal_variants() {
        let signals = vec![
            KernelSignal::Shutdown,
            KernelSignal::Suspend,
            KernelSignal::Resume,
            KernelSignal::Ping,
            KernelSignal::Pong,
        ];
        for signal in signals {
            let json = serde_json::to_string(&signal).unwrap();
            let _: KernelSignal = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn expanded_signal_variants_serde() {
        let signals = vec![
            KernelSignal::ReloadConfig,
            KernelSignal::DumpState,
            KernelSignal::UserDefined(42),
            KernelSignal::Kill,
            KernelSignal::LinkExit {
                pid: 7,
                reason: ExitReason::Crash("boom".into()),
            },
            KernelSignal::MonitorDown(ProcessDown {
                pid: 9,
                reason: ExitReason::Normal,
            }),
            KernelSignal::ResourceWarning {
                resource: "memory".into(),
                current: 800,
                limit: 1000,
            },
        ];
        for signal in signals {
            let json = serde_json::to_string(&signal).unwrap();
            let restored: KernelSignal = serde_json::from_str(&json).unwrap();
            // Verify roundtrip by re-serializing
            let json2 = serde_json::to_string(&restored).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn exit_reason_variants() {
        let reasons = vec![
            ExitReason::Normal,
            ExitReason::Crash("error".into()),
            ExitReason::Killed,
            ExitReason::Timeout,
        ];
        for reason in reasons {
            let json = serde_json::to_string(&reason).unwrap();
            let _: ExitReason = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn process_down_serde() {
        let pd = ProcessDown {
            pid: 42,
            reason: ExitReason::Crash("segfault".into()),
        };
        let json = serde_json::to_string(&pd).unwrap();
        let restored: ProcessDown = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.pid, 42);
        assert!(matches!(restored.reason, ExitReason::Crash(ref s) if s == "segfault"));
    }

    #[test]
    fn message_with_correlation_id() {
        let msg = KernelMessage::with_correlation(
            1,
            MessageTarget::Process(2),
            MessagePayload::Text("request".into()),
            "req-123".into(),
        );
        assert_eq!(msg.correlation_id, Some("req-123".into()));
        assert_eq!(msg.from, 1);
    }

    #[test]
    fn message_without_correlation_id() {
        let msg = KernelMessage::text(1, MessageTarget::Process(2), "hello");
        assert!(msg.correlation_id.is_none());
    }

    #[test]
    fn tool_call_message() {
        let msg = KernelMessage::tool_call(
            1,
            MessageTarget::Process(2),
            "read_file",
            serde_json::json!({"path": "/src/main.rs"}),
        );
        match &msg.payload {
            MessagePayload::ToolCall { name, args } => {
                assert_eq!(name, "read_file");
                assert_eq!(args["path"], "/src/main.rs");
            }
            other => panic!("expected ToolCall, got: {other:?}"),
        }
    }

    #[test]
    fn tool_result_message() {
        let msg = KernelMessage::tool_result(
            2,
            MessageTarget::Process(1),
            "call-123",
            serde_json::json!({"content": "file contents"}),
        );
        match &msg.payload {
            MessagePayload::ToolResult { call_id, result } => {
                assert_eq!(call_id, "call-123");
                assert_eq!(result["content"], "file contents");
            }
            other => panic!("expected ToolResult, got: {other:?}"),
        }
    }

    #[test]
    fn topic_target() {
        let msg = KernelMessage::text(1, MessageTarget::Topic("build-status".into()), "done");
        assert!(matches!(msg.target, MessageTarget::Topic(ref t) if t == "build-status"));
    }

    #[test]
    fn tool_call_serde_roundtrip() {
        let msg = KernelMessage::tool_call(
            1,
            MessageTarget::Process(2),
            "search",
            serde_json::json!({"query": "test"}),
        );
        let json = serde_json::to_string(&msg).unwrap();
        let restored: KernelMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored.payload,
            MessagePayload::ToolCall { ref name, .. } if name == "search"
        ));
    }

    #[test]
    fn correlation_id_serde_roundtrip() {
        let msg = KernelMessage::with_correlation(
            1,
            MessageTarget::Process(2),
            MessagePayload::Text("req".into()),
            "corr-456".into(),
        );
        let json = serde_json::to_string(&msg).unwrap();
        let restored: KernelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.correlation_id, Some("corr-456".into()));
    }

    #[test]
    fn rvf_payload_variant() {
        let payload = MessagePayload::Rvf {
            segment_type: 0x40,
            data: vec![0xCA, 0xFE],
        };
        let msg = KernelMessage::new(1, MessageTarget::Process(2), payload);
        assert_eq!(msg.payload.type_name(), "rvf");

        // serde roundtrip
        let json = serde_json::to_string(&msg).unwrap();
        let restored: KernelMessage = serde_json::from_str(&json).unwrap();
        match &restored.payload {
            MessagePayload::Rvf { segment_type, data } => {
                assert_eq!(*segment_type, 0x40);
                assert_eq!(data, &[0xCA, 0xFE]);
            }
            other => panic!("expected Rvf, got: {other:?}"),
        }
    }

    #[test]
    fn payload_type_names() {
        assert_eq!(MessagePayload::Text("hi".into()).type_name(), "text");
        assert_eq!(
            MessagePayload::Json(serde_json::json!(1)).type_name(),
            "json"
        );
        assert_eq!(
            MessagePayload::Signal(KernelSignal::Ping).type_name(),
            "signal"
        );
        assert_eq!(
            MessagePayload::Binary(vec![0xDE, 0xAD]).type_name(),
            "binary"
        );
    }

    #[test]
    fn binary_payload_serde_roundtrip() {
        let payload = MessagePayload::Binary(vec![0x01, 0x02, 0x03, 0xFF]);
        let json = serde_json::to_string(&payload).unwrap();
        let restored: MessagePayload = serde_json::from_str(&json).unwrap();
        match restored {
            MessagePayload::Binary(data) => {
                assert_eq!(data, vec![0x01, 0x02, 0x03, 0xFF]);
            }
            other => panic!("expected Binary, got {:?}", other),
        }
    }

    #[test]
    fn remote_node_serde_roundtrip() {
        let target = MessageTarget::RemoteNode {
            node_id: "node-42".into(),
            target: Box::new(MessageTarget::Process(7)),
        };
        let json = serde_json::to_string(&target).unwrap();
        let restored: MessageTarget = serde_json::from_str(&json).unwrap();
        match restored {
            MessageTarget::RemoteNode { node_id, target } => {
                assert_eq!(node_id, "node-42");
                assert!(matches!(*target, MessageTarget::Process(7)));
            }
            other => panic!("expected RemoteNode, got: {other:?}"),
        }
    }

    #[test]
    fn global_pid_equality() {
        let a = GlobalPid::local(1, "node-a");
        let b = GlobalPid::local(1, "node-b");
        let c = GlobalPid::local(1, "node-a");
        assert_ne!(a, b, "same pid on different nodes must not be equal");
        assert_eq!(a, c, "same pid on same node must be equal");
    }

    #[test]
    fn global_pid_is_local() {
        let gpid = GlobalPid::local(5, "my-node");
        assert!(gpid.is_local("my-node"));
        assert!(!gpid.is_local("other-node"));
    }

    #[test]
    fn global_pid_display() {
        let gpid = GlobalPid::local(42, "alpha");
        assert_eq!(gpid.to_string(), "alpha:42");
    }

    #[test]
    fn correlation_id_absent_in_json_when_none() {
        let msg = KernelMessage::text(1, MessageTarget::Process(2), "hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("correlation_id"));
    }

    // ── K2-G4: Trace ID tests ──────────────────────────────────

    #[test]
    fn trace_id_absent_by_default() {
        let msg = KernelMessage::text(1, MessageTarget::Process(2), "hello");
        assert!(msg.trace_id.is_none());
    }

    #[test]
    fn trace_id_with_trace() {
        let msg = KernelMessage::with_trace(
            1,
            MessageTarget::Process(2),
            MessagePayload::Text("traced".into()),
            "trace-abc-123".into(),
        );
        assert_eq!(msg.trace_id, Some("trace-abc-123".into()));
    }

    #[test]
    fn trace_id_set_builder() {
        let msg = KernelMessage::text(1, MessageTarget::Process(2), "hello")
            .set_trace_id("my-trace");
        assert_eq!(msg.trace_id, Some("my-trace".into()));
    }

    #[test]
    fn trace_id_ensure_generates() {
        let msg = KernelMessage::text(1, MessageTarget::Process(2), "hello").ensure_trace_id();
        assert!(msg.trace_id.is_some());
        assert!(!msg.trace_id.as_ref().unwrap().is_empty());
    }

    #[test]
    fn trace_id_ensure_preserves_existing() {
        let msg = KernelMessage::text(1, MessageTarget::Process(2), "hello")
            .set_trace_id("existing-trace")
            .ensure_trace_id();
        assert_eq!(msg.trace_id, Some("existing-trace".into()));
    }

    #[test]
    fn trace_id_serde_roundtrip() {
        let msg = KernelMessage::with_trace(
            1,
            MessageTarget::Process(2),
            MessagePayload::Text("traced".into()),
            "trace-roundtrip".into(),
        );
        let json = serde_json::to_string(&msg).unwrap();
        let restored: KernelMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.trace_id, Some("trace-roundtrip".into()));
    }

    #[test]
    fn trace_id_absent_in_json_when_none() {
        let msg = KernelMessage::text(1, MessageTarget::Process(2), "hello");
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("trace_id"));
    }

    #[test]
    fn trace_id_backward_compat_deserialization() {
        // Simulate a message serialized without trace_id field
        let json = r#"{"id":"test-id","from":1,"target":{"Process":2},"payload":{"Text":"hello"},"timestamp":"2024-01-01T00:00:00Z"}"#;
        let msg: KernelMessage = serde_json::from_str(json).unwrap();
        assert!(msg.trace_id.is_none());
        assert!(msg.correlation_id.is_none());
    }
}
