//! The connected node, the nodes under it, and the cogs and edge devices on each.

use crate::app::Manager;
use crate::node_tree::{self, Role, Twig};
use crate::style;
use eframe::egui;

impl Manager {
    pub(crate) fn tree_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.client.ensure_mesh(ctx);
        style::section_header(ui, "This node", "the node you connected to, the tailnet nodes under it, and what is attached to each");
        let host_url = self.client.s.host.clone();
        let (name, net, mesh, cogs, mesh_note) = {
            let sh = self.client.snapshot();
            let name = sh.net.as_ref().and_then(|r| r.as_ref().ok()).map(|n| n.node.clone()).unwrap_or_default();
            let net = sh.net.as_ref().and_then(|r| r.as_ref().ok()).cloned();
            let mesh_note = match sh.mesh.as_ref().and_then(|m| m.result.as_ref()) {
                Some(Err(e)) => Some(e.clone()),
                Some(Ok(m)) if m.scope != "mesh" && !m.reason.is_empty() => Some(m.reason.clone()),
                _ => None,
            };
            let mesh = sh.mesh.as_ref().and_then(|m| m.result.as_ref()).and_then(|r| r.as_ref().ok()).map(|m| m.nodes.clone()).unwrap_or_default();
            let cogs = sh.host.as_ref().and_then(|r| r.as_ref().ok()).map(|h| h.cogs.clone()).unwrap_or_default();
            (name, net, mesh, cogs, mesh_note)
        };
        if let Some(note) = mesh_note {
            ui.label(style::dim(ui, format!("Peer cogs: {note}. Tailnet peers from this node still list below.")));
        }
        let root = node_tree::tree(&host_url, &name, &cogs, net.as_ref(), &mesh);
        let mut jump: Option<String> = None;
        draw(ui, &root, 0, &mut jump);
        if let Some(url) = jump {
            self.attach_to(url);
        }
    }
}

fn draw(ui: &mut egui::Ui, twig: &Twig, depth: usize, jump: &mut Option<String>) {
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 16.0);
        let mark = match twig.role {
            Role::Root => "node",
            Role::Peer => "node",
            Role::Cog => "cog",
            Role::Edge => "edge",
        };
        ui.label(style::dim(ui, mark));
        ui.label(style::body(twig.label.as_str()).strong());
        if twig.role == Role::Root || twig.role == Role::Peer {
            ui.label(style::dim(ui, twig.reach.label()));
        }
        if !twig.note.is_empty() {
            ui.label(style::dim(ui, twig.note.as_str()));
        }
        if let Some(url) = &twig.connect_url
            && ui.small_button("Open").on_hover_text(format!("Attach the console to {url}. This node's mesh key stays here.")).clicked()
        {
            *jump = Some(url.clone());
        }
    });
    for child in &twig.children {
        draw(ui, child, depth + 1, jump);
    }
}
