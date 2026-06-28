//! Built-in tool catalog: 27+ kernel tool specifications.

use super::types::*;
use crate::governance::EffectVector;

/// Return the complete catalog of 27 built-in kernel tools.
pub fn builtin_tool_catalog() -> Vec<BuiltinToolSpec> {
    let mut catalog = Vec::with_capacity(29);

    // --- Filesystem tools (10) ---
    catalog.push(BuiltinToolSpec {
        name: "fs.read_file".into(),
        category: ToolCategory::Filesystem,
        description: "Read file contents with optional offset/limit".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path to read"},
                "offset": {"type": "integer", "description": "Byte offset to start reading"},
                "limit": {"type": "integer", "description": "Maximum bytes to read"}
            },
            "required": ["path"]
        }),
        gate_action: "tool.fs.read".into(),
        effect: EffectVector {
            risk: 0.1,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.write_file".into(),
        category: ToolCategory::Filesystem,
        description: "Write content to a file".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"},
                "append": {"type": "boolean", "default": false}
            },
            "required": ["path", "content"]
        }),
        gate_action: "tool.fs.write".into(),
        effect: EffectVector {
            risk: 0.4,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.read_dir".into(),
        category: ToolCategory::Filesystem,
        description: "List directory contents".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        }),
        gate_action: "tool.fs.read".into(),
        effect: EffectVector {
            risk: 0.1,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.create_dir".into(),
        category: ToolCategory::Filesystem,
        description: "Create a directory (recursive)".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "recursive": {"type": "boolean", "default": true}
            },
            "required": ["path"]
        }),
        gate_action: "tool.fs.write".into(),
        effect: EffectVector {
            risk: 0.3,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.remove".into(),
        category: ToolCategory::Filesystem,
        description: "Remove a file or directory".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "recursive": {"type": "boolean", "default": false}
            },
            "required": ["path"]
        }),
        gate_action: "tool.fs.delete".into(),
        effect: EffectVector {
            risk: 0.7,
            security: 0.3,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.copy".into(),
        category: ToolCategory::Filesystem,
        description: "Copy a file or directory".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "src": {"type": "string"},
                "dst": {"type": "string"}
            },
            "required": ["src", "dst"]
        }),
        gate_action: "tool.fs.write".into(),
        effect: EffectVector {
            risk: 0.3,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.move".into(),
        category: ToolCategory::Filesystem,
        description: "Move/rename a file or directory".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "src": {"type": "string"},
                "dst": {"type": "string"}
            },
            "required": ["src", "dst"]
        }),
        gate_action: "tool.fs.write".into(),
        effect: EffectVector {
            risk: 0.5,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.stat".into(),
        category: ToolCategory::Filesystem,
        description: "Get file/directory metadata".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        }),
        gate_action: "tool.fs.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.exists".into(),
        category: ToolCategory::Filesystem,
        description: "Check if a path exists".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        }),
        gate_action: "tool.fs.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "fs.glob".into(),
        category: ToolCategory::Filesystem,
        description: "Find files matching a glob pattern".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "base_dir": {"type": "string"}
            },
            "required": ["pattern"]
        }),
        gate_action: "tool.fs.read".into(),
        effect: EffectVector {
            risk: 0.1,
            ..Default::default()
        },
        native: true,
    });

    // --- Agent tools (7) ---
    catalog.push(BuiltinToolSpec {
        name: "agent.spawn".into(),
        category: ToolCategory::Agent,
        description: "Spawn a new agent process".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_id": {"type": "string"},
                "template": {"type": "string"},
                "capabilities": {"type": "object"},
                "backend": {"type": "string", "enum": ["native", "wasm", "container"]}
            },
            "required": ["agent_id"]
        }),
        gate_action: "tool.agent.spawn".into(),
        effect: EffectVector {
            risk: 0.5,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "agent.stop".into(),
        category: ToolCategory::Agent,
        description: "Stop a running agent".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {"type": "integer"},
                "graceful": {"type": "boolean", "default": true}
            },
            "required": ["pid"]
        }),
        gate_action: "tool.agent.stop".into(),
        effect: EffectVector {
            risk: 0.4,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "agent.list".into(),
        category: ToolCategory::Agent,
        description: "List all running agents".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        gate_action: "tool.agent.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "agent.inspect".into(),
        category: ToolCategory::Agent,
        description: "Inspect agent details".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {"type": "integer"}
            },
            "required": ["pid"]
        }),
        gate_action: "tool.agent.read".into(),
        effect: EffectVector {
            risk: 0.1,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "agent.send".into(),
        category: ToolCategory::Agent,
        description: "Send a message to an agent".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {"type": "integer"},
                "message": {"type": "object"}
            },
            "required": ["pid", "message"]
        }),
        gate_action: "tool.agent.ipc".into(),
        effect: EffectVector {
            risk: 0.2,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "agent.suspend".into(),
        category: ToolCategory::Agent,
        description: "Suspend an agent".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {"type": "integer"}
            },
            "required": ["pid"]
        }),
        gate_action: "tool.agent.suspend".into(),
        effect: EffectVector {
            risk: 0.3,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "agent.resume".into(),
        category: ToolCategory::Agent,
        description: "Resume a suspended agent".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "pid": {"type": "integer"}
            },
            "required": ["pid"]
        }),
        gate_action: "tool.agent.resume".into(),
        effect: EffectVector {
            risk: 0.2,
            ..Default::default()
        },
        native: true,
    });

    // --- IPC tools (2) ---
    catalog.push(BuiltinToolSpec {
        name: "ipc.send".into(),
        category: ToolCategory::Agent,
        description: "Send a message to an agent or topic via kernel IPC".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "target_pid": {"type": "integer", "description": "Target agent PID"},
                "topic": {"type": "string", "description": "Topic name (alternative to target_pid)"},
                "payload": {"type": "object", "description": "Message payload (JSON)"},
                "text": {"type": "string", "description": "Plain text message (alternative to payload)"}
            }
        }),
        gate_action: "tool.ipc.send".into(),
        effect: EffectVector { risk: 0.2, ..Default::default() },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "ipc.subscribe".into(),
        category: ToolCategory::Agent,
        description: "Subscribe to a topic for receiving messages".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "topic": {"type": "string", "description": "Topic name to subscribe to"},
                "pid": {"type": "integer", "description": "PID to subscribe (defaults to caller)"}
            },
            "required": ["topic"]
        }),
        gate_action: "tool.ipc.subscribe".into(),
        effect: EffectVector {
            risk: 0.1,
            ..Default::default()
        },
        native: true,
    });

    // --- System tools (10) ---
    catalog.push(BuiltinToolSpec {
        name: "sys.service.list".into(),
        category: ToolCategory::System,
        description: "List registered services".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        gate_action: "tool.sys.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.service.health".into(),
        category: ToolCategory::System,
        description: "Check service health".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            }
        }),
        gate_action: "tool.sys.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.chain.status".into(),
        category: ToolCategory::System,
        description: "Get chain status".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        gate_action: "tool.sys.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.chain.query".into(),
        category: ToolCategory::System,
        description: "Query chain events".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer", "default": 20},
                "source": {"type": "string"},
                "kind": {"type": "string"}
            }
        }),
        gate_action: "tool.sys.read".into(),
        effect: EffectVector {
            risk: 0.1,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.tree.read".into(),
        category: ToolCategory::System,
        description: "Read resource tree".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        gate_action: "tool.sys.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.tree.inspect".into(),
        category: ToolCategory::System,
        description: "Inspect a resource tree node".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            },
            "required": ["path"]
        }),
        gate_action: "tool.sys.read".into(),
        effect: EffectVector {
            risk: 0.1,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.env.get".into(),
        category: ToolCategory::System,
        description: "Get environment variable".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"}
            },
            "required": ["name"]
        }),
        gate_action: "tool.sys.env".into(),
        effect: EffectVector {
            risk: 0.2,
            privacy: 0.3,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.cron.add".into(),
        category: ToolCategory::System,
        description: "Add a cron job".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "interval_secs": {"type": "integer"},
                "command": {"type": "string"},
                "target_pid": {"type": "integer"}
            },
            "required": ["name", "interval_secs", "command"]
        }),
        gate_action: "tool.sys.cron".into(),
        effect: EffectVector {
            risk: 0.4,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.cron.list".into(),
        category: ToolCategory::System,
        description: "List cron jobs".into(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
        gate_action: "tool.sys.read".into(),
        effect: EffectVector {
            risk: 0.05,
            ..Default::default()
        },
        native: true,
    });
    catalog.push(BuiltinToolSpec {
        name: "sys.cron.remove".into(),
        category: ToolCategory::System,
        description: "Remove a cron job".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"}
            },
            "required": ["id"]
        }),
        gate_action: "tool.sys.cron".into(),
        effect: EffectVector {
            risk: 0.3,
            ..Default::default()
        },
        native: true,
    });

    // --- ECC tools (7, behind `ecc` feature) ---
    #[cfg(feature = "ecc")]
    {
        catalog.push(BuiltinToolSpec {
            name: "ecc.embed".into(),
            category: ToolCategory::Ecc,
            description: "Insert vector into HNSW index".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "embedding": {"type": "array", "items": {"type": "number"}},
                    "metadata": {"type": "object"}
                },
                "required": ["id", "embedding"]
            }),
            gate_action: "ecc.embed".into(),
            effect: EffectVector {
                risk: 0.1,
                ..Default::default()
            },
            native: true,
        });
        catalog.push(BuiltinToolSpec {
            name: "ecc.search".into(),
            category: ToolCategory::Ecc,
            description: "k-NN similarity search".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "array", "items": {"type": "number"}},
                    "k": {"type": "integer", "default": 10}
                },
                "required": ["query"]
            }),
            gate_action: "ecc.search".into(),
            effect: EffectVector {
                risk: 0.05,
                ..Default::default()
            },
            native: true,
        });
        catalog.push(BuiltinToolSpec {
            name: "ecc.causal.link".into(),
            category: ToolCategory::Ecc,
            description: "Create causal edge".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "source": {"type": "integer"},
                    "target": {"type": "integer"},
                    "edge_type": {"type": "string"},
                    "weight": {"type": "number", "default": 1.0}
                },
                "required": ["source", "target", "edge_type"]
            }),
            gate_action: "ecc.causal.link".into(),
            effect: EffectVector {
                risk: 0.3,
                ..Default::default()
            },
            native: true,
        });
        catalog.push(BuiltinToolSpec {
            name: "ecc.causal.query".into(),
            category: ToolCategory::Ecc,
            description: "Traverse causal graph".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "node": {"type": "integer"},
                    "direction": {"type": "string", "enum": ["forward", "reverse"]},
                    "depth": {"type": "integer", "default": 3}
                },
                "required": ["node"]
            }),
            gate_action: "ecc.causal.query".into(),
            effect: EffectVector {
                risk: 0.05,
                ..Default::default()
            },
            native: true,
        });
        catalog.push(BuiltinToolSpec {
            name: "ecc.crossref.create".into(),
            category: ToolCategory::Ecc,
            description: "Link nodes across structures".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "source_id": {"type": "string"},
                    "target_id": {"type": "string"},
                    "ref_type": {"type": "string"}
                },
                "required": ["source_id", "target_id", "ref_type"]
            }),
            gate_action: "ecc.crossref.create".into(),
            effect: EffectVector {
                risk: 0.2,
                ..Default::default()
            },
            native: true,
        });
        catalog.push(BuiltinToolSpec {
            name: "ecc.tick.status".into(),
            category: ToolCategory::Ecc,
            description: "Query cognitive tick state".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            gate_action: "ecc.tick.status".into(),
            effect: EffectVector {
                risk: 0.05,
                ..Default::default()
            },
            native: true,
        });
        catalog.push(BuiltinToolSpec {
            name: "ecc.calibration.run".into(),
            category: ToolCategory::Ecc,
            description: "Re-run boot calibration".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
            gate_action: "ecc.calibration.run".into(),
            effect: EffectVector {
                risk: 0.1,
                ..Default::default()
            },
            native: true,
        });
    }

    catalog
}
