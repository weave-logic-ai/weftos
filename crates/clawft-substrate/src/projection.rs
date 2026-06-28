//! Row projections — align the daemon's wire shape with the admin
//! ontology's user-facing field names.
//!
//! The daemon's RPC responses use canonical field names
//! (`agent_id`, `cpu_time_ms`, …). The surface-description fixtures
//! bind columns using user-facing ontology names (`name`, `cpu`).
//! These projection helpers enrich each row with aliases without
//! removing the raw fields, so both sets of bindings resolve against
//! the same `substrate/kernel/*` topic.
//!
//! This module is platform-neutral (no I/O, no async) so it can run
//! under the native `KernelAdapter` poller *and* the wasm-only
//! fallback path in `clawft_gui_egui::live`.

use serde_json::Value;

/// Project `kernel.ps` rows. Adds:
/// - `name`  ← `agent_id`
/// - `cpu`   ← display form of `cpu_time_ms` (e.g. `"4.21s"`)
///
/// Idempotent: if the row already carries `name` / `cpu` it is left
/// alone. Non-array / non-object inputs are passed through unchanged.
pub fn project_process_rows(value: Value) -> Value {
    let Some(arr) = value.as_array() else {
        return value;
    };
    let projected: Vec<Value> = arr
        .iter()
        .map(|row| {
            let mut obj = row.as_object().cloned().unwrap_or_default();
            if !obj.contains_key("name")
                && let Some(name) = obj.get("agent_id").and_then(|v| v.as_str())
            {
                obj.insert("name".into(), Value::String(name.to_string()));
            }
            if !obj.contains_key("cpu")
                && let Some(cpu_ms) = obj.get("cpu_time_ms").and_then(|v| v.as_u64())
            {
                obj.insert("cpu".into(), Value::String(format_cpu_ms(cpu_ms)));
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(projected)
}

/// Project `kernel.services` rows. M1.5.1a passes through unchanged
/// — the daemon doesn't yet emit per-service CPU percent or latency.
/// M1.5.1d promotes individual subsystems (chain/mesh/defi) to real
/// per-service adapters and this grows a real projection.
pub fn project_service_rows(value: Value) -> Value {
    value
}

/// Expand a `kernel.services` list into per-service leaf paths so a
/// surface that binds `substrate/kernel/services/<name>/status` can
/// resolve on the wasm fallback (which has only the flat list). The
/// caller gets back a `Vec<(path, value)>` to splice into the
/// `OntologySnapshot`.
pub fn explode_services_by_name(services: &[Value]) -> Vec<(String, Value)> {
    let mut out = Vec::with_capacity(services.len() * 2);
    for svc in services {
        let Some(obj) = svc.as_object() else { continue };
        let Some(name) = obj.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(health) = obj.get("health") {
            out.push((
                format!("substrate/kernel/services/{name}/status"),
                health.clone(),
            ));
        }
        if let Some(stype) = obj.get("service_type") {
            out.push((
                format!("substrate/kernel/services/{name}/service_type"),
                stype.clone(),
            ));
        }
        // Placeholder until M1.5.1d wires real per-service metrics.
        // A bound gauge displays 0.0 — the affordance still dispatches
        // the right verb because the composer derives the service
        // name from the node path, not from this value.
        out.push((
            format!("substrate/kernel/services/{name}/cpu_percent"),
            Value::from(0.0),
        ));
    }
    out
}

fn format_cpu_ms(ms: u64) -> String {
    if ms >= 60_000 {
        let secs = ms / 1000;
        let mins = secs / 60;
        let rem = secs % 60;
        format!("{mins}m{rem:02}s")
    } else if ms >= 1000 {
        format!("{:.2}s", (ms as f64) / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn process_projection_adds_name_and_cpu_aliases() {
        let input = json!([
            {"pid": 1, "agent_id": "kernel.boot", "cpu_time_ms": 1250}
        ]);
        let out = project_process_rows(input);
        let row = &out.as_array().unwrap()[0];
        assert_eq!(row["name"], "kernel.boot");
        assert_eq!(row["cpu"], "1.25s");
        // Raw fields preserved for surfaces binding to canonical names.
        assert_eq!(row["agent_id"], "kernel.boot");
        assert_eq!(row["cpu_time_ms"], 1250);
    }

    #[test]
    fn process_projection_preserves_existing_aliases() {
        let input = json!([
            {"pid": 1, "name": "preexisting", "cpu": "99.99s", "agent_id": "real"}
        ]);
        let out = project_process_rows(input);
        let row = &out.as_array().unwrap()[0];
        // If the caller already provided a `name`, we don't clobber.
        assert_eq!(row["name"], "preexisting");
        assert_eq!(row["cpu"], "99.99s");
    }

    #[test]
    fn process_projection_passes_through_non_array() {
        let input = json!({"not": "an array"});
        let out = project_process_rows(input.clone());
        assert_eq!(out, input);
    }

    #[test]
    fn explode_services_emits_name_scoped_paths() {
        let services =
            vec![json!({"name": "mesh-listener", "service_type": "mesh", "health": "healthy"})];
        let paths: Vec<String> = explode_services_by_name(&services)
            .into_iter()
            .map(|(p, _)| p)
            .collect();
        assert!(
            paths
                .iter()
                .any(|p| p == "substrate/kernel/services/mesh-listener/status")
        );
        assert!(
            paths
                .iter()
                .any(|p| p == "substrate/kernel/services/mesh-listener/cpu_percent")
        );
    }

    #[test]
    fn format_cpu_ms_chooses_appropriate_unit() {
        assert_eq!(format_cpu_ms(250), "250ms");
        assert_eq!(format_cpu_ms(4200), "4.20s");
        assert_eq!(format_cpu_ms(65_000), "1m05s");
    }
}
