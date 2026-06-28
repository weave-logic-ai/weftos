//! Built-in kernel agent work loop.
//!
//! Every daemon-spawned agent runs this loop. It receives messages
//! from the A2ARouter inbox and processes built-in commands.

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::a2a::A2ARouter;
use crate::cron::CronService;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::process::{Pid, ProcessState, ProcessTable, ResourceUsage};

/// Run the built-in kernel agent work loop.
///
/// The agent:
/// 1. Receives messages from its A2ARouter inbox
/// 2. Processes built-in commands dispatched as JSON `{"cmd": "..."}` payloads
/// 3. Sends responses back via A2ARouter
/// 4. Tracks resource usage (messages_sent, tool_calls, cpu_time_ms)
/// 5. Supports suspend/resume via `{"cmd":"suspend"}` / `{"cmd":"resume"}`
/// 6. Enforces gate checks before exec/cron commands (when gate is provided)
/// 7. Exits when the cancellation token is triggered
///
/// Returns an exit code (0 = normal shutdown).
#[allow(clippy::too_many_arguments)]
pub async fn kernel_agent_loop(
    pid: Pid,
    cancel: CancellationToken,
    mut inbox: mpsc::Receiver<KernelMessage>,
    a2a: Arc<A2ARouter>,
    cron: Arc<CronService>,
    process_table: Arc<ProcessTable>,
    tool_registry: Option<Arc<crate::wasm_runner::ToolRegistry>>,
    #[cfg(feature = "exochain")] chain: Option<Arc<crate::chain::ChainManager>>,
    #[cfg(feature = "exochain")] gate: Option<Arc<dyn crate::gate::GateBackend>>,
) -> i32 {
    let started = Instant::now();
    debug!(pid, "agent loop started");

    // Extract agent_id once before the loop (used by gate checks)
    #[cfg(feature = "exochain")]
    let agent_id = process_table
        .get(pid)
        .map(|e| e.agent_id.clone())
        .unwrap_or_else(|| format!("pid-{pid}"));

    let mut usage = ResourceUsage::default();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!(pid, "agent loop cancelled");
                // Final resource update
                usage.cpu_time_ms = started.elapsed().as_millis() as u64;
                let _ = process_table.update_resources(pid, usage);
                return 0;
            }
            msg = inbox.recv() => {
                match msg {
                    Some(message) => {
                        let cmd = extract_cmd(&message);

                        // Log message receipt on chain
                        #[cfg(feature = "exochain")]
                        if let Some(ref cm) = chain {
                            cm.append(
                                "ipc",
                                "ipc.recv",
                                Some(serde_json::json!({
                                    "pid": pid,
                                    "from": message.from,
                                    "msg_id": &message.id,
                                    "cmd": cmd.as_deref().unwrap_or("none"),
                                })),
                            );
                        }

                        // Handle suspend command
                        if cmd.as_deref() == Some("suspend") {
                            // Send acknowledgement BEFORE transitioning state,
                            // because A2ARouter.send() checks sender is Running.
                            let reply = KernelMessage::with_correlation(
                                pid,
                                MessageTarget::Process(message.from),
                                MessagePayload::Json(serde_json::json!({
                                    "status": "suspended",
                                    "pid": pid,
                                })),
                                message.id.clone(),
                            );
                            send_reply(&a2a, reply, #[cfg(feature = "exochain")] chain.as_deref()).await;
                            usage.messages_sent += 1;

                            // Transition to Suspended
                            let _ = process_table.update_state(pid, ProcessState::Suspended);
                            debug!(pid, "agent suspended");

                            #[cfg(feature = "exochain")]
                            if let Some(ref cm) = chain {
                                cm.append(
                                    "supervisor",
                                    "agent.suspend",
                                    Some(serde_json::json!({
                                        "pid": pid,
                                        "from": message.from,
                                        "msg_id": &message.id,
                                    })),
                                );
                            }

                            // Enter parking loop
                            let resumed = parking_loop(
                                pid,
                                &cancel,
                                &mut inbox,
                                &a2a,
                                &process_table,
                                #[cfg(feature = "exochain")] chain.as_deref(),
                                &mut usage,
                            ).await;

                            if !resumed {
                                // Cancelled during suspend
                                usage.cpu_time_ms = started.elapsed().as_millis() as u64;
                                let _ = process_table.update_resources(pid, usage);
                                return 0;
                            }
                            // Resumed — continue main loop
                            continue;
                        }

                        // Gate check for protected commands
                        #[cfg(feature = "exochain")]
                        if let Some(ref gate_backend) = gate
                            && let Some(ref cmd_str) = cmd
                        {
                            let action = match cmd_str.as_str() {
                                "exec" => Some("tool.exec"),
                                "cron.add" => Some("service.cron.add"),
                                "cron.remove" => Some("service.cron.remove"),
                                _ => None,
                            };
                            if let Some(action_str) = action {
                                // Build enriched gate context with tool name and effect vector (K4 A2)
                                let context = if cmd_str == "exec" {
                                    let tool_name = extract_tool_name(&message);
                                    let effect = tool_name.as_deref().and_then(|tn| {
                                        tool_registry.as_ref().and_then(|reg| {
                                            reg.get(tn).map(|t| &t.spec().effect)
                                        })
                                    });
                                    let mut ctx = serde_json::json!({"pid": pid});
                                    if let Some(tn) = &tool_name {
                                        ctx["tool"] = serde_json::json!(tn);
                                    }
                                    if let Some(ev) = effect {
                                        ctx["effect"] = serde_json::json!({
                                            "risk": ev.risk,
                                            "security": ev.security,
                                            "privacy": ev.privacy,
                                        });
                                    }
                                    ctx
                                } else {
                                    serde_json::json!({"pid": pid})
                                };
                                let decision = gate_backend.check(&agent_id, action_str, &context);
                                match decision {
                                    crate::gate::GateDecision::Deny { reason, .. } => {
                                        let reply = KernelMessage::with_correlation(
                                            pid,
                                            MessageTarget::Process(message.from),
                                            MessagePayload::Json(serde_json::json!({
                                                "error": reason,
                                                "denied": true,
                                            })),
                                            message.id.clone(),
                                        );
                                        send_reply(&a2a, reply, chain.as_deref()).await;
                                        usage.messages_sent += 1;
                                        continue;
                                    }
                                    crate::gate::GateDecision::Defer { reason } => {
                                        let reply = KernelMessage::with_correlation(
                                            pid,
                                            MessageTarget::Process(message.from),
                                            MessagePayload::Json(serde_json::json!({
                                                "deferred": true,
                                                "reason": reason,
                                            })),
                                            message.id.clone(),
                                        );
                                        send_reply(&a2a, reply, chain.as_deref()).await;
                                        usage.messages_sent += 1;
                                        continue;
                                    }
                                    crate::gate::GateDecision::Permit { .. } => {
                                        // Permitted — continue with normal handling
                                    }
                                }
                            }
                        }

                        // Track tool_calls for exec command and log sudo usage
                        if cmd.as_deref() == Some("exec") {
                            usage.tool_calls += 1;

                            // K4 B1: Log sudo override usage to chain
                            #[cfg(feature = "exochain")]
                            {
                                let sudo_flag = match &message.payload {
                                    MessagePayload::Json(v) => v.get("sudo").and_then(|s| s.as_bool()).unwrap_or(false),
                                    _ => false,
                                };
                                if sudo_flag
                                    && let Some(ref cm) = chain
                                {
                                    cm.append(
                                        "security",
                                        "sudo.override",
                                        Some(serde_json::json!({
                                            "pid": pid,
                                            "agent_id": &agent_id,
                                            "tool": extract_tool_name(&message).unwrap_or_default(),
                                        })),
                                    );
                                }
                            }
                        }

                        handle_message(
                            pid,
                            &message,
                            &a2a,
                            &cron,
                            tool_registry.as_deref(),
                            #[cfg(feature = "exochain")]
                            chain.as_deref(),
                            &started,
                        ).await;

                        usage.messages_sent += 1;

                        // Log message acknowledgement on chain
                        #[cfg(feature = "exochain")]
                        if let Some(ref cm) = chain {
                            cm.append(
                                "ipc",
                                "ipc.ack",
                                Some(serde_json::json!({
                                    "pid": pid,
                                    "msg_id": &message.id,
                                    "cmd": cmd.as_deref().unwrap_or("none"),
                                    "status": "processed",
                                })),
                            );
                        }

                        // Update resource counters every 10 messages and periodically
                        if usage.messages_sent % 10 == 0 {
                            usage.cpu_time_ms = started.elapsed().as_millis() as u64;
                            let _ = process_table.update_resources(pid, usage.clone());
                        }
                    }
                    None => {
                        // Inbox closed — shutdown
                        debug!(pid, "inbox closed, exiting");
                        usage.cpu_time_ms = started.elapsed().as_millis() as u64;
                        let _ = process_table.update_resources(pid, usage);
                        return 0;
                    }
                }
            }
        }
    }
}

/// Extract the command string from a message payload.
fn extract_cmd(msg: &KernelMessage) -> Option<String> {
    match &msg.payload {
        MessagePayload::Json(v) => v.get("cmd").and_then(|c| c.as_str()).map(String::from),
        MessagePayload::Text(text) => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                v.get("cmd").and_then(|c| c.as_str()).map(String::from)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extract the tool name from an exec message payload.
#[cfg(feature = "exochain")]
fn extract_tool_name(msg: &KernelMessage) -> Option<String> {
    match &msg.payload {
        MessagePayload::Json(v) => v.get("tool").and_then(|t| t.as_str()).map(String::from),
        MessagePayload::Text(text) => serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| v.get("tool").and_then(|t| t.as_str()).map(String::from)),
        _ => None,
    }
}

/// Parking loop for suspended agents.
///
/// Waits for either a resume command or cancellation. Returns `true`
/// if resumed, `false` if cancelled.
async fn parking_loop(
    pid: Pid,
    cancel: &CancellationToken,
    inbox: &mut mpsc::Receiver<KernelMessage>,
    a2a: &A2ARouter,
    process_table: &ProcessTable,
    #[cfg(feature = "exochain")] chain: Option<&crate::chain::ChainManager>,
    usage: &mut ResourceUsage,
) -> bool {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!(pid, "cancelled while suspended");
                return false;
            }
            msg = inbox.recv() => {
                match msg {
                    Some(message) => {
                        let cmd = extract_cmd(&message);
                        if cmd.as_deref() == Some("resume") {
                            // Transition back to Running
                            let _ = process_table.update_state(pid, ProcessState::Running);
                            debug!(pid, "agent resumed");

                            #[cfg(feature = "exochain")]
                            if let Some(cm) = chain {
                                cm.append(
                                    "supervisor",
                                    "agent.resume",
                                    Some(serde_json::json!({
                                        "pid": pid,
                                        "from": message.from,
                                        "msg_id": &message.id,
                                    })),
                                );
                            }

                            let reply = KernelMessage::with_correlation(
                                pid,
                                MessageTarget::Process(message.from),
                                MessagePayload::Json(serde_json::json!({
                                    "status": "resumed",
                                    "pid": pid,
                                })),
                                message.id.clone(),
                            );
                            send_reply(a2a, reply, #[cfg(feature = "exochain")] chain).await;
                            usage.messages_sent += 1;
                            return true;
                        }

                        // All other commands while suspended get an error
                        let reply = KernelMessage::with_correlation(
                            pid,
                            MessageTarget::Process(message.from),
                            MessagePayload::Json(serde_json::json!({
                                "error": "agent suspended",
                                "pid": pid,
                            })),
                            message.id.clone(),
                        );
                        send_reply(a2a, reply, #[cfg(feature = "exochain")] chain).await;
                        usage.messages_sent += 1;
                    }
                    None => {
                        // Inbox closed while suspended
                        return false;
                    }
                }
            }
        }
    }
}

/// Send a reply message through the A2ARouter.
async fn send_reply(
    a2a: &A2ARouter,
    reply: KernelMessage,
    #[cfg(feature = "exochain")] chain: Option<&crate::chain::ChainManager>,
) {
    #[cfg(feature = "exochain")]
    {
        if let Err(e) = a2a.send_checked(reply, chain).await {
            warn!(error = %e, "failed to send reply");
        }
    }
    #[cfg(not(feature = "exochain"))]
    {
        if let Err(e) = a2a.send(reply).await {
            warn!(error = %e, "failed to send reply");
        }
    }
}

/// Handle a single inbound message.
async fn handle_message(
    pid: Pid,
    msg: &KernelMessage,
    a2a: &A2ARouter,
    cron: &CronService,
    tool_registry: Option<&crate::wasm_runner::ToolRegistry>,
    #[cfg(feature = "exochain")] chain: Option<&crate::chain::ChainManager>,
    started: &Instant,
) {
    // Extract command from payload — supports JSON, Text, and RVF envelopes.
    let cmd_value = match &msg.payload {
        MessagePayload::Json(v) => v.clone(),
        MessagePayload::Text(text) => {
            // Try parsing text as JSON, otherwise treat as plain text
            match serde_json::from_str::<serde_json::Value>(text) {
                Ok(v) => v,
                Err(_) => serde_json::json!({"cmd": "echo", "text": text}),
            }
        }
        MessagePayload::Rvf { segment_type, data } => {
            // Decode RVF-typed payloads:
            //   0x40 (ExochainEvent) — treat inner CBOR/JSON as command
            //   Other — wrap as a typed envelope for the agent
            debug!(
                pid,
                segment_type,
                data_len = data.len(),
                "received RVF payload"
            );

            // With exochain: try CBOR decode first (rvf-wire format), then JSON
            #[cfg(feature = "exochain")]
            {
                if let Ok(val) = ciborium::from_reader::<ciborium::Value, _>(&data[..]) {
                    let json_str = serde_json::to_string(&val).unwrap_or_default();
                    match serde_json::from_str::<serde_json::Value>(&json_str) {
                        Ok(v) => v,
                        Err(_) => serde_json::json!({
                            "cmd": "rvf.recv",
                            "segment_type": segment_type,
                            "data_len": data.len(),
                        }),
                    }
                } else if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
                    v
                } else {
                    serde_json::json!({
                        "cmd": "rvf.recv",
                        "segment_type": segment_type,
                        "data_len": data.len(),
                    })
                }
            }
            // Without exochain: try JSON decode, fall back to rvf.recv
            #[cfg(not(feature = "exochain"))]
            {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) {
                    v
                } else {
                    serde_json::json!({
                        "cmd": "rvf.recv",
                        "segment_type": segment_type,
                        "data_len": data.len(),
                    })
                }
            }
        }
        _ => {
            debug!(pid, "ignoring signal message");
            return;
        }
    };

    let cmd = cmd_value
        .get("cmd")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let response = match cmd {
        "ping" => {
            let uptime_ms = started.elapsed().as_millis() as u64;
            serde_json::json!({
                "status": "ok",
                "pid": pid,
                "uptime_ms": uptime_ms,
            })
        }
        "cron.add" => {
            let name = cmd_value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unnamed")
                .to_string();
            let interval_secs = cmd_value
                .get("interval_secs")
                .and_then(|v| v.as_u64())
                .unwrap_or(60);
            let command = cmd_value
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("ping")
                .to_string();
            let target_pid = cmd_value.get("target_pid").and_then(|v| v.as_u64());

            match cron.add_job(name, interval_secs, command, target_pid) {
                Ok(job) => serde_json::to_value(&job).unwrap_or_default(),
                Err(e) => serde_json::json!({ "error": e.to_string() }),
            }
        }
        "cron.list" => {
            let jobs = cron.list_jobs();
            serde_json::to_value(&jobs).unwrap_or_default()
        }
        "cron.remove" => {
            let id = cmd_value.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match cron.remove_job(id) {
                Ok(Some(job)) => {
                    #[cfg(feature = "exochain")]
                    if let Some(cm) = chain {
                        cm.append(
                            "cron",
                            "cron.remove",
                            Some(serde_json::json!({
                                "job_id": job.id,
                                "name": job.name,
                                "via_agent": pid,
                            })),
                        );
                    }
                    serde_json::json!({"removed": true, "job_id": job.id})
                }
                Ok(None) => serde_json::json!({"removed": false, "error": "job not found"}),
                Err(e) => serde_json::json!({"removed": false, "error": format!("{e}")}),
            }
        }
        "exec" => {
            // K3 tool dispatch via ToolRegistry
            let tool_name = cmd_value.get("tool").and_then(|v| v.as_str()).unwrap_or("");
            let args = cmd_value
                .get("args")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            if tool_name.is_empty() {
                // Backwards compat: echo mode when no tool specified
                let text = cmd_value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no input)");
                serde_json::json!({
                    "status": "ok",
                    "echo": text,
                    "pid": pid,
                })
            } else if let Some(registry) = tool_registry {
                match registry.execute(tool_name, args) {
                    Ok(result) => {
                        #[cfg(feature = "exochain")]
                        if let Some(cm) = chain {
                            cm.append(
                                "tool",
                                "tool.exec",
                                Some(serde_json::json!({
                                    "tool": tool_name,
                                    "pid": pid,
                                    "status": "ok",
                                })),
                            );
                        }
                        serde_json::json!({
                            "status": "ok",
                            "tool": tool_name,
                            "result": result,
                            "pid": pid,
                        })
                    }
                    Err(e) => {
                        #[cfg(feature = "exochain")]
                        if let Some(cm) = chain {
                            cm.append(
                                "tool",
                                "tool.exec",
                                Some(serde_json::json!({
                                    "tool": tool_name,
                                    "pid": pid,
                                    "status": "error",
                                    "error": e.to_string(),
                                })),
                            );
                        }
                        serde_json::json!({
                            "error": e.to_string(),
                            "tool": tool_name,
                            "pid": pid,
                        })
                    }
                }
            } else {
                serde_json::json!({
                    "error": "tool registry not available",
                    "tool": tool_name,
                    "pid": pid,
                })
            }
        }
        "echo" => {
            let text = cmd_value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            serde_json::json!({"echo": text, "pid": pid})
        }
        "rvf.recv" => {
            // Acknowledge receipt of an RVF-typed payload
            let seg_type = cmd_value
                .get("segment_type")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let data_len = cmd_value
                .get("data_len")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            serde_json::json!({
                "status": "ok",
                "cmd": "rvf.recv",
                "segment_type": seg_type,
                "data_len": data_len,
                "pid": pid,
            })
        }
        unknown => {
            serde_json::json!({
                "error": format!("unknown command: {unknown}"),
                "pid": pid,
            })
        }
    };

    // Send response back to sender via chain-logged path
    let reply = KernelMessage::with_correlation(
        pid,
        MessageTarget::Process(msg.from),
        MessagePayload::Json(response),
        msg.id.clone(),
    );

    #[cfg(feature = "exochain")]
    {
        if let Err(e) = a2a.send_checked(reply, chain).await {
            warn!(pid, error = %e, "failed to send reply");
        }
    }
    #[cfg(not(feature = "exochain"))]
    {
        if let Err(e) = a2a.send(reply).await {
            warn!(pid, error = %e, "failed to send reply");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{AgentCapabilities, CapabilityChecker};
    use crate::process::{ProcessEntry, ProcessState, ProcessTable, ResourceUsage};
    use crate::topic::TopicRouter;

    fn setup() -> (Arc<A2ARouter>, Arc<CronService>, Arc<ProcessTable>) {
        let pt = Arc::new(ProcessTable::new(64));

        // Insert a "kernel" process at PID 0 for message routing
        let kernel_entry = ProcessEntry {
            pid: 0,
            agent_id: "kernel".into(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        pt.insert_with_pid(kernel_entry).unwrap();

        let checker = Arc::new(CapabilityChecker::new(pt.clone()));
        let topic_router = Arc::new(TopicRouter::new(pt.clone()));
        let a2a = Arc::new(A2ARouter::new(pt.clone(), checker, topic_router));
        let cron = Arc::new(CronService::new());
        (a2a, cron, pt)
    }

    fn spawn_agent(
        pt: &ProcessTable,
        a2a: &A2ARouter,
        agent_id: &str,
    ) -> (Pid, mpsc::Receiver<KernelMessage>) {
        let entry = ProcessEntry {
            pid: 0,
            agent_id: agent_id.into(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        let pid = pt.insert(entry).unwrap();
        let rx = a2a.create_inbox(pid);
        (pid, rx)
    }

    /// Helper to spawn the agent loop with the new parameters.
    fn spawn_loop(
        agent_pid: Pid,
        cancel: CancellationToken,
        inbox: mpsc::Receiver<KernelMessage>,
        a2a: Arc<A2ARouter>,
        cron: Arc<CronService>,
        pt: Arc<ProcessTable>,
    ) -> tokio::task::JoinHandle<i32> {
        tokio::spawn(async move {
            kernel_agent_loop(
                agent_pid,
                cancel,
                inbox,
                a2a,
                cron,
                pt,
                None, // tool_registry
                #[cfg(feature = "exochain")]
                None,
                #[cfg(feature = "exochain")]
                None,
            )
            .await
        })
    }

    #[tokio::test]
    async fn ping_command() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // Send ping from kernel (PID 0)
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "ping"})),
        );
        a2a.send(msg).await.unwrap();

        // Wait for reply
        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "ok");
            assert_eq!(v["pid"], agent_pid);
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        let code = handle.await.unwrap();
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn unknown_command() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "nosuch"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert!(v["error"].as_str().unwrap().contains("unknown command"));
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cron_add_via_agent() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(
            agent_pid,
            cancel.clone(),
            inbox,
            a2a.clone(),
            cron.clone(),
            pt,
        );

        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({
                "cmd": "cron.add",
                "name": "test-job",
                "interval_secs": 30,
                "command": "health",
            })),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["name"], "test-job");
            assert!(v["id"].as_str().is_some());
        } else {
            panic!("expected JSON reply");
        }

        // Verify job was actually added
        assert_eq!(cron.job_count(), 1);

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_exits_cleanly() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a, cron, pt);

        cancel.cancel();
        let code = handle.await.unwrap();
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn rvf_json_payload_processed() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // Send an RVF payload containing JSON bytes (e.g. `{"cmd":"ping"}`)
        let json_bytes = serde_json::to_vec(&serde_json::json!({"cmd": "ping"})).unwrap();
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Rvf {
                segment_type: 0x40,
                data: json_bytes,
            },
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "ok");
            assert_eq!(v["pid"], agent_pid);
        } else {
            panic!("expected JSON reply to RVF-wrapped ping");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn rvf_opaque_binary_acknowledged() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // Send raw binary that isn't valid JSON or CBOR
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Rvf {
                segment_type: 0x42,
                data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["cmd"], "rvf.recv");
            assert_eq!(v["segment_type"], 0x42);
            assert_eq!(v["data_len"], 4);
        } else {
            panic!("expected JSON reply acknowledging RVF binary");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn resource_usage_increments() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(
            agent_pid,
            cancel.clone(),
            inbox,
            a2a.clone(),
            cron,
            pt.clone(),
        );

        // Send a ping
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "ping"})),
        );
        a2a.send(msg).await.unwrap();

        // Wait for reply
        let _reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        // Cancel and wait for the loop to exit (which triggers final resource update)
        cancel.cancel();
        let _code = handle.await.unwrap();

        // Check resource usage was updated
        let entry = pt.get(agent_pid).unwrap();
        assert!(
            entry.resource_usage.messages_sent >= 1,
            "messages_sent should be at least 1"
        );
    }

    #[tokio::test]
    async fn suspend_resume_cycle() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(
            agent_pid,
            cancel.clone(),
            inbox,
            a2a.clone(),
            cron,
            pt.clone(),
        );

        // Send suspend
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "suspend"})),
        );
        a2a.send(msg).await.unwrap();

        // Wait for suspended acknowledgement
        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "suspended");
        } else {
            panic!("expected JSON reply");
        }

        // Verify process state is Suspended
        let entry = pt.get(agent_pid).unwrap();
        assert_eq!(entry.state, ProcessState::Suspended);

        // Send a ping while suspended — should get error
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "ping"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["error"], "agent suspended");
        } else {
            panic!("expected JSON error reply");
        }

        // Send resume
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "resume"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "resumed");
        } else {
            panic!("expected JSON reply");
        }

        // Verify process state is Running again
        let entry = pt.get(agent_pid).unwrap();
        assert_eq!(entry.state, ProcessState::Running);

        // Ping should work again after resume
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "ping"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "ok");
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn gate_deny_blocks_exec() {
        use crate::gate::{GateBackend, GateDecision};

        // Gate that always denies
        struct AlwaysDeny;
        impl GateBackend for AlwaysDeny {
            fn check(
                &self,
                _agent_id: &str,
                _action: &str,
                _context: &serde_json::Value,
            ) -> GateDecision {
                GateDecision::Deny {
                    reason: "test deny".into(),
                    receipt: None,
                }
            }
        }

        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let cancel2 = cancel.clone();
        let a2a2 = a2a.clone();
        let pt2 = pt.clone();

        let handle = tokio::spawn(async move {
            kernel_agent_loop(
                agent_pid,
                cancel2,
                inbox,
                a2a2,
                cron,
                pt2,
                None, // tool_registry
                None, // chain
                Some(Arc::new(AlwaysDeny) as Arc<dyn GateBackend>),
            )
            .await
        });

        // Send exec — should be denied by gate
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "exec", "text": "hello"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["denied"], true);
            assert_eq!(v["error"], "test deny");
        } else {
            panic!("expected JSON deny reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn gate_permit_allows_exec() {
        use crate::gate::{GateBackend, GateDecision};

        // Gate that always permits
        struct AlwaysPermit;
        impl GateBackend for AlwaysPermit {
            fn check(
                &self,
                _agent_id: &str,
                _action: &str,
                _context: &serde_json::Value,
            ) -> GateDecision {
                GateDecision::Permit { token: None }
            }
        }

        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "test-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let cancel2 = cancel.clone();
        let a2a2 = a2a.clone();
        let pt2 = pt.clone();

        let handle = tokio::spawn(async move {
            kernel_agent_loop(
                agent_pid,
                cancel2,
                inbox,
                a2a2,
                cron,
                pt2,
                None, // tool_registry
                None, // chain
                Some(Arc::new(AlwaysPermit) as Arc<dyn GateBackend>),
            )
            .await
        });

        // Send exec — should be permitted
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "exec", "text": "hello"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "ok");
            assert_eq!(v["echo"], "hello");
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn chain_logs_ipc_recv_ack() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "chain-test");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cm = Arc::new(crate::chain::ChainManager::new(0, 1000));
        let cancel = CancellationToken::new();
        let cancel2 = cancel.clone();
        let a2a2 = a2a.clone();
        let pt2 = pt.clone();
        let cm2 = cm.clone();

        let handle = tokio::spawn(async move {
            kernel_agent_loop(
                agent_pid,
                cancel2,
                inbox,
                a2a2,
                cron,
                pt2,
                None, // tool_registry
                Some(cm2),
                None, // gate
            )
            .await
        });

        // Send ping from kernel (PID 0)
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "ping"})),
        );
        let msg_id = msg.id.clone();
        a2a.send(msg).await.unwrap();

        // Wait for reply
        let _reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        cancel.cancel();
        handle.await.unwrap();

        // Verify chain events: ipc.recv + ipc.ack (plus ipc.send from reply)
        let events = cm.tail(10);
        let recv_evt = events.iter().find(|e| e.kind == "ipc.recv");
        let ack_evt = events.iter().find(|e| e.kind == "ipc.ack");

        assert!(recv_evt.is_some(), "expected ipc.recv event on chain");
        assert!(ack_evt.is_some(), "expected ipc.ack event on chain");

        let recv_payload = recv_evt.unwrap().payload.as_ref().unwrap();
        assert_eq!(recv_payload["pid"], agent_pid);
        assert_eq!(recv_payload["from"], 0);
        assert_eq!(recv_payload["msg_id"], msg_id);
        assert_eq!(recv_payload["cmd"], "ping");

        let ack_payload = ack_evt.unwrap().payload.as_ref().unwrap();
        assert_eq!(ack_payload["pid"], agent_pid);
        assert_eq!(ack_payload["msg_id"], msg_id);
        assert_eq!(ack_payload["cmd"], "ping");
        assert_eq!(ack_payload["status"], "processed");
    }

    #[cfg(feature = "exochain")]
    #[tokio::test]
    async fn chain_logs_suspend_resume() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "suspend-test");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cm = Arc::new(crate::chain::ChainManager::new(0, 1000));
        let cancel = CancellationToken::new();
        let cancel2 = cancel.clone();
        let a2a2 = a2a.clone();
        let pt2 = pt.clone();
        let cm2 = cm.clone();

        let handle = tokio::spawn(async move {
            kernel_agent_loop(
                agent_pid,
                cancel2,
                inbox,
                a2a2,
                cron,
                pt2,
                None, // tool_registry
                Some(cm2),
                None, // gate
            )
            .await
        });

        // Send suspend
        let suspend_msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "suspend"})),
        );
        let suspend_id = suspend_msg.id.clone();
        a2a.send(suspend_msg).await.unwrap();

        // Wait for suspended ack
        let _reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        // Send resume
        let resume_msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "resume"})),
        );
        let resume_id = resume_msg.id.clone();
        a2a.send(resume_msg).await.unwrap();

        // Wait for resumed ack
        let _reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        cancel.cancel();
        handle.await.unwrap();

        // Verify chain events
        let events = cm.tail(20);
        let suspend_evt = events.iter().find(|e| e.kind == "agent.suspend");
        let resume_evt = events.iter().find(|e| e.kind == "agent.resume");

        assert!(
            suspend_evt.is_some(),
            "expected agent.suspend event on chain"
        );
        assert!(resume_evt.is_some(), "expected agent.resume event on chain");

        let sp = suspend_evt.unwrap().payload.as_ref().unwrap();
        assert_eq!(sp["pid"], agent_pid);
        assert_eq!(sp["from"], 0);
        assert_eq!(sp["msg_id"], suspend_id);

        let rp = resume_evt.unwrap().payload.as_ref().unwrap();
        assert_eq!(rp["pid"], agent_pid);
        assert_eq!(rp["from"], 0);
        assert_eq!(rp["msg_id"], resume_id);
    }

    // ── Additional agent_loop coverage tests ─────────────────────

    #[tokio::test]
    async fn echo_command() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "echo-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "echo", "text": "hello world"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["echo"], "hello world");
            assert_eq!(v["pid"], agent_pid);
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn text_payload_parsed_as_json() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "text-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // Send text payload that is valid JSON
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Text(r#"{"cmd": "ping"}"#.into()),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "ok");
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn text_payload_non_json_becomes_echo() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "text-echo-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // Send plain text that isn't JSON
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Text("just plain text".into()),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            // Plain text becomes {"cmd": "echo", "text": "just plain text"}
            assert_eq!(v["echo"], "just plain text");
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn inbox_close_causes_clean_exit() {
        let pt = Arc::new(ProcessTable::new(64));

        // Insert kernel process
        let kernel_entry = ProcessEntry {
            pid: 0,
            agent_id: "kernel".into(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        pt.insert_with_pid(kernel_entry).unwrap();

        let checker = Arc::new(CapabilityChecker::new(pt.clone()));
        let topic_router = Arc::new(TopicRouter::new(pt.clone()));
        let a2a = Arc::new(A2ARouter::new(pt.clone(), checker, topic_router));
        let cron = Arc::new(CronService::new());

        // Create agent with a manually controlled inbox
        let (tx, rx) = mpsc::channel(32);
        let agent_entry = ProcessEntry {
            pid: 0,
            agent_id: "close-agent".into(),
            state: ProcessState::Running,
            capabilities: AgentCapabilities::default(),
            resource_usage: ResourceUsage::default(),
            cancel_token: CancellationToken::new(),
            parent_pid: None,
        };
        let agent_pid = pt.insert(agent_entry).unwrap();

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), rx, a2a, cron, pt);

        // Drop the sender to close the inbox
        drop(tx);

        let code = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(code, 0, "should exit cleanly when inbox closes");
    }

    #[tokio::test]
    async fn cron_list_returns_empty() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "cron-list-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "cron.list"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert!(v.is_array(), "cron.list should return array");
            assert_eq!(v.as_array().unwrap().len(), 0, "should be empty");
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn cron_remove_nonexistent() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "cron-rm-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "cron.remove", "id": "nonexistent"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["removed"], false);
            assert!(v["error"].as_str().is_some());
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn exec_without_tool_echoes() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "exec-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // exec without tool name falls back to echo mode
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "exec", "text": "fallback"})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert_eq!(v["status"], "ok");
            assert_eq!(v["echo"], "fallback");
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn exec_with_tool_name_no_registry() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "exec-noreg-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // exec with tool name but no registry
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "exec", "tool": "fs.read", "args": {}})),
        );
        a2a.send(msg).await.unwrap();

        let reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        if let MessagePayload::Json(v) = &reply.payload {
            assert!(
                v["error"]
                    .as_str()
                    .unwrap()
                    .contains("tool registry not available"),
                "should report tool registry unavailable"
            );
        } else {
            panic!("expected JSON reply");
        }

        cancel.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn multiple_messages_increment_usage() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "multi-msg-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(
            agent_pid,
            cancel.clone(),
            inbox,
            a2a.clone(),
            cron,
            pt.clone(),
        );

        // Send 3 pings
        for _ in 0..3 {
            let msg = KernelMessage::new(
                0,
                MessageTarget::Process(agent_pid),
                MessagePayload::Json(serde_json::json!({"cmd": "ping"})),
            );
            a2a.send(msg).await.unwrap();
            let _reply =
                tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
                    .await
                    .unwrap()
                    .unwrap();
        }

        cancel.cancel();
        let _code = handle.await.unwrap();

        let entry = pt.get(agent_pid).unwrap();
        assert!(
            entry.resource_usage.messages_sent >= 3,
            "should have sent at least 3 messages, got {}",
            entry.resource_usage.messages_sent
        );
    }

    #[tokio::test]
    async fn cancel_during_suspend_exits_cleanly() {
        let (a2a, cron, pt) = setup();
        let (agent_pid, inbox) = spawn_agent(&pt, &a2a, "cancel-suspend-agent");
        let mut kernel_inbox = a2a.create_inbox(0);

        let cancel = CancellationToken::new();
        let handle = spawn_loop(agent_pid, cancel.clone(), inbox, a2a.clone(), cron, pt);

        // Send suspend
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(agent_pid),
            MessagePayload::Json(serde_json::json!({"cmd": "suspend"})),
        );
        a2a.send(msg).await.unwrap();

        // Wait for suspended ack
        let _reply = tokio::time::timeout(std::time::Duration::from_secs(1), kernel_inbox.recv())
            .await
            .unwrap()
            .unwrap();

        // Cancel while suspended
        cancel.cancel();
        let code = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(code, 0, "should exit cleanly when cancelled during suspend");
    }

    #[tokio::test]
    async fn extract_cmd_from_json() {
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(1),
            MessagePayload::Json(serde_json::json!({"cmd": "test_cmd"})),
        );
        assert_eq!(extract_cmd(&msg), Some("test_cmd".to_string()));
    }

    #[tokio::test]
    async fn extract_cmd_from_text() {
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(1),
            MessagePayload::Text(r#"{"cmd": "from_text"}"#.into()),
        );
        assert_eq!(extract_cmd(&msg), Some("from_text".to_string()));
    }

    #[tokio::test]
    async fn extract_cmd_from_plain_text_returns_none() {
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(1),
            MessagePayload::Text("not json".into()),
        );
        assert_eq!(extract_cmd(&msg), None);
    }

    #[tokio::test]
    async fn extract_cmd_from_signal_returns_none() {
        let msg = KernelMessage::new(
            0,
            MessageTarget::Process(1),
            MessagePayload::Signal(crate::ipc::KernelSignal::Shutdown),
        );
        assert_eq!(extract_cmd(&msg), None);
    }
}
