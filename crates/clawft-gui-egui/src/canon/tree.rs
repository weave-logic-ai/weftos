//! `ui://tree` — hierarchical disclosure. ADR-001 row 15.
//!
//! Wraps `egui::CollapsingHeader` and keeps the open-set in egui's
//! persistent memory keyed by the tree's root id.

use std::borrow::Cow;
use std::collections::HashSet;

use eframe::egui;

use super::CanonWidget;
use super::response::CanonResponse;
use super::types::{Affordance, Confidence, IdentityUri, MutationAxis, Tooltip, VariantId};

const IDENTITY: &str = "ui://tree";

static AFFORDANCES: &[Affordance] = &[
    Affordance {
        name: Cow::Borrowed("expand"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("collapse"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("select-node"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: false,
    },
    Affordance {
        name: Cow::Borrowed("reorder"),
        verb: Cow::Borrowed("wsp.update"),
        actors: &[],
        args_schema: None,
        reorderable: true,
    },
];

static MUTATION_AXES: &[MutationAxis] = &[
    MutationAxis::new("indent"),
    MutationAxis::new("icon-set"),
    MutationAxis::new("default-open-depth"),
    MutationAxis::new("show-count-badges"),
];

/// The recursive tree structure callers pass in. Keep this small and
/// renderer-agnostic — it is not the kernel's authoritative tree type.
#[derive(Clone, Debug)]
pub enum TreeNode {
    Leaf(String),
    Branch {
        label: String,
        children: Vec<TreeNode>,
    },
}

impl TreeNode {
    pub fn leaf(s: impl Into<String>) -> Self {
        TreeNode::Leaf(s.into())
    }

    pub fn branch(label: impl Into<String>, children: Vec<TreeNode>) -> Self {
        TreeNode::Branch {
            label: label.into(),
            children,
        }
    }

    fn label(&self) -> &str {
        match self {
            TreeNode::Leaf(s) => s.as_str(),
            TreeNode::Branch { label, .. } => label.as_str(),
        }
    }
}

/// Per-frame tree outcome. `clicked_label` is the last node whose label
/// was clicked this frame (leaf labels are select targets; branch headers
/// fire expand/collapse, which egui handles internally).
#[derive(Clone, Debug, Default)]
pub struct TreeOutcome {
    pub clicked_label: Option<String>,
    pub expanded_label: Option<String>,
    pub collapsed_label: Option<String>,
}

pub struct Tree<'a> {
    id_source: egui::Id,
    root: Option<&'a TreeNode>,
    default_open_depth: u32,
    tooltip: Option<Tooltip>,
    variant: VariantId,
}

impl<'a> Tree<'a> {
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            id_source: egui::Id::new(("canon.tree", id_source)),
            root: None,
            default_open_depth: 1,
            tooltip: None,
            variant: 0,
        }
    }

    pub fn root(mut self, root: &'a TreeNode) -> Self {
        self.root = Some(root);
        self
    }

    pub fn default_open_depth(mut self, d: u32) -> Self {
        self.default_open_depth = d;
        self
    }

    pub fn tooltip(mut self, text: impl Into<Tooltip>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn variant(mut self, variant: VariantId) -> Self {
        self.variant = variant;
        self
    }

    /// Show the tree and return the per-frame outcome alongside the
    /// canon response.
    pub fn show_with_outcome(self, ui: &mut egui::Ui) -> (CanonResponse, TreeOutcome) {
        let id = self.id_source;
        let variant = self.variant;
        let tooltip = self.tooltip.clone();
        let root = self.root;
        let default_open_depth = self.default_open_depth;

        // Open-set lives in egui memory under our root id. Egui's
        // CollapsingHeader already persists per-header state, but keeping
        // a parallel set here gives us expand/collapse attribution in
        // the TreeOutcome and mirrors the session-5 contract.
        let open_key = egui::Id::new(("canon.tree.open_set", id));
        let prior_open: HashSet<String> = ui.ctx().memory_mut(|m| {
            m.data
                .get_persisted_mut_or_default::<HashSet<String>>(open_key)
                .clone()
        });

        let mut outcome = TreeOutcome::default();
        let mut next_open: HashSet<String> = prior_open.clone();

        let inner = ui.scope(|ui| {
            if let Some(node) = root {
                render_node(
                    ui,
                    node,
                    0,
                    default_open_depth,
                    &prior_open,
                    &mut next_open,
                    &mut outcome,
                );
            }
        });

        // Flush the updated open-set back into memory.
        ui.ctx().memory_mut(|m| {
            let slot = m
                .data
                .get_persisted_mut_or_default::<HashSet<String>>(open_key);
            *slot = next_open;
        });

        let mut resp = inner.response;
        if let Some(tt) = &tooltip {
            resp = resp.on_hover_text(tt.as_ref());
        }

        let chosen: Option<&'static str> = if outcome.expanded_label.is_some() {
            Some("expand")
        } else if outcome.collapsed_label.is_some() {
            Some("collapse")
        } else if outcome.clicked_label.is_some() {
            Some("select-node")
        } else {
            None
        };

        let canon = CanonResponse::from_egui(resp, Cow::Borrowed(IDENTITY), variant, chosen)
            .with_id_hint(id);
        (canon, outcome)
    }
}

fn render_node(
    ui: &mut egui::Ui,
    node: &TreeNode,
    depth: u32,
    default_open_depth: u32,
    prior_open: &HashSet<String>,
    next_open: &mut HashSet<String>,
    outcome: &mut TreeOutcome,
) {
    match node {
        TreeNode::Leaf(label) => {
            let resp = ui.selectable_label(false, label);
            if resp.clicked() {
                outcome.clicked_label = Some(label.clone());
            }
        }
        TreeNode::Branch { label, children } => {
            let was_open = prior_open.contains(label);
            let default_open = was_open || depth < default_open_depth;

            let header = egui::CollapsingHeader::new(label)
                .id_salt(("canon.tree.node", label.as_str(), depth))
                .default_open(default_open);
            let header_resp = header.show(ui, |ui| {
                for child in children {
                    render_node(
                        ui,
                        child,
                        depth + 1,
                        default_open_depth,
                        prior_open,
                        next_open,
                        outcome,
                    );
                }
            });

            // Detect transitions via the returned open state.
            let now_open = header_resp.openness > 0.5;
            if now_open {
                next_open.insert(label.clone());
                if !was_open {
                    outcome.expanded_label = Some(label.clone());
                }
            } else {
                next_open.remove(label);
                if was_open {
                    outcome.collapsed_label = Some(label.clone());
                }
            }

            if header_resp.header_response.clicked() && !was_open && !now_open {
                outcome.clicked_label = Some(label.clone());
            }
        }
    }
    let _ = node.label(); // silence unused accessor in release builds
}

impl<'a> CanonWidget for Tree<'a> {
    fn id(&self) -> egui::Id {
        self.id_source
    }

    fn identity_uri(&self) -> IdentityUri {
        Cow::Borrowed(IDENTITY)
    }

    fn affordances(&self) -> &[Affordance] {
        AFFORDANCES
    }

    fn confidence(&self) -> Confidence {
        Confidence::deterministic()
    }

    fn variant_id(&self) -> VariantId {
        self.variant
    }

    fn mutation_axes(&self) -> &[MutationAxis] {
        MUTATION_AXES
    }

    fn tooltip(&self) -> Option<&Tooltip> {
        self.tooltip.as_ref()
    }

    fn show(self, ui: &mut egui::Ui) -> CanonResponse {
        self.show_with_outcome(ui).0
    }
}
