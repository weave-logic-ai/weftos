//! Instance verbs of a `workload-host` (`start`, `stop`, `unload`,
//! `status`, `logs`). A `status` without an instance id lists every
//! instance with its decision id, plus the decisions still in flight, so a
//! controller that lost a `place` response can reconcile.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

use super::host_service::{InstanceBody, WorkloadHostService, refuse, runtime_refusal};
use super::msg::{CtlRequest, Refusal, RefusalCode, method};

/// Marks a decision in flight for the life of one `place` / `load`.
pub(super) struct InFlight<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: Option<String>,
}

impl<'a> InFlight<'a> {
    pub(super) fn enter(set: &'a Mutex<HashSet<String>>, id: Option<String>) -> Self {
        if let (Some(d), Ok(mut s)) = (&id, set.lock()) {
            s.insert(d.clone());
        }
        Self { set, id }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let (Some(d), Ok(mut s)) = (&self.id, self.set.lock()) {
            s.remove(d);
        }
    }
}

/// Largest captured output a `logs` answer carries.
const MAX_LOG_BYTES: usize = 64 * 1024;

impl WorkloadHostService {
    pub(super) async fn instance_op(&self, req: &CtlRequest) -> Result<Value, Refusal> {
        let b: InstanceBody = serde_json::from_value(req.body.clone())
            .map_err(|e| refuse(RefusalCode::InvalidRequest, format!("instance body: {e}")))?;
        // In-flight decisions are read before the instance map: `place`
        // inserts its instance before it leaves the in-flight set, so a
        // decision is always visible in one of the two.
        let in_flight: Vec<String> = self
            .in_flight
            .lock()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        let mut map = self.instances.lock().await;
        let Some(iid) = b.instance_id else {
            if req.method != method::STATUS {
                return Err(refuse(RefusalCode::InvalidRequest, "instance_id required"));
            }
            let mut all: Vec<Value> = in_flight
                .into_iter()
                .map(|d| json!({ "decision_id": d, "in_flight": true }))
                .collect();
            for (id, p) in map.iter() {
                let host = &self.routes[&p.route];
                all.push(json!({
                    "instance_id": id, "workload": p.name, "variant": p.variant,
                    "decision_id": p.decision_id, "status": host.status(&p.handle).await,
                }));
            }
            return Ok(Value::Array(all));
        };
        let p = map
            .get_mut(&iid)
            .ok_or_else(|| refuse(RefusalCode::UnknownInstance, format!("no instance {iid}")))?;
        let host = self.routes[&p.route].clone();
        match req.method.as_str() {
            method::START => {
                if let (Some(hk), Some(l)) = (&self.ingest, &p.ingest) {
                    hk.activate(l).map_err(|e| {
                        refuse(RefusalCode::Runtime, format!("ingest bridge: {e}"))
                    })?;
                }
                host.start(&p.handle).await.map(|_| json!({"started": iid}))
            }
            method::STOP => {
                // The token goes first: a stopping cog must not keep a
                // credential the node no longer vouches for.
                if let (Some(hk), Some(l)) = (&self.ingest, &p.ingest) {
                    hk.deactivate(l);
                }
                let grace = Duration::from_millis(b.grace_ms.unwrap_or(2_000).min(60_000));
                host.stop(&p.handle, grace).await.map(|ev| {
                    let audit = ev.audit();
                    p.last = Some(ev);
                    json!({ "stopped": iid, "evidence": audit })
                })
            }
            method::UNLOAD => {
                if let (Some(hk), Some(l)) = (&self.ingest, &p.ingest) {
                    hk.deactivate(l);
                }
                let h = p.handle.clone();
                let r = host.unload(h).await.map(|_| json!({ "unloaded": iid }));
                if r.is_ok() {
                    map.remove(&iid);
                }
                r
            }
            method::STATUS => {
                Ok(json!({ "instance_id": iid, "status": host.status(&p.handle).await }))
            }
            _ => Ok(match &p.last {
                Some(ev) => json!({
                    "instance_id": iid,
                    "stdout": clip(&ev.stdout), "stderr": clip(&ev.stderr),
                    "exit_code": ev.exit_code, "truncated": ev.truncated,
                }),
                None => json!({ "instance_id": iid, "note": "no captured run yet (stop first)" }),
            }),
        }
        .map_err(|e| runtime_refusal(&e))
    }
}

fn clip(s: &str) -> &str {
    let mut end = s.len().min(MAX_LOG_BYTES);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
