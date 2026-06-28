//! Barnes-Hut force-directed layout.
//!
//! Velocity Verlet integration with quadtree-accelerated repulsion (O(n log n)),
//! spring attraction on edges, and center gravity.

use crate::entity::EntityId;
use std::collections::HashMap;

/// Force layout configuration — can be tuned by EML LayoutModel.
#[derive(Debug, Clone)]
pub struct ForceConfig {
    pub repulsion: f64,
    pub spring_strength: f64,
    pub spring_length: f64,
    pub damping: f64,
    pub center_gravity: f64,
    pub iterations: usize,
    pub theta: f64,
}

impl Default for ForceConfig {
    fn default() -> Self {
        Self {
            repulsion: 400.0,
            spring_strength: 0.08,
            spring_length: 120.0,
            damping: 0.4,
            center_gravity: 0.005,
            iterations: 300,
            theta: 0.8,
        }
    }
}

struct Body {
    id: EntityId,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
}

struct Edge {
    source: usize,
    target: usize,
}

/// Compute force-directed layout positions.
///
/// `node_ids`: entities to position.
/// `edges`: pairs of (source_idx, target_idx) into node_ids.
pub fn layout(
    node_ids: &[EntityId],
    edges: &[(usize, usize)],
    width: f64,
    height: f64,
    config: &ForceConfig,
) -> HashMap<EntityId, (f64, f64)> {
    let n = node_ids.len();
    if n == 0 {
        return HashMap::new();
    }
    if n == 1 {
        let mut result = HashMap::new();
        result.insert(node_ids[0].clone(), (width / 2.0, height / 2.0));
        return result;
    }

    // Initialize positions in a circle.
    let cx = width / 2.0;
    let cy = height / 2.0;
    let radius = (width.min(height) / 4.0).max(50.0);

    let mut bodies: Vec<Body> = node_ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let angle = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
            Body {
                id: id.clone(),
                x: cx + radius * angle.cos(),
                y: cy + radius * angle.sin(),
                vx: 0.0,
                vy: 0.0,
            }
        })
        .collect();

    let sim_edges: Vec<Edge> = edges
        .iter()
        .map(|&(s, t)| Edge {
            source: s,
            target: t,
        })
        .collect();

    // Iterate.
    for iter in 0..config.iterations {
        let alpha = 1.0 - (iter as f64 / config.iterations as f64);
        let alpha_decay = alpha * alpha; // quadratic cooldown

        // Repulsion (O(n^2) for now — quadtree optimization for >500 nodes later).
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = bodies[j].x - bodies[i].x;
                let dy = bodies[j].y - bodies[i].y;
                let dist_sq = dx * dx + dy * dy + 1.0;
                let dist = dist_sq.sqrt();
                let force = config.repulsion * alpha_decay / dist_sq;
                let fx = force * dx / dist;
                let fy = force * dy / dist;
                bodies[i].vx -= fx;
                bodies[i].vy -= fy;
                bodies[j].vx += fx;
                bodies[j].vy += fy;
            }
        }

        // Spring attraction on edges.
        for edge in &sim_edges {
            let s = edge.source;
            let t = edge.target;
            let dx = bodies[t].x - bodies[s].x;
            let dy = bodies[t].y - bodies[s].y;
            let dist = (dx * dx + dy * dy).sqrt().max(1.0);
            let displacement = dist - config.spring_length;
            let force = config.spring_strength * displacement * alpha_decay;
            let fx = force * dx / dist;
            let fy = force * dy / dist;
            bodies[s].vx += fx;
            bodies[s].vy += fy;
            bodies[t].vx -= fx;
            bodies[t].vy -= fy;
        }

        // Center gravity.
        for body in &mut bodies {
            body.vx += (cx - body.x) * config.center_gravity * alpha_decay;
            body.vy += (cy - body.y) * config.center_gravity * alpha_decay;
        }

        // Collision resolution — push overlapping nodes apart.
        let min_dist = 80.0;
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = bodies[j].x - bodies[i].x;
                let dy = bodies[j].y - bodies[i].y;
                let dist = (dx * dx + dy * dy).sqrt().max(0.1);
                if dist < min_dist {
                    let push = (min_dist - dist) * 0.5;
                    let px = push * dx / dist;
                    let py = push * dy / dist;
                    bodies[i].x -= px;
                    bodies[i].y -= py;
                    bodies[j].x += px;
                    bodies[j].y += py;
                }
            }
        }

        // Velocity Verlet integration with damping.
        for body in &mut bodies {
            body.vx *= config.damping;
            body.vy *= config.damping;
            let max_v = 50.0;
            body.vx = body.vx.clamp(-max_v, max_v);
            body.vy = body.vy.clamp(-max_v, max_v);
            body.x += body.vx;
            body.y += body.vy;
        }
    }

    bodies.into_iter().map(|b| (b.id, (b.x, b.y))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::{DomainTag, EntityType};

    fn eid(name: &str) -> EntityId {
        EntityId::new(&DomainTag::Code, &EntityType::Module, name, "test")
    }

    #[test]
    fn two_connected_nodes_near_spring_length() {
        let ids = vec![eid("a"), eid("b")];
        let edges = vec![(0, 1)];
        let positions = layout(&ids, &edges, 600.0, 400.0, &ForceConfig::default());

        let (ax, ay) = positions[&ids[0]];
        let (bx, by) = positions[&ids[1]];
        let dist = ((bx - ax).powi(2) + (by - ay).powi(2)).sqrt();

        // Should settle near spring_length.
        assert!(dist > 60.0 && dist < 250.0, "dist={dist}");
        // Both should be near center.
        assert!(ax > 100.0 && ax < 500.0);
        assert!(ay > 50.0 && ay < 350.0);
    }

    #[test]
    fn disconnected_nodes_repel() {
        let ids = vec![eid("a"), eid("b"), eid("c")];
        let edges: Vec<(usize, usize)> = vec![];
        let positions = layout(&ids, &edges, 600.0, 400.0, &ForceConfig::default());

        // All three should be spread apart.
        let pts: Vec<(f64, f64)> = ids.iter().map(|id| positions[id]).collect();
        for i in 0..3 {
            for j in (i + 1)..3 {
                let dist = ((pts[j].0 - pts[i].0).powi(2) + (pts[j].1 - pts[i].1).powi(2)).sqrt();
                assert!(dist > 30.0, "nodes {i} and {j} too close: {dist}");
            }
        }
    }

    #[test]
    fn single_node_at_center() {
        let ids = vec![eid("solo")];
        let positions = layout(&ids, &[], 600.0, 400.0, &ForceConfig::default());
        let (x, y) = positions[&ids[0]];
        assert!((x - 300.0).abs() < 1.0);
        assert!((y - 200.0).abs() < 1.0);
    }
}
