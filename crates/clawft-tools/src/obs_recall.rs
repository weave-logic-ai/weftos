//! `obs_recall` — page a session ObservationPack archive.
//!
//! Read-only. Resolves `id` only inside the current session's
//! `.observations/` ledger (task-local
//! [`ObservationContext`](clawft_core::observation_pack::ObservationContext)).
//! Does not touch workspace FS. Native-only: browser builds skip registration.

use clawft_core::observation_pack::{
    recall, ObservationContext, RECALL_DEFAULT_LIMIT, RECALL_MAX_LIMIT,
};
use clawft_core::tools::registry::{Tool, ToolError};
use serde_json::{json, Value};

/// Page bytes from this session's observation archive.
pub struct ObsRecallTool;

impl ObsRecallTool {
    /// Create the read-only recall tool.
    pub fn new() -> Self {
        Self
    }
}

impl Default for ObsRecallTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Tool for ObsRecallTool {
    fn name(&self) -> &str {
        "obs_recall"
    }

    fn description(&self) -> &str {
        "Page a previously archived tool result. Use the observation_pack id from a projected tool result with offset/limit. Do not re-run the original tool to re-read its output."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Observation id (obs_… from observation_pack)"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Byte offset into the archive (default 0)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": RECALL_MAX_LIMIT,
                    "description": "Page size in bytes (default 8192, max 32768)"
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: Value) -> Result<Value, ToolError> {
        let id = args
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArgs("missing required field: id".into()))?;
        let offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(RECALL_DEFAULT_LIMIT as u64);
        if limit == 0 || limit > RECALL_MAX_LIMIT as u64 {
            return Err(ToolError::InvalidArgs(format!(
                "limit must be 1..={RECALL_MAX_LIMIT}"
            )));
        }
        let ctx = ObservationContext::current().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "obs_recall requires a live session archive context".into(),
            )
        })?;
        recall(
            &ctx.sessions_dir,
            &ctx.session_key,
            id,
            offset,
            limit as u32,
        )
        .await
        .map_err(|e| match e {
            clawft_core::observation_pack::RecallError::InvalidId => {
                ToolError::InvalidArgs(e.to_string())
            }
            clawft_core::observation_pack::RecallError::NotFound => {
                ToolError::FileNotFound(e.to_string())
            }
            other => ToolError::ExecutionFailed(other.to_string()),
        })
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use clawft_core::observation_pack::{pack_tool_result, MAX_PROMPT_BYTES};

    #[tokio::test]
    async fn pages_archive_and_sets_eof() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let val = json!({"data": "p".repeat(12_000)});
        let _ = pack_tool_result(&sessions, "s:1", "bash", "c1", val, MAX_PROMPT_BYTES).await;
        let ledger = tokio::fs::read_to_string(
            clawft_core::observation_pack::observations_dir(&sessions, "s:1").join("ledger.jsonl"),
        )
        .await
        .unwrap();
        let row: serde_json::Value = serde_json::from_str(ledger.lines().next().unwrap()).unwrap();
        let id = row["id"].as_str().unwrap().to_string();
        let total = row["bytes"].as_u64().unwrap();

        let ctx = ObservationContext {
            sessions_dir: sessions,
            session_key: "s:1".into(),
        };
        let page = ctx
            .scope(async {
                ObsRecallTool::new()
                    .execute(json!({"id": id, "offset": 0, "limit": 8192}))
                    .await
            })
            .await
            .unwrap();
        assert_eq!(page["eof"], false);
        assert_eq!(page["bytes"], total);
        assert_eq!(page["chunk"].as_str().unwrap().len(), 8192);

        let ctx = ObservationContext {
            sessions_dir: tmp.path().to_path_buf(),
            session_key: "s:1".into(),
        };
        let last_off = (total / 8192) * 8192;
        let last = ctx
            .scope(async {
                ObsRecallTool::new()
                    .execute(json!({"id": id, "offset": last_off, "limit": 8192}))
                    .await
            })
            .await
            .unwrap();
        assert_eq!(last["eof"], true);
    }

    #[tokio::test]
    async fn rejects_path_traversal_id() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = ObservationContext {
            sessions_dir: tmp.path().to_path_buf(),
            session_key: "s:1".into(),
        };
        let err = ctx
            .scope(async {
                ObsRecallTool::new()
                    .execute(json!({"id": "obs_../ledger"}))
                    .await
            })
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidArgs(_) => {}
            other => panic!("expected InvalidArgs, got {other:?}"),
        }
    }
}
