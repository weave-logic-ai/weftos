//! Where a cog is on the mesh: the per-node view behind the detail panel and the catalog badges.
//!
//! The data is the connected host's `/mesh/cogs` answer (it fans out to its tailnet peers itself;
//! the console never polls peers). With no answer, or a host that predates the route, the view is
//! built from the connected host alone and says so (`Scope::ThisHostOnly`). Egui-independent.

use crate::client::{HostCog, HostStatus, MeshCogs};
use crate::sensor_link::{cog_state, CogState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The host asked its tailnet peers and at least one answered.
    Mesh,
    /// Only the connected host is known; the string says why.
    ThisHostOnly(String),
}

impl MeshNodeView {
    /// Stable identity for picking a node: the address, never the name (names are advisory and can
    /// collide or be spoofed); the connected host is always `"self"`.
    pub fn key(&self) -> String {
        if self.is_self { "self".into() } else { self.ip.clone() }
    }
}

#[derive(Clone, Debug)]
pub struct MeshNodeView {
    pub name: String,
    pub ip: String,
    pub is_self: bool,
    pub reachable: bool,
    pub error: Option<String>,
    pub cogs: Vec<HostCog>,
}

#[derive(Clone, Debug)]
pub struct MeshView {
    pub scope: Scope,
    pub nodes: Vec<MeshNodeView>,
}

/// Build the view from the mesh answer when there is one, else from the connected host alone.
/// `mesh` is `None` before the first answer, `Some(Err)` on failure (`"unsupported"` = old host).
pub fn mesh_view(mesh: Option<&Result<MeshCogs, String>>, host: Option<&HostStatus>) -> MeshView {
    let local = |why: &str| MeshView {
        scope: Scope::ThisHostOnly(why.to_string()),
        nodes: host
            .map(|h| vec![MeshNodeView { name: "this host".into(), ip: String::new(), is_self: true, reachable: true, error: None, cogs: h.cogs.clone() }])
            .unwrap_or_default(),
    };
    match mesh {
        Some(Ok(m)) if !m.nodes.is_empty() => MeshView {
            scope: if m.scope == "mesh" { Scope::Mesh } else { Scope::ThisHostOnly(if m.reason.is_empty() { "no peers answered".into() } else { m.reason.clone() }) },
            nodes: m
                .nodes
                .iter()
                .map(|n| MeshNodeView { name: n.node.clone(), ip: n.ip.clone(), is_self: n.is_self, reachable: n.reachable, error: n.error.clone(), cogs: n.cogs.clone() })
                .collect(),
        },
        Some(Err(e)) if e == "unsupported" => local("this host's cog-host does not serve /mesh/cogs (update it for the mesh view)"),
        Some(Err(e)) => local(&format!("mesh query failed: {e}")),
        _ => local("mesh not queried yet"),
    }
}

/// One cog on one node.
#[derive(Clone, Debug, PartialEq)]
pub struct CogOnNode {
    pub node: String,
    pub ip: String,
    pub is_self: bool,
    pub version: String,
    pub state: CogState,
    pub refusal: Option<String>,
    pub restarts: u32,
    pub uptime_s: Option<u64>,
    pub log_bytes: Option<u64>,
    pub log_age_s: Option<u64>,
}

/// Where one cog stands across the mesh.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CogMesh {
    pub on: Vec<CogOnNode>,
    /// Reachable nodes that do not have it.
    pub without: Vec<String>,
    /// Peers that did not answer, with the reason.
    pub unreachable: Vec<(String, String)>,
}

pub fn cog_mesh(view: &MeshView, id: &str) -> CogMesh {
    let mut out = CogMesh::default();
    for n in &view.nodes {
        if !n.reachable {
            out.unreachable.push((n.name.clone(), n.error.clone().unwrap_or_else(|| "no answer".into())));
        } else if let Some(c) = n.cogs.iter().find(|c| c.id == id) {
            out.on.push(CogOnNode {
                node: n.name.clone(),
                ip: n.ip.clone(),
                is_self: n.is_self,
                version: c.version.clone(),
                state: cog_state(c),
                refusal: c.licence_refusal.clone(),
                restarts: c.restarts,
                uptime_s: c.uptime_s,
                log_bytes: c.log_bytes,
                log_age_s: c.log_age_s,
            });
        } else {
            out.without.push(n.name.clone());
        }
    }
    out
}

impl CogMesh {
    /// Strongest state on any node (running beats the rest), `None` when installed nowhere.
    pub fn best_state(&self) -> Option<CogState> {
        self.on.iter().map(|n| n.state).min_by_key(|s| match s {
            CogState::Running => 0,
            CogState::Starting => 1,
            CogState::Refused => 2,
            CogState::Stopped => 3,
        })
    }
    pub fn running_nodes(&self) -> usize {
        self.on.iter().filter(|n| n.state == CogState::Running).count()
    }
}

pub fn fmt_bytes(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KB", b as f64 / 1024.0)
    } else {
        format!("{:.1} MB", b as f64 / 1048576.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::MeshNodeRaw;

    fn cog(id: &str, running: bool, v: &str) -> HostCog {
        HostCog { id: id.into(), version: v.into(), running, enabled: running, log_bytes: Some(2048), log_age_s: Some(3), ..Default::default() }
    }
    fn raw(scope: &str, nodes: Vec<MeshNodeRaw>) -> Result<MeshCogs, String> {
        Ok(MeshCogs { scope: scope.into(), reason: "r".into(), nodes })
    }
    fn node(name: &str, is_self: bool, reachable: bool, cogs: Vec<HostCog>) -> MeshNodeRaw {
        MeshNodeRaw { node: name.into(), is_self, reachable, error: (!reachable).then(|| "refused".into()), cogs, ..Default::default() }
    }

    #[test]
    fn mesh_answer_gives_per_node_version_state_and_log() {
        let m = raw("mesh", vec![
            node("cog0", true, true, vec![cog("ld2450-radar", true, "0.1.0")]),
            node("pi5", false, true, vec![cog("ld2450-radar", false, "0.0.9"), cog("rd-03e", true, "0.1.0")]),
            node("zero", false, true, vec![]),
            node("down", false, false, vec![]),
        ]);
        let v = mesh_view(Some(&m), None);
        assert_eq!(v.scope, Scope::Mesh);
        let c = cog_mesh(&v, "ld2450-radar");
        assert_eq!(c.on.len(), 2);
        assert_eq!(c.on[0].version, "0.1.0");
        assert_eq!(c.on[1].state, CogState::Stopped);
        assert_eq!(c.on[0].log_bytes, Some(2048));
        assert_eq!(c.without, vec!["zero".to_string()]);
        assert_eq!(c.unreachable, vec![("down".to_string(), "refused".to_string())]);
        assert_eq!(c.best_state(), Some(CogState::Running));
        assert_eq!(c.running_nodes(), 1);
        assert_eq!(cog_mesh(&v, "nope").best_state(), None);
    }

    #[test]
    fn old_host_or_failure_falls_back_to_this_host_labelled() {
        let h = HostStatus { cogs: vec![cog("a", true, "1")], ..Default::default() };
        let v = mesh_view(Some(&Err("unsupported".into())), Some(&h));
        assert!(matches!(&v.scope, Scope::ThisHostOnly(w) if w.contains("/mesh/cogs")));
        assert_eq!(cog_mesh(&v, "a").on.len(), 1);
        let v = mesh_view(Some(&Err("timeout".into())), Some(&h));
        assert!(matches!(&v.scope, Scope::ThisHostOnly(w) if w.contains("timeout")));
        let v = mesh_view(None, None);
        assert!(v.nodes.is_empty() && matches!(v.scope, Scope::ThisHostOnly(_)));
    }

    #[test]
    fn host_only_answer_keeps_its_reason() {
        let m = raw("this_host_only", vec![node("cog0", true, true, vec![])]);
        let v = mesh_view(Some(&m), None);
        assert_eq!(v.scope, Scope::ThisHostOnly("r".into()));
    }

    #[test]
    fn bytes_format() {
        assert_eq!(fmt_bytes(10), "10 B");
        assert_eq!(fmt_bytes(2048), "2.0 KB");
        assert_eq!(fmt_bytes(3 * 1048576), "3.0 MB");
    }
}
