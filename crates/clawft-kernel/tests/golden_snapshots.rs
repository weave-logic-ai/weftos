//! Golden artifact snapshot tests (Sprint 11, Track 2).
//!
//! These tests use the `insta` crate to capture canonical serialization
//! formats for key data structures. If a serialization format changes,
//! `cargo insta review` surfaces the diff for human review.

#[cfg(all(feature = "exochain", feature = "native"))]
mod exochain_snapshots {
    use clawft_kernel::chain::ChainManager;

    #[test]
    fn chain_event_serialization_format() {
        let cm = ChainManager::new(0, 100);

        // Append a test event after genesis.
        cm.append(
            "test-source",
            "test.event",
            Some(serde_json::json!({
                "agent_id": "agent-1",
                "action": "tool.read_file",
                "detail": "reading config",
            })),
        );

        // Get the last event (the one we just appended).
        let events = cm.tail(1);
        assert_eq!(events.len(), 1);

        let event = &events[0];

        // Snapshot the structure with deterministic fields only.
        // Hashes and timestamps are non-deterministic, so we snapshot
        // the shape rather than exact bytes.
        let shape = serde_json::json!({
            "sequence": event.sequence,
            "chain_id": event.chain_id,
            "source": event.source,
            "kind": event.kind,
            "has_payload": event.payload.is_some(),
            "has_prev_hash": event.prev_hash != [0u8; 32],
            "has_hash": event.hash != [0u8; 32],
            "has_payload_hash": event.payload_hash != [0u8; 32],
        });

        insta::assert_json_snapshot!("chain_event_structure", shape);
    }

    #[test]
    fn chain_genesis_event_format() {
        let cm = ChainManager::new(42, 100);

        // Genesis is event 0.
        let events = cm.tail(1);
        assert_eq!(events.len(), 1);
        let genesis = &events[0];

        let shape = serde_json::json!({
            "sequence": genesis.sequence,
            "chain_id": genesis.chain_id,
            "source": genesis.source,
            "kind": genesis.kind,
            "payload": genesis.payload,
            "prev_hash_is_zero": genesis.prev_hash == [0u8; 32],
        });

        insta::assert_json_snapshot!("chain_genesis_event", shape);
    }

    #[test]
    fn chain_checkpoint_format() {
        let cm = ChainManager::new(0, 2);

        // Append enough events to trigger auto-checkpoint.
        cm.append("src", "evt.1", None);
        cm.append("src", "evt.2", None);
        cm.append("src", "evt.3", None);

        let checkpoints = cm.checkpoints();
        assert!(
            !checkpoints.is_empty(),
            "checkpoint should exist after exceeding interval"
        );

        let checkpoint = &checkpoints[0];
        let shape = serde_json::json!({
            "chain_id": checkpoint.chain_id,
            "has_last_hash": checkpoint.last_hash != [0u8; 32],
            "sequence_gte_2": checkpoint.sequence >= 2,
        });

        insta::assert_json_snapshot!("chain_checkpoint_structure", shape);
    }
}

#[cfg(feature = "native")]
mod a2a_snapshots {
    use chrono::Utc;
    use clawft_kernel::ipc::{KernelMessage, MessagePayload, MessageTarget};

    #[test]
    fn a2a_text_message_envelope() {
        let msg = KernelMessage {
            id: "msg-00000000-0000-0000-0000-000000000001".into(),
            from: 1,
            target: MessageTarget::Process(7),
            payload: MessagePayload::Text("hello agent-7".into()),
            timestamp: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            correlation_id: None,
            trace_id: Some("trace-abc-123".into()),
        };

        let json = serde_json::to_value(&msg).unwrap();
        insta::assert_json_snapshot!("a2a_text_message_envelope", json);
    }

    #[test]
    fn a2a_tool_call_envelope() {
        let msg = KernelMessage {
            id: "msg-00000000-0000-0000-0000-000000000002".into(),
            from: 3,
            target: MessageTarget::Service("file-service".into()),
            payload: MessagePayload::ToolCall {
                name: "read_file".into(),
                args: serde_json::json!({"path": "/etc/config"}),
            },
            timestamp: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            correlation_id: Some("corr-xyz".into()),
            trace_id: Some("trace-def-456".into()),
        };

        let json = serde_json::to_value(&msg).unwrap();
        insta::assert_json_snapshot!("a2a_tool_call_envelope", json);
    }

    #[test]
    fn a2a_tool_result_envelope() {
        let msg = KernelMessage {
            id: "msg-00000000-0000-0000-0000-000000000003".into(),
            from: 7,
            target: MessageTarget::Process(3),
            payload: MessagePayload::ToolResult {
                call_id: "corr-xyz".into(),
                result: serde_json::json!({"content": "file contents here"}),
            },
            timestamp: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            correlation_id: Some("corr-xyz".into()),
            trace_id: Some("trace-def-456".into()),
        };

        let json = serde_json::to_value(&msg).unwrap();
        insta::assert_json_snapshot!("a2a_tool_result_envelope", json);
    }

    #[test]
    fn a2a_broadcast_message_envelope() {
        let msg = KernelMessage {
            id: "msg-00000000-0000-0000-0000-000000000004".into(),
            from: 0,
            target: MessageTarget::Broadcast,
            payload: MessagePayload::Json(serde_json::json!({
                "event": "shutdown",
                "reason": "maintenance"
            })),
            timestamp: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            correlation_id: None,
            trace_id: None,
        };

        let json = serde_json::to_value(&msg).unwrap();
        insta::assert_json_snapshot!("a2a_broadcast_message_envelope", json);
    }

    #[test]
    fn a2a_service_method_envelope() {
        let msg = KernelMessage {
            id: "msg-00000000-0000-0000-0000-000000000005".into(),
            from: 2,
            target: MessageTarget::ServiceMethod {
                service: "auth-service".into(),
                method: "validate_token".into(),
            },
            payload: MessagePayload::Json(serde_json::json!({
                "token": "jwt-xyz"
            })),
            timestamp: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            correlation_id: None,
            trace_id: Some("trace-ghi-789".into()),
        };

        let json = serde_json::to_value(&msg).unwrap();
        insta::assert_json_snapshot!("a2a_service_method_envelope", json);
    }
}

#[cfg(feature = "native")]
mod config_snapshots {
    use clawft_types::config::{Config, KernelConfig};

    #[test]
    fn default_config_snapshot() {
        let config = Config::default();
        let json = serde_json::to_value(&config).unwrap();
        insta::assert_json_snapshot!("default_config", json);
    }

    #[test]
    fn default_kernel_config_snapshot() {
        let kernel = KernelConfig::default();
        let json = serde_json::to_value(&kernel).unwrap();
        insta::assert_json_snapshot!("default_kernel_config", json);
    }
}

#[cfg(all(feature = "exochain", feature = "native"))]
mod gate_decision_snapshots {
    use clawft_kernel::gate::GateDecision;

    #[test]
    fn gate_decision_permit_snapshot() {
        let d = GateDecision::Permit { token: None };
        insta::assert_json_snapshot!("gate_decision_permit", serde_json::to_value(&d).unwrap());
    }

    #[test]
    fn gate_decision_permit_with_token_snapshot() {
        let d = GateDecision::Permit {
            token: Some(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        };
        insta::assert_json_snapshot!(
            "gate_decision_permit_with_token",
            serde_json::to_value(&d).unwrap()
        );
    }

    #[test]
    fn gate_decision_deny_snapshot() {
        let d = GateDecision::Deny {
            reason: "risk threshold exceeded".into(),
            receipt: None,
        };
        insta::assert_json_snapshot!("gate_decision_deny", serde_json::to_value(&d).unwrap());
    }

    #[test]
    fn gate_decision_defer_snapshot() {
        let d = GateDecision::Defer {
            reason: "needs human approval".into(),
        };
        insta::assert_json_snapshot!("gate_decision_defer", serde_json::to_value(&d).unwrap());
    }
}
