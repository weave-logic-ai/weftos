//! Project scoping for the console: which nodes, workloads and cogs belong to the project the
//! console was opened for. Read out of the fleet snapshot (`placement.project_id` on each placed
//! instance); pure and tested, drawn by the views.

use crate::fleet;
use serde_json::Value;
use std::collections::BTreeSet;

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The project's display name, if the snapshot lists projects (`projects: [{project_id|id, name}]`).
pub fn name(snap: &Value, project: &str) -> Option<String> {
    snap["projects"]
        .as_array()?
        .iter()
        .find(|p| ["project_id", "id", "ulid"].iter().any(|k| p[*k].as_str().is_some_and(|v| same(v, project))))
        .and_then(|p| p["name"].as_str())
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}

/// Node ids that host at least one instance of the project.
pub fn hosting_nodes(snap: &Value, project: &str) -> BTreeSet<String> {
    snap["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| fleet::instances(n).iter().any(|i| same(&i.project, project)))
        .filter_map(|n| n["node_id"].as_str().map(str::to_owned))
        .collect()
}

/// Workload names of the project's instances, across every node.
pub fn workloads(snap: &Value, project: &str) -> BTreeSet<String> {
    snap["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(fleet::instances)
        .filter(|i| same(&i.project, project))
        .map(|i| i.workload)
        .collect()
}

/// Whether a cog on the host is one of the project's workloads (`<id>`, `cog-<id>` or `cog:<id>`).
pub fn owns_cog(workloads: &BTreeSet<String>, cog_id: &str) -> bool {
    workloads.iter().any(|w| w == cog_id || w.strip_prefix("cog-").or_else(|| w.strip_prefix("cog:")) == Some(cog_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const P: &str = "01J8ZQ4W7K3M9N2P5R6T8V0XYZ";

    fn snap() -> Value {
        let inst = |w: &str, p: Option<&str>| json!({ "placement": { "instance_id": "i", "workload": w, "project_id": p } });
        json!({
            "projects": [{ "project_id": P, "name": "weftos" }, { "id": "OTHER", "name": "x" }],
            "nodes": [
                { "node_id": "n1", "instances": { "value": [inst("anomaly-detect", Some(P)), inst("cog-b", None)] } },
                { "node_id": "n2", "instances": { "value": [inst("c", Some("OTHER"))] } },
                { "node_id": "n3" }
            ]
        })
    }

    #[test]
    fn name_comes_from_the_snapshot() {
        assert_eq!(name(&snap(), P).as_deref(), Some("weftos"));
        assert_eq!(name(&snap(), &P.to_lowercase()).as_deref(), Some("weftos"));
        assert_eq!(name(&snap(), "ZZZ"), None);
        assert_eq!(name(&json!({}), P), None);
    }

    #[test]
    fn only_nodes_with_the_project_host_it() {
        assert_eq!(hosting_nodes(&snap(), P), BTreeSet::from(["n1".to_string()]));
        assert!(hosting_nodes(&snap(), "nope").is_empty());
    }

    #[test]
    fn project_workloads_and_cogs() {
        let w = workloads(&snap(), P);
        assert_eq!(w, BTreeSet::from(["anomaly-detect".to_string()]));
        assert!(owns_cog(&w, "anomaly-detect"));
        assert!(!owns_cog(&w, "cog-b"));
        assert!(owns_cog(&BTreeSet::from(["cog-x".to_string()]), "x"));
        assert!(!owns_cog(&BTreeSet::new(), "x"));
    }
}
