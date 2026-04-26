//! Left-hand tree widget for the Explorer.
//!
//! Walks the substrate namespace by calling `substrate.list` with a
//! `{prefix, depth: 1}` request whenever the user expands a node.
//! Children are cached in [`Explorer::tree_children`](super::Explorer)
//! so navigation stays instant after the first fetch; the owning
//! Explorer also re-polls expanded prefixes on a slow tick so newly
//! appearing paths show up without the user re-clicking.

use eframe::egui;
use serde_json::Value;

use super::{Explorer, TreeNode, ACTIVITY_WINDOW};

/// The substrate root — children at depth-1 below this are
/// **node-ids** (`n-<6-hex>` BLAKE3 prefixes per the node-identity
/// gate). The Explorer's tree starts here; there is no synthetic
/// header above it. The `ui.heading("Substrate")` painted by
/// [`Explorer::show`](crate::explorer::Explorer::show) already
/// frames the panel.
pub const ROOT_PREFIX: &str = "substrate";

/// Render the left tree pane. Returns the path newly selected this
/// frame (if any) so the caller can swap subscriptions atomically.
///
/// Layout: each top-level row is one node in the mesh; expanding a
/// node reveals its substrate subtree (`sensor/`, `health/`, `meta/`,
/// `kernel/` for kernel-class nodes, etc.). Mesh-canonical paths
/// under `substrate/_derived/...` appear as a sibling top-level row
/// when populated.
pub fn paint(ui: &mut egui::Ui, ex: &mut Explorer) -> Option<String> {
    let mut newly_selected: Option<String> = None;
    let mut to_request: Vec<String> = Vec::new();

    // Auto-request the root listing if we don't have a cache entry
    // yet. Also keep the root marked expanded so the slow-tick
    // re-list fires for newly-arrived nodes.
    if !ex.tree_children.contains_key(ROOT_PREFIX) {
        to_request.push(ROOT_PREFIX.to_string());
    }
    ex.expanded.insert(ROOT_PREFIX.to_string());

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(msg) = &ex.backend_hint {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("⚠ backend")
                            .small()
                            .color(egui::Color32::from_rgb(220, 170, 90)),
                    )
                    .on_hover_text(msg);
                });
                ui.separator();
            }

            match ex.tree_children.get(ROOT_PREFIX).cloned() {
                Some(kids) if kids.is_empty() => {
                    ui.label(
                        egui::RichText::new("(no nodes registered yet)")
                            .italics()
                            .small()
                            .color(egui::Color32::from_rgb(140, 140, 150)),
                    );
                }
                Some(kids) => {
                    for child in kids {
                        let label = last_segment(&child.path);
                        let is_leaf = child.has_value && child.child_count == 0;
                        render_node(
                            ui,
                            ex,
                            &child.path,
                            label,
                            0,
                            is_leaf,
                            &mut newly_selected,
                            &mut to_request,
                        );
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("loading…")
                            .italics()
                            .small()
                            .color(egui::Color32::from_rgb(140, 140, 150)),
                    );
                }
            }
        });

    // After the borrow of ex is released, enqueue any list requests
    // gathered during the walk. We can't do this inside the walk
    // because `to_request.push(...)` needs `&mut Vec` while we're
    // holding `&mut Explorer`.
    for prefix in to_request {
        ex.queue_list(prefix);
    }

    newly_selected
}

/// Render a single tree row and, if `expanded`, its children.
///
/// * `prefix` — the prefix we asked the backend to list (`""` for root).
/// * `display` — the label to show for this row.
/// * `depth` — indentation depth.
/// * `is_leaf` — true when `substrate.list` reported this child with
///   `has_value: true && child_count == 0`. Leaves render without an
///   expand caret because expanding them would fire a
///   `substrate.list { prefix: <leaf-path> }` whose kernel-side
///   contract returns the leaf itself as a child (see
///   `substrate_service::list` and the test
///   `list_leaf_prefix_returns_itself`). Recursing into
///   `tree_children["<leaf>"] == [{path:"<leaf>",...}]` would render
///   the same row inside itself indefinitely and overflow the stack
///   on WASM.
#[allow(clippy::too_many_arguments)]
fn render_node(
    ui: &mut egui::Ui,
    ex: &mut Explorer,
    prefix: &str,
    display: &str,
    depth: usize,
    is_leaf: bool,
    newly_selected: &mut Option<String>,
    to_request: &mut Vec<String>,
) {
    let is_expanded = ex.expanded.contains(prefix);
    let is_selected = ex.selected.as_deref() == Some(prefix);
    let has_activity = path_is_active(ex, prefix);

    // Indent by depth.
    let indent = (depth as f32) * 14.0;

    ui.horizontal(|ui| {
        ui.add_space(indent);

        if is_leaf {
            // Pad to the same column width the caret button would
            // occupy so leaf rows stay aligned with their siblings.
            // The caret button is rendered with `frame(false)` so its
            // width is just the glyph; ~14 px matches "▸" in the
            // current theme.
            ui.add_space(14.0);
        } else {
            // Expand/collapse arrow. Clicking fires a list request if
            // we haven't cached children for this prefix yet.
            let arrow = if is_expanded { "▾" } else { "▸" };
            let arrow_resp = ui
                .add(egui::Button::new(egui::RichText::new(arrow).monospace()).frame(false));
            if arrow_resp.clicked() {
                if is_expanded {
                    ex.expanded.remove(prefix);
                } else {
                    ex.expanded.insert(prefix.to_string());
                    // Always re-request on expand — children may have
                    // changed since last time.
                    to_request.push(prefix.to_string());
                }
            }
        }

        // Activity dot (● if updated in the last ACTIVITY_WINDOW, ○ otherwise).
        let (dot, dot_color) = if has_activity {
            ("●", egui::Color32::from_rgb(110, 210, 160))
        } else {
            ("○", egui::Color32::from_rgb(90, 90, 100))
        };
        ui.label(
            egui::RichText::new(dot)
                .monospace()
                .small()
                .color(dot_color),
        );

        // Label / selectable. Top-level rows (depth 0 = node-ids)
        // render slightly bolder so the eye anchors on the node
        // boundary; deeper rows are plain monospace.
        let label_text = if depth == 0 {
            egui::RichText::new(display).monospace().strong()
        } else {
            egui::RichText::new(display).monospace()
        };
        let resp = ui.selectable_label(is_selected, label_text);
        if resp.clicked() {
            if !is_selected {
                *newly_selected = Some(prefix.to_string());
            }
            ex.selected = Some(prefix.to_string());
        }
    });

    if !is_expanded {
        return;
    }

    // Render cached children, if any. If we have no cache entry yet,
    // a list request has already been queued above.
    let children = ex.tree_children.get(prefix).cloned();
    match children {
        Some(kids) if kids.is_empty() => {
            ui.horizontal(|ui| {
                ui.add_space(indent + 24.0);
                ui.label(
                    egui::RichText::new("(empty)")
                        .italics()
                        .small()
                        .color(egui::Color32::from_rgb(140, 140, 150)),
                );
            });
        }
        Some(kids) => {
            for child in kids {
                // Defense in depth against the kernel's "list of a leaf
                // returns the leaf itself as a child" contract: skip any
                // child whose path equals our own prefix. Without this
                // guard the row would re-render itself inside itself.
                // The is_leaf branch in the parent's caret rendering
                // suppresses the expand affordance that triggers this
                // case in the first place; this is the second line.
                if child.path == prefix {
                    continue;
                }
                let child_label = last_segment(&child.path);
                let is_leaf = child.has_value && child.child_count == 0;
                render_node(
                    ui,
                    ex,
                    &child.path,
                    child_label,
                    depth + 1,
                    is_leaf,
                    newly_selected,
                    to_request,
                );
            }
        }
        None => {
            ui.horizontal(|ui| {
                ui.add_space(indent + 24.0);
                ui.label(
                    egui::RichText::new("loading…")
                        .italics()
                        .small()
                        .color(egui::Color32::from_rgb(140, 140, 150)),
                );
            });
        }
    }
}

/// Is this path currently "live"? (delta within the last window.)
fn path_is_active(ex: &Explorer, path: &str) -> bool {
    let Some(t) = ex.activity.get(path) else {
        return false;
    };
    t.elapsed() <= ACTIVITY_WINDOW
}

/// Last path segment used as a tree label (`substrate/sensor/mic` → `mic`).
fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Parse a `substrate.list` response into a Vec of children. Tolerant
/// of shape drift — returns an empty Vec when the response doesn't
/// match the expected envelope.
pub fn parse_list_response(v: &Value) -> Vec<TreeNode> {
    let Some(children) = v.get("children").and_then(|c| c.as_array()) else {
        return Vec::new();
    };
    children
        .iter()
        .filter_map(|c| {
            let path = c.get("path")?.as_str()?.to_string();
            let has_value = c
                .get("has_value")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            let child_count = c
                .get("child_count")
                .and_then(|n| n.as_u64())
                .unwrap_or(0);
            Some(TreeNode {
                path,
                has_value,
                child_count,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_list_response_happy_path() {
        let v = json!({
            "children": [
                { "path": "substrate/sensor/mic", "has_value": true, "child_count": 0 },
                { "path": "substrate/sensor/tof", "has_value": true, "child_count": 0 },
            ]
        });
        let kids = parse_list_response(&v);
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].path, "substrate/sensor/mic");
        assert!(kids[0].has_value);
    }

    #[test]
    fn parse_list_response_empty() {
        let v = json!({ "children": [] });
        assert!(parse_list_response(&v).is_empty());
    }

    #[test]
    fn parse_list_response_garbage_returns_empty() {
        let v = json!({ "oops": 1 });
        assert!(parse_list_response(&v).is_empty());
    }

    #[test]
    fn last_segment_basic() {
        assert_eq!(last_segment("substrate/sensor/mic"), "mic");
        assert_eq!(last_segment("root"), "root");
        assert_eq!(last_segment(""), "");
    }

    #[test]
    fn parse_list_response_preserves_leaf_self_reference() {
        // The kernel's `substrate.list` returns the prefix itself as the
        // sole child when the prefix is a leaf with a value (see
        // `substrate_service::list` and the test `list_leaf_prefix_returns_itself`
        // in clawft-kernel). The parser MUST preserve that shape — the
        // recursion guard lives in `render_node`, not here. If we filtered
        // self-references at parse time, callers that use `list` to ask
        // "is this a leaf?" would silently lose their answer.
        let v = json!({
            "children": [
                { "path": "substrate/n-bfc4cd/sensor/mic/pcm_chunk",
                  "has_value": true, "child_count": 0 }
            ]
        });
        let kids = parse_list_response(&v);
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].path, "substrate/n-bfc4cd/sensor/mic/pcm_chunk");
        assert!(kids[0].has_value);
        assert_eq!(kids[0].child_count, 0);
    }
}
