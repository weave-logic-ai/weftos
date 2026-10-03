//! WEFT-709 / ADR-078 W1 — geometric partition of a sparse/dense point cloud.
//!
//! Pure-geometry structure stage (no ML labels):
//! 1. Plane RANSAC → floor + walls as `WM_SURFACE`
//! 2. Euclidean cluster on residual points → `WM_OBJECT` (label `unknown`)
//! 3. Coarse free-space `WM_VOLUME` above the dominant floor plane
//!
//! All producers leave `vector: null` (ADR-088 / WEFT-709 default).

use serde_json::{Value, json};

/// 3D point in scene-local coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point3 {
    /// X
    pub x: f64,
    /// Y
    pub y: f64,
    /// Z
    pub z: f64,
}

impl Point3 {
    /// Construct.
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    fn sub(self, o: Point3) -> [f64; 3] {
        [self.x - o.x, self.y - o.y, self.z - o.z]
    }
}

/// Axis-aligned bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb3 {
    /// Min corner.
    pub min: [f64; 3],
    /// Max corner.
    pub max: [f64; 3],
}

impl Aabb3 {
    fn from_point(p: Point3) -> Self {
        Self {
            min: [p.x, p.y, p.z],
            max: [p.x, p.y, p.z],
        }
    }

    fn expand(&mut self, p: Point3) {
        self.min[0] = self.min[0].min(p.x);
        self.min[1] = self.min[1].min(p.y);
        self.min[2] = self.min[2].min(p.z);
        self.max[0] = self.max[0].max(p.x);
        self.max[1] = self.max[1].max(p.y);
        self.max[2] = self.max[2].max(p.z);
    }

    fn from_points(pts: &[Point3]) -> Option<Self> {
        let mut it = pts.iter().copied();
        let first = it.next()?;
        let mut a = Self::from_point(first);
        for p in it {
            a.expand(p);
        }
        Some(a)
    }

    fn to_json(self) -> Value {
        json!({ "min": self.min, "max": self.max })
    }

    fn volume(self) -> f64 {
        (self.max[0] - self.min[0]).max(0.0)
            * (self.max[1] - self.min[1]).max(0.0)
            * (self.max[2] - self.min[2]).max(0.0)
    }
}

/// Partition parameters (defaults tuned for room-scale COLMAP clouds).
#[derive(Debug, Clone)]
pub struct PartitionParams {
    /// RANSAC iterations per plane.
    pub ransac_iters: usize,
    /// Inlier distance threshold (scene units).
    pub plane_dist: f64,
    /// Minimum inliers to accept a plane.
    pub min_plane_inliers: usize,
    /// Max planes (floor + walls).
    pub max_planes: usize,
    /// Cluster cell size for grid-based Euclidean clustering.
    pub cluster_cell: f64,
    /// Minimum points per object cluster.
    pub min_cluster_pts: usize,
    /// Max object clusters to emit.
    pub max_objects: usize,
}

impl Default for PartitionParams {
    fn default() -> Self {
        Self {
            ransac_iters: 200,
            plane_dist: 0.05,
            min_plane_inliers: 20,
            max_planes: 6,
            cluster_cell: 0.25,
            min_cluster_pts: 8,
            max_objects: 64,
        }
    }
}

/// One extracted surface plane.
#[derive(Debug, Clone)]
pub struct SurfaceHit {
    /// Optional label (`floor`, `wall`, …).
    pub label: String,
    /// Unit normal.
    pub normal: [f64; 3],
    /// Point on plane.
    pub point: [f64; 3],
    /// Planar extent AABB.
    pub bound: Aabb3,
}

/// One unknown object blob.
#[derive(Debug, Clone)]
pub struct ObjectHit {
    /// Always `unknown` in W1.
    pub label: String,
    /// Covering AABB.
    pub bound: Aabb3,
    /// Point count that formed the cluster.
    pub point_count: usize,
}

/// Free / occupied volume.
#[derive(Debug, Clone)]
pub struct VolumeHit {
    /// `free` | `occupied` | …
    pub kind: String,
    /// Volume AABB.
    pub bound: Aabb3,
}

/// Full W1 partition result.
#[derive(Debug, Clone, Default)]
pub struct PartitionResult {
    /// Surfaces (floor/walls).
    pub surfaces: Vec<SurfaceHit>,
    /// Object AABBs.
    pub objects: Vec<ObjectHit>,
    /// Free-space (and optional occupied) volumes.
    pub volumes: Vec<VolumeHit>,
}

/// Run geometric partition on a point set.
pub fn partition_points(points: &[Point3], params: &PartitionParams) -> PartitionResult {
    if points.is_empty() {
        return PartitionResult::default();
    }

    let mut remaining: Vec<Point3> = points.to_vec();
    let mut surfaces = Vec::new();

    for plane_i in 0..params.max_planes {
        if remaining.len() < params.min_plane_inliers {
            break;
        }
        let Some((normal, point, inliers)) =
            ransac_plane(&remaining, params.ransac_iters, params.plane_dist)
        else {
            break;
        };
        if inliers.len() < params.min_plane_inliers {
            break;
        }

        let bound = Aabb3::from_points(&inliers).unwrap_or_else(|| Aabb3::from_point(point));
        let label = classify_plane(normal, plane_i == 0);
        surfaces.push(SurfaceHit {
            label,
            normal,
            point: [point.x, point.y, point.z],
            bound,
        });

        // Remove inliers (by spatial proximity to plane + original set membership).
        let inlier_set: std::collections::HashSet<(i64, i64, i64)> = inliers
            .iter()
            .map(|p| quantize(p, params.plane_dist * 0.5))
            .collect();
        remaining.retain(|p| {
            let d = plane_distance(*p, normal, point);
            d > params.plane_dist || !inlier_set.contains(&quantize(p, params.plane_dist * 0.5))
        });
        // Simpler residual: drop points within distance of this plane.
        remaining.retain(|p| plane_distance(*p, normal, point) > params.plane_dist);
    }

    let objects = cluster_objects(&remaining, params);
    let volumes = free_space_volumes(points, &surfaces, &objects);

    PartitionResult {
        surfaces,
        objects,
        volumes,
    }
}

/// Serialize partition into world_model JSON leaf records (`vector` always null).
pub fn partition_to_json_leaves(part: &PartitionResult) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let surfaces: Vec<Value> = part
        .surfaces
        .iter()
        .map(|s| {
            json!({
                "tag": "WM_SURFACE",
                "tag_u32": 0x5350_0011u32,
                "label": s.label,
                "normal": s.normal,
                "point": s.point,
                "bound": s.bound.to_json(),
                "vector": null,
            })
        })
        .collect();
    let objects: Vec<Value> = part
        .objects
        .iter()
        .map(|o| {
            json!({
                "tag": "WM_OBJECT",
                "tag_u32": 0x5350_0010u32,
                "label": o.label,
                "confidence": null,
                "bound": o.bound.to_json(),
                "point_count": o.point_count,
                "vector": null,
            })
        })
        .collect();
    let volumes: Vec<Value> = part
        .volumes
        .iter()
        .map(|v| {
            json!({
                "tag": "WM_VOLUME",
                "tag_u32": 0x5350_0012u32,
                "kind": v.kind,
                "bound": v.bound.to_json(),
                "vector": null,
            })
        })
        .collect();
    (surfaces, objects, volumes)
}

fn quantize(p: &Point3, cell: f64) -> (i64, i64, i64) {
    let c = cell.max(1e-9);
    (
        (p.x / c).round() as i64,
        (p.y / c).round() as i64,
        (p.z / c).round() as i64,
    )
}

fn plane_distance(p: Point3, normal: [f64; 3], origin: Point3) -> f64 {
    let d = -(normal[0] * origin.x + normal[1] * origin.y + normal[2] * origin.z);
    (normal[0] * p.x + normal[1] * p.y + normal[2] * p.z + d).abs()
}

fn classify_plane(normal: [f64; 3], first: bool) -> String {
    // Prefer Y-up; also accept Z-up.
    let abs = [normal[0].abs(), normal[1].abs(), normal[2].abs()];
    if abs[1] > 0.85 || abs[2] > 0.85 {
        if first {
            "floor".into()
        } else {
            "ceiling_or_floor".into()
        }
    } else {
        "wall".into()
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(mut n: [f64; 3]) -> Option<[f64; 3]> {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-12 {
        return None;
    }
    n[0] /= len;
    n[1] /= len;
    n[2] /= len;
    Some(n)
}

/// Deterministic LCG for RANSAC sample indices (no rand crate).
fn lcg_step(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1);
    *state
}

fn ransac_plane(
    points: &[Point3],
    iters: usize,
    dist_thresh: f64,
) -> Option<([f64; 3], Point3, Vec<Point3>)> {
    if points.len() < 3 {
        return None;
    }
    let mut best_normal = [0.0, 1.0, 0.0];
    let mut best_point = points[0];
    let mut best_inliers: Vec<Point3> = Vec::new();
    let mut seed = points.len() as u64 * 17 + 31;

    for _ in 0..iters {
        let i0 = (lcg_step(&mut seed) as usize) % points.len();
        let i1 = (lcg_step(&mut seed) as usize) % points.len();
        let i2 = (lcg_step(&mut seed) as usize) % points.len();
        if i0 == i1 || i1 == i2 || i0 == i2 {
            continue;
        }
        let p0 = points[i0];
        let p1 = points[i1];
        let p2 = points[i2];
        let v1 = p1.sub(p0);
        let v2 = p2.sub(p0);
        let Some(normal) = normalize(cross(v1, v2)) else {
            continue;
        };
        let mut inliers = Vec::new();
        for &p in points {
            if plane_distance(p, normal, p0) <= dist_thresh {
                inliers.push(p);
            }
        }
        if inliers.len() > best_inliers.len() {
            best_inliers = inliers;
            best_normal = normal;
            best_point = p0;
        }
    }

    if best_inliers.is_empty() {
        None
    } else {
        Some((best_normal, best_point, best_inliers))
    }
}

fn cluster_objects(points: &[Point3], params: &PartitionParams) -> Vec<ObjectHit> {
    if points.is_empty() {
        return Vec::new();
    }
    let cell = params.cluster_cell.max(1e-6);
    // Union-find over grid cells that contain points, linking 26-neighbors.
    use std::collections::HashMap;
    let mut cell_pts: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        let key = (
            (p.x / cell).floor() as i64,
            (p.y / cell).floor() as i64,
            (p.z / cell).floor() as i64,
        );
        cell_pts.entry(key).or_default().push(i);
    }
    let keys: Vec<_> = cell_pts.keys().copied().collect();
    let mut parent: HashMap<(i64, i64, i64), (i64, i64, i64)> =
        keys.iter().map(|&k| (k, k)).collect();

    fn find(
        parent: &mut HashMap<(i64, i64, i64), (i64, i64, i64)>,
        k: (i64, i64, i64),
    ) -> (i64, i64, i64) {
        let p = parent[&k];
        if p != k {
            let r = find(parent, p);
            parent.insert(k, r);
            r
        } else {
            k
        }
    }

    for &k in &keys {
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let n = (k.0 + dx, k.1 + dy, k.2 + dz);
                    if cell_pts.contains_key(&n) {
                        let ra = find(&mut parent, k);
                        let rb = find(&mut parent, n);
                        if ra != rb {
                            parent.insert(rb, ra);
                        }
                    }
                }
            }
        }
    }

    let mut groups: HashMap<(i64, i64, i64), Vec<Point3>> = HashMap::new();
    for &k in &keys {
        let root = find(&mut parent, k);
        for &idx in &cell_pts[&k] {
            groups.entry(root).or_default().push(points[idx]);
        }
    }

    let mut objects: Vec<ObjectHit> = groups
        .into_values()
        .filter(|pts| pts.len() >= params.min_cluster_pts)
        .filter_map(|pts| {
            let bound = Aabb3::from_points(&pts)?;
            Some(ObjectHit {
                label: "unknown".into(),
                bound,
                point_count: pts.len(),
            })
        })
        .collect();
    // Prefer larger clusters.
    objects.sort_by_key(|b| std::cmp::Reverse(b.point_count));
    objects.truncate(params.max_objects);
    objects
}

fn free_space_volumes(
    all_points: &[Point3],
    surfaces: &[SurfaceHit],
    objects: &[ObjectHit],
) -> Vec<VolumeHit> {
    let Some(scene) = Aabb3::from_points(all_points) else {
        return Vec::new();
    };
    let mut volumes = Vec::new();

    // Occupied: union of object AABBs as one coarse occupied volume if any.
    if let Some(occ) = objects.iter().map(|o| o.bound).reduce(|mut a, b| {
        a.min[0] = a.min[0].min(b.min[0]);
        a.min[1] = a.min[1].min(b.min[1]);
        a.min[2] = a.min[2].min(b.min[2]);
        a.max[0] = a.max[0].max(b.max[0]);
        a.max[1] = a.max[1].max(b.max[1]);
        a.max[2] = a.max[2].max(b.max[2]);
        a
    })
        && occ.volume() > 0.0 {
            volumes.push(VolumeHit {
                kind: "occupied".into(),
                bound: occ,
            });
        }

    // Free slab: from floor (if known) up to scene max, otherwise upper half of scene.
    let floor_y = surfaces
        .iter()
        .find(|s| s.label == "floor")
        .map(|s| s.bound.max[1])
        .or_else(|| {
            surfaces
                .iter()
                .find(|s| s.label.contains("floor"))
                .map(|s| s.bound.max[1])
        });

    let free = if let Some(fy) = floor_y {
        Aabb3 {
            min: [scene.min[0], fy, scene.min[2]],
            max: scene.max,
        }
    } else {
        let mid_y = (scene.min[1] + scene.max[1]) * 0.5;
        Aabb3 {
            min: [scene.min[0], mid_y, scene.min[2]],
            max: scene.max,
        }
    };
    if free.volume() > 0.0 {
        volumes.push(VolumeHit {
            kind: "free".into(),
            bound: free,
        });
    }
    volumes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room_cloud() -> Vec<Point3> {
        let mut pts = Vec::new();
        // Floor y=0
        for x in 0..20 {
            for z in 0..20 {
                pts.push(Point3::new(x as f64 * 0.1, 0.0, z as f64 * 0.1));
            }
        }
        // Wall z=0
        for x in 0..20 {
            for y in 0..15 {
                pts.push(Point3::new(x as f64 * 0.1, y as f64 * 0.1, 0.0));
            }
        }
        // Object blob around (1, 0.3, 1)
        for i in 0..30 {
            let t = i as f64 * 0.02;
            pts.push(Point3::new(1.0 + t, 0.3 + t * 0.5, 1.0 + t * 0.3));
        }
        pts
    }

    #[test]
    fn partition_finds_surface_and_object() {
        let pts = room_cloud();
        let part = partition_points(&pts, &PartitionParams::default());
        assert!(
            !part.surfaces.is_empty(),
            "expected at least one plane surface"
        );
        assert!(
            part.surfaces.iter().any(|s| s.label == "floor" || s.label == "wall"),
            "expected floor or wall label"
        );
        assert!(
            !part.objects.is_empty() || !part.volumes.is_empty(),
            "expected objects or free volume"
        );
        let (s, o, v) = partition_to_json_leaves(&part);
        assert_eq!(s.len(), part.surfaces.len());
        assert_eq!(o.len(), part.objects.len());
        assert_eq!(v.len(), part.volumes.len());
        // ADR-088: all vector fields null.
        for leaf in s.iter().chain(o.iter()).chain(v.iter()) {
            assert!(leaf["vector"].is_null());
        }
    }

    #[test]
    fn empty_cloud() {
        let part = partition_points(&[], &PartitionParams::default());
        assert!(part.surfaces.is_empty());
        assert!(part.objects.is_empty());
    }
}
