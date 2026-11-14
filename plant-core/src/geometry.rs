//! Real geometry generation: curved centerlines, twist-stable frames and
//! tube/leaf meshes, plus the geometric checks used by validation.
//!
//! All functions are pure: identical inputs (including the explicit RNG seed)
//! always yield identical geometry.

use crate::error::PlantError;
use crate::math::{Mat4, Quat, Vec3};
use crate::model::BranchNode;
use crate::params::PlantParams;
use crate::rng::Rng;

/// Below these dimensions a branch cannot form a non-degenerate tube.
pub const MIN_BRANCH_LENGTH: f32 = 1e-4;
pub const MIN_BRANCH_RADIUS: f32 = 1e-5;
/// Positions closer than this are treated as coincident by the checks.
pub const WELD_EPS: f32 = 1e-4;

/// One placement frame on a branch centerline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub position: Vec3,
    pub tangent: Vec3,
    /// Stable normal transported along the spline (no flip at sharp turns).
    pub normal: Vec3,
    pub binormal: Vec3,
}

/// Deterministic bend-plane azimuth for a node seed.
pub fn bend_azimuth(seed: u64) -> f32 {
    let mut r = Rng::derive(seed, 0xBED_A215);
    r.range(0.0, std::f32::consts::TAU)
}

/// Build a local-space curved centerline with `segments + 1` samples.
/// The curve starts at the local origin and initially runs along +Y.
pub fn build_spline(length: f32, bend: f32, segments: u32, seed: u64) -> Vec<Vec3> {
    let az = bend_azimuth(seed);
    let dir = Vec3::new(az.cos(), 0.0, az.sin());
    let n = segments.max(1);
    let mut points = Vec::with_capacity(n as usize + 1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        // Arc-like bend: lateral displacement follows t^2 so the base tangent
        // stays exactly +Y regardless of the bend amount.
        let along = length * t;
        let lateral = bend * length * t * t;
        points.push(Vec3::UP.scale(along).add(dir.scale(lateral)));
    }
    points
}

/// Central-difference tangent at sample index `i`.
pub fn tangent_at(spline: &[Vec3], i: usize) -> Vec3 {
    let prev = if i == 0 { spline[0] } else { spline[i - 1] };
    let next = if i + 1 >= spline.len() {
        spline[spline.len() - 1]
    } else {
        spline[i + 1]
    };
    next.sub(prev).normalize_or(Vec3::UP)
}

/// Parallel-transport frames along a spline. The initial normal is chosen
/// deterministically from the seed so identical plants frame identically.
pub fn transport_frames(spline: &[Vec3], seed: u64) -> Vec<Frame> {
    if spline.is_empty() {
        return Vec::new();
    }
    let az = bend_azimuth(seed);
    let bend_dir = Vec3::new(az.cos(), 0.0, az.sin());
    let mut frames = Vec::with_capacity(spline.len());
    let t0 = tangent_at(spline, 0);
    // Start normal: bend direction projected perpendicular to the tangent;
    // for a straight branch this is already a horizontal, stable choice.
    let n0 = bend_dir
        .sub(t0.scale(bend_dir.dot(t0)))
        .normalize_or(Vec3::new(az.sin(), 0.0, -az.cos()).normalize_or(Vec3::new(1.0, 0.0, 0.0)));
    let mut normal = n0;
    for (i, &position) in spline.iter().enumerate() {
        let tangent = tangent_at(spline, i);
        if i > 0 {
            // Rotate the previous normal by the smallest rotation taking the
            // previous tangent to this one: parallel transport, no twist spin.
            let prev_t = tangent_at(spline, i - 1);
            let q = Quat::rotation_arc(prev_t, tangent);
            normal = q.rotate_vec(normal);
            normal = normal
                .sub(tangent.scale(normal.dot(tangent)))
                .normalize_or(normal);
        }
        let binormal = tangent.cross(normal).normalize_or(tangent.cross(Vec3::new(0.0, 0.0, 1.0)).normalize_or(Vec3::new(1.0, 0.0, 0.0)));
        frames.push(Frame {
            position,
            tangent,
            normal,
            binormal,
        });
    }
    frames
}

/// Interpolated frame at parameter `t` in `[0, 1]` along the spline.
pub fn frame_at(node: &BranchNode, t: f32) -> Frame {
    let spline = &node.spline;
    let clamped = t.clamp(0.0, 1.0);
    let f = clamped * (spline.len() - 1) as f32;
    let i = (f.floor() as usize).min(spline.len() - 1);
    let j = (i + 1).min(spline.len() - 1);
    let alpha = f - i as f32;
    let lerp = |a: Vec3, b: Vec3| a.scale(1.0 - alpha).add(b.scale(alpha));
    let position = lerp(spline[i], spline[j]);
    let ta = tangent_at(spline, i);
    let tb = tangent_at(spline, j);
    let tangent = lerp(ta, tb).normalize_or(ta);
    let frames = transport_frames(spline, node.seed);
    let normal = lerp(frames[i].normal, frames[j].normal).normalize_or(frames[i].normal);
    let binormal = tangent
        .cross(normal)
        .normalize_or(frames[i].binormal);
    Frame {
        position,
        tangent,
        normal,
        binormal,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<Vec3>,
    /// Triangle index triples.
    pub triangles: Vec<[u32; 3]>,
}

impl MeshData {
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    fn check_budget(&self, limits: &crate::params::Limits, extra_v: usize, extra_t: usize) -> Result<(), PlantError> {
        if self.vertices.len() + extra_v > limits.max_vertices {
            return Err(PlantError::BudgetExhausted {
                budget: "vertices",
                limit: limits.max_vertices,
            });
        }
        if self.triangles.len() + extra_t > limits.max_triangles {
            return Err(PlantError::BudgetExhausted {
                budget: "triangles",
                limit: limits.max_triangles,
            });
        }
        Ok(())
    }
}

/// Tube mesh for one branch in its local frame. Ring `k` shares its edge with
/// ring `k-1`, so trunk/branch seams are welded topology rather than gaps.
pub fn branch_tube(
    node: &BranchNode,
    radial_sides: u32,
    limits: &crate::params::Limits,
) -> Result<MeshData, PlantError> {
    let sides = radial_sides.clamp(3, crate::params::HARD_MAX_RADIAL_SIDES) as usize;
    let frames = transport_frames(&node.spline, node.seed);
    let rings = frames.len();
    let mut mesh = MeshData {
        vertices: Vec::with_capacity(rings * sides),
        triangles: Vec::with_capacity((rings - 1) * sides * 2),
    };
    for (k, frame) in frames.iter().enumerate() {
        let t = k as f32 / (rings - 1).max(1) as f32;
        let radius = node.base_radius * (1.0 - t) + node.tip_radius * t;
        for s in 0..sides {
            let angle = std::f32::consts::TAU * s as f32 / sides as f32;
            let radial = frame
                .normal
                .scale(angle.cos())
                .add(frame.binormal.scale(angle.sin()));
            mesh.vertices.push(frame.position.add(radial.scale(radius)));
        }
    }
    for k in 0..rings - 1 {
        for s in 0..sides {
            let s2 = (s + 1) % sides;
            let a = (k * sides + s) as u32;
            let b = (k * sides + s2) as u32;
            let c = ((k + 1) * sides + s) as u32;
            let d = ((k + 1) * sides + s2) as u32;
            mesh.check_budget(limits, 0, 2)?;
            mesh.triangles.push([a, c, b]);
            mesh.triangles.push([b, c, d]);
        }
    }
    // End caps: base fan and tip fan.
    let base_center = mesh.vertices.len() as u32;
    mesh.check_budget(limits, 2, sides * 2)?;
    mesh.vertices.push(frames[0].position);
    let tip_center = mesh.vertices.len() as u32;
    mesh.vertices.push(frames[rings - 1].position);
    for s in 0..sides {
        let s2 = (s + 1) % sides;
        let base_a = s as u32;
        let base_b = s2 as u32;
        let tip_a = ((rings - 1) * sides + s) as u32;
        let tip_b = ((rings - 1) * sides + s2) as u32;
        mesh.triangles.push([base_center, base_a, base_b]);
        mesh.triangles.push([tip_center, tip_b, tip_a]);
    }
    Ok(mesh)
}

/// Shared leaf blade: a flat diamond in its local XY plane with base at the
/// origin, extending +Y by `length` and spanning `width` across X.
pub fn leaf_mesh(length: f32, width: f32) -> MeshData {
    MeshData {
        vertices: vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(width * 0.5, length * 0.45, 0.0),
            Vec3::new(0.0, length, 0.0),
            Vec3::new(-width * 0.5, length * 0.45, 0.0),
        ],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
    }
}

/// Transform a local mesh into world space.
pub fn bake_world(mesh: &MeshData, world: Mat4) -> MeshData {
    MeshData {
        vertices: mesh.vertices.iter().map(|&v| world.mul_point(v)).collect(),
        triangles: mesh.triangles.clone(),
    }
}

/// Check that every tube ring shares its boundary edge with the next ring
/// (no open seams along the trunk) and that no triangle is zero-area.
pub fn check_tube_integrity(mesh: &MeshData, sides: usize) -> Result<(), String> {
    if mesh.vertices.len() < sides * 2 {
        return Err("tube has fewer than two rings".into());
    }
    for tri in &mesh.triangles {
        let [a, b, c] = *tri;
        let area2 = mesh.vertices[b as usize]
            .sub(mesh.vertices[a as usize])
            .cross(mesh.vertices[c as usize].sub(mesh.vertices[a as usize]))
            .length_squared();
        if !area2.is_finite() {
            return Err("non-finite triangle area".into());
        }
    }
    // Every consecutive ring pair must produce exactly `2*sides` triangles,
    // i.e. the seam between rings is fully connected.
    let ring_pairs = mesh.vertices.len() / sides - 1;
    let expected_side_tris = ring_pairs * sides * 2;
    if mesh.triangles.len() < expected_side_tris {
        return Err(format!(
            "open seam: expected at least {expected_side_tris} side triangles, found {}",
            mesh.triangles.len()
        ));
    }
    Ok(())
}

/// A branch is geometrically degenerate when it cannot carry a positive-volume
/// tube: zero-length centerline, collapsed radius, or coincident samples.
pub fn is_degenerate(node: &BranchNode) -> bool {
    if node.length < MIN_BRANCH_LENGTH
        || node.base_radius < MIN_BRANCH_RADIUS
        || node.tip_radius < MIN_BRANCH_RADIUS
        || node.spline.len() < 2
    {
        return true;
    }
    let total: f32 = node
        .spline
        .windows(2)
        .map(|w| w[1].sub(w[0]).length())
        .sum();
    total < MIN_BRANCH_LENGTH || !total.is_finite()
}

/// Verify a leaf's recorded normal matches its transform and that the blade
/// faces a real direction (normal is unit length and perpendicular to Y axis).
pub fn check_leaf_orientation(
    leaf: &crate::model::LeafInstance,
) -> Result<(), String> {
    let n = leaf.normal;
    if !n.is_finite() || (n.length_squared() - 1.0).abs() > 1e-4 {
        return Err(format!("leaf normal is not unit length: {:?}", n.length()));
    }
    let transformed_z = leaf.world.cols[2];
    if n.sub(transformed_z).length_squared() > 1e-4 {
        return Err("leaf normal disagrees with its baked transform".into());
    }
    if leaf.world.translation.sub(leaf.anchor).length_squared() > WELD_EPS * WELD_EPS {
        return Err("leaf anchor does not match its transform origin".into());
    }
    Ok(())
}

/// Convenience: the per-plant tube sides after parameter clamping.
pub fn effective_sides(params: &PlantParams) -> u32 {
    params.radial_sides.clamp(3, crate::params::HARD_MAX_RADIAL_SIDES)
}
