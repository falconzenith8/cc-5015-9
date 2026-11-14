//! Deterministic hierarchical plant builder.
//!
//! The builder is fully iterative (a BFS queue, never recursion) and every RNG
//! stream is derived from an explicit node seed, so the same
//! [`PlantParams`] always produces the identical tree. Local transforms are
//! composed through the parent chain: a child's local origin is the sampled
//! attachment point on the parent's *curved* centerline, expressed in the
//! parent's local frame, which means scaled or bent parents still place every
//! child at a deterministic, exact connection point.

use crate::error::PlantError;
use crate::geometry::{
    bake_world, bend_azimuth, branch_tube, is_degenerate, leaf_mesh, transport_frames, Frame,
    MeshData, MIN_BRANCH_LENGTH, MIN_BRANCH_RADIUS,
};
use crate::math::{Mat4, Quat, Vec3};
use crate::model::{BranchNode, LeafInstance, NodeId, Plant, PruneReason, PruneRecord};
use crate::params::PlantParams;
use crate::rng::Rng;

/// Golden angle in radians: π(3 − √5) ≈ 2.3999634. The literal is the value
/// rounded to f32; clippy's precision lint is opted out of locally.
#[allow(clippy::excessive_precision)]
const GOLDEN_ANGLE: f32 = 2.3999634;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildStats {
    pub nodes_emitted: usize,
    pub leaves_emitted: usize,
    pub depth_reached: u32,
    pub pruned_degenerate: usize,
    pub pruned_depth: usize,
    pub pruned_node_budget: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshStats {
    pub vertices: usize,
    pub triangles: usize,
    pub tube_vertices: usize,
    pub tube_triangles: usize,
    pub leaf_vertices: usize,
    pub leaf_triangles: usize,
    /// Set when the geometry budget stopped further emission. The plant stays
    /// valid; remaining leaves are simply not instanced.
    pub vertex_budget_hit: bool,
    pub triangle_budget_hit: bool,
    pub leaf_budget_hit: bool,
}

/// Structural pass: hierarchy only, no render geometry.
pub fn build_plant(params: PlantParams) -> Result<(Plant, BuildStats), PlantError> {
    let params = params.validated()?;
    let limits = params.limits.clone();

    let mut nodes: Vec<BranchNode> = Vec::new();
    let mut pruned: Vec<PruneRecord> = Vec::new();
    let mut stats = BuildStats::default();

    let trunk_seed = params.seed;
    let trunk_spline = crate::geometry::build_spline(
        params.trunk_length,
        params.bend,
        params.segments,
        trunk_seed,
    );
    let trunk = BranchNode {
        id: 0,
        parent: None,
        depth: 0,
        local: Mat4::IDENTITY,
        world: Mat4::IDENTITY,
        length: params.trunk_length,
        base_radius: params.trunk_radius,
        tip_radius: params.trunk_radius * params.taper,
        spline: trunk_spline,
        seed: trunk_seed,
        children: Vec::new(),
        terminal: false,
    };
    nodes.push(trunk);
    stats.nodes_emitted = 1;
    stats.depth_reached = 0;

    // BFS over node ids; a plain index loop works because children are only
    // ever appended after their parent.
    let mut cursor: usize = 0;
    while cursor < nodes.len() {
        let parent_id = cursor;
        cursor += 1;
        let parent_depth = nodes[parent_id].depth;
        let parent_seed = nodes[parent_id].seed;
        let parent_length = nodes[parent_id].length;
        let parent_radius = nodes[parent_id].base_radius;

        if parent_depth + 1 >= limits.max_depth {
            if params.branch_count > 0 {
                for c in 0..params.branch_count {
                    pruned.push(PruneRecord {
                        parent_id,
                        child_index: c,
                        reason: PruneReason::DepthLimit,
                    });
                    stats.pruned_depth += 1;
                }
            }
            continue;
        }

        let mut rng = Rng::derive(parent_seed, 0xC11D_BACE);
        let layout_origin = rng.range(0.0, GOLDEN_ANGLE);
        let attach_jitter = rng.jitter(0.08);
        // Attachments snap to discrete spline samples so a child's local origin
        // is an exact point of the parent's curved centerline, never an
        // interpolated point that could drift off it.
        let sample_count = nodes[parent_id].spline.len();
        let last_attach = sample_count.saturating_sub(1).max(1);
        let first_attach = last_attach.min(1);

        for child_index in 0..params.branch_count {
            if nodes.len() >= limits.max_nodes {
                pruned.push(PruneRecord {
                    parent_id,
                    child_index,
                    reason: PruneReason::NodeBudget,
                });
                stats.pruned_node_budget += 1;
                continue;
            }

            let child_seed = mix_child_seed(parent_seed, child_index, params.seed);
            let mut child_rng = Rng::new(child_seed);

            // Attachment along the parent centerline: upper half of the branch,
            // golden-angle spread keeps multiple children from stacking. The
            // result is snapped to an actual spline sample index.
            let spread = 0.25 + 0.6 * ((child_index as f32 + 0.5) / params.branch_count as f32);
            let t = (spread + attach_jitter).clamp(0.15, 0.95);
            let sample_index = ((t * last_attach as f32).round() as usize)
                .clamp(first_attach, last_attach);
            let frame = sample_frame(&nodes[parent_id], sample_index, sample_count);

            let length_jitter = 1.0 + child_rng.jitter(params.length_jitter);
            let child_length = (parent_length * params.length_ratio * length_jitter)
                .max(MIN_BRANCH_LENGTH);
            let child_radius = (parent_radius * params.radius_ratio).max(MIN_BRANCH_RADIUS);
            let tip_radius = child_radius * params.taper;

            if child_length < MIN_BRANCH_LENGTH * 2.0
                || child_radius < MIN_BRANCH_RADIUS * 2.0
                || child_length * child_radius * child_radius < 1e-9
            {
                pruned.push(PruneRecord {
                    parent_id,
                    child_index,
                    reason: PruneReason::Degenerate,
                });
                stats.pruned_degenerate += 1;
                continue;
            }

            let azimuth = layout_origin
                + GOLDEN_ANGLE * child_index as f32
                + child_rng.jitter(params.angle_jitter);
            let angle = (params.branch_angle + child_rng.jitter(params.angle_jitter))
                .clamp(0.0, std::f32::consts::PI);

            // Radial departure direction built entirely in the parent frame,
            // so it follows the transported normal of a bent parent correctly.
            let radial = frame
                .normal
                .scale(azimuth.cos())
                .add(frame.binormal.scale(azimuth.sin()))
                .normalize_or(frame.normal);
            let direction = frame
                .tangent
                .scale(angle.cos())
                .add(radial.scale(angle.sin()))
                .normalize_or(frame.tangent);

            let rotation = Quat::rotation_arc(Vec3::UP, direction);
            // Local origin is the attachment point in parent-local coordinates;
            // uniform local scale stays 1 because size is carried explicitly in
            // length/radius and inherited through the transform chain.
            let local = Mat4::from_trs(frame.position, rotation, 1.0);
            let parent_world = nodes[parent_id].world;
            let world = parent_world.mul(local);

            let spline =
                crate::geometry::build_spline(child_length, params.bend, params.segments, child_seed);

            let id: NodeId = nodes.len();
            nodes.push(BranchNode {
                id,
                parent: Some(parent_id),
                depth: parent_depth + 1,
                local,
                world,
                length: child_length,
                base_radius: child_radius,
                tip_radius,
                spline,
                seed: child_seed,
                children: Vec::new(),
                terminal: false,
            });
            nodes[parent_id].children.push(id);
            stats.nodes_emitted += 1;
            stats.depth_reached = stats.depth_reached.max(parent_depth + 1);
        }
    }

    for node in &mut nodes {
        node.terminal = node.children.is_empty();
    }

    let plant = Plant {
        params,
        nodes,
        leaves: Vec::new(),
        pruned,
    };
    Ok((plant, stats))
}

fn sample_frame(node: &crate::model::BranchNode, index: usize, sample_count: usize) -> Frame {
    let frames = transport_frames(&node.spline, node.seed);
    frames[index.min(sample_count - 1)]
}

/// Independent, collision-resistant child seed. Mixing in the root seed keeps
/// identical subtrees across different trees distinguishable while remaining
/// a pure function of the parent/child identity.
fn mix_child_seed(parent_seed: u64, child_index: u32, root_seed: u64) -> u64 {
    let mut z = parent_seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(child_index as u64)
        .wrapping_add(root_seed ^ 0xD1B5_4A92_7E4D_AE83);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Geometry pass: tube meshes per node plus instanced leaves, all bounded by
/// the vertex/triangle/leaf budgets. Returns per-node baked world meshes so the
/// renderer never has to walk the hierarchy to draw.
pub fn build_meshes(plant: &Plant) -> Result<(Vec<MeshData>, MeshStats), PlantError> {
    let limits = &plant.params.limits;
    let sides = crate::geometry::effective_sides(&plant.params);
    let mut meshes: Vec<MeshData> = Vec::with_capacity(plant.nodes.len());
    let mut stats = MeshStats::default();

    for node in &plant.nodes {
        if is_degenerate(node) {
            // Degenerate nodes are kept in the hierarchy for editability but
            // produce no geometry; validation confirms they never slipped in.
            meshes.push(MeshData {
                vertices: Vec::new(),
                triangles: Vec::new(),
            });
            continue;
        }
        let local_mesh = branch_tube(node, sides, limits)?;
        let world_mesh = bake_world(&local_mesh, node.world);
        let v = world_mesh.vertex_count();
        let t = world_mesh.triangle_count();
        if stats.vertices + v > limits.max_vertices {
            stats.vertex_budget_hit = true;
            meshes.push(MeshData {
                vertices: Vec::new(),
                triangles: Vec::new(),
            });
            continue;
        }
        if stats.triangles + t > limits.max_triangles {
            stats.triangle_budget_hit = true;
            meshes.push(MeshData {
                vertices: Vec::new(),
                triangles: Vec::new(),
            });
            continue;
        }
        stats.vertices += v;
        stats.triangles += t;
        stats.tube_vertices += v;
        stats.tube_triangles += t;
        meshes.push(world_mesh);
    }
    Ok((meshes, stats))
}

/// Instancing pass: one shared leaf blade, many explicit instance transforms.
/// Structure (the tree) and rendering (instances) are never fused.
pub fn build_leaves(plant: &Plant) -> (Vec<LeafInstance>, MeshStats) {
    let mut leaves: Vec<LeafInstance> = Vec::new();
    let mut stats = MeshStats::default();
    if !plant.params.leaves.enabled {
        return (leaves, stats);
    }
    let lp = &plant.params.leaves;
    let limits = &plant.params.limits;
    let per_leaf_v = 4usize;
    let per_leaf_t = 2usize;

    for node in &plant.nodes {
        if is_degenerate(node) {
            continue;
        }
        let cluster_size = if node.terminal { lp.per_tip } else { 0 };
        if cluster_size == 0 {
            continue;
        }
        let frames = transport_frames(&node.spline, node.seed);
        let tip = frames[frames.len() - 1];
        let mut rng = Rng::derive(node.seed, 0x1EAF_C155);
        for i in 0..cluster_size {
            if leaves.len() >= limits.max_leaves {
                stats.leaf_budget_hit = true;
                break;
            }
            if stats.vertices + per_leaf_v > limits.max_vertices {
                stats.vertex_budget_hit = true;
                break;
            }
            if stats.triangles + per_leaf_t > limits.max_triangles {
                stats.triangle_budget_hit = true;
                break;
            }
            let az = bend_azimuth(node.seed)
                + GOLDEN_ANGLE * i as f32
                + rng.jitter(0.3);
            let radial = tip
                .normal
                .scale(az.cos())
                .add(tip.binormal.scale(az.sin()))
                .normalize_or(tip.normal);
            // Lift blends the branch tangent with the world-up direction so
            // leaves face outward/up rather than pointing along the shoot.
            let lifted = tip
                .tangent
                .scale(1.0 - lp.lift)
                .add(Vec3::UP.scale(lp.lift))
                .add(radial.scale(0.35))
                .normalize_or(tip.tangent);
            let roll = rng.range(0.0, std::f32::consts::TAU);
            let dir_q = Quat::rotation_arc(Vec3::UP, lifted);
            let roll_q = Quat::axis_angle(Vec3::UP, roll);
            let rotation = dir_q.mul(roll_q);
            // Anchor sits slightly outside the tip ring to avoid z-fighting
            // with the branch cap.
            let local_anchor = tip.position.add(tip.tangent.scale(node.tip_radius * 1.05));
            let anchor = node.world.mul_point(local_anchor);
            let local = Mat4::from_trs(local_anchor, rotation, 1.0);
            let world = node.world.mul(local);
            let normal = world.cols[2].normalize_or(Vec3::UP);
            leaves.push(LeafInstance {
                node_id: node.id,
                world,
                anchor,
                normal,
            });
            stats.vertices += per_leaf_v;
            stats.triangles += per_leaf_t;
            stats.leaf_vertices += per_leaf_v;
            stats.leaf_triangles += per_leaf_t;
        }
    }
    (leaves, stats)
}

/// The shared blade mesh referenced by every leaf instance.
pub fn shared_leaf_mesh(params: &PlantParams) -> MeshData {
    leaf_mesh(params.leaves.length, params.leaves.width)
}

/// Merge stats from the two geometry passes.
pub fn merge_mesh_stats(a: &MeshStats, b: &MeshStats) -> MeshStats {
    MeshStats {
        vertices: a.vertices + b.vertices,
        triangles: a.triangles + b.triangles,
        tube_vertices: a.tube_vertices + b.tube_vertices,
        tube_triangles: a.tube_triangles + b.tube_triangles,
        leaf_vertices: a.leaf_vertices + b.leaf_vertices,
        leaf_triangles: a.leaf_triangles + b.leaf_triangles,
        vertex_budget_hit: a.vertex_budget_hit || b.vertex_budget_hit,
        triangle_budget_hit: a.triangle_budget_hit || b.triangle_budget_hit,
        leaf_budget_hit: a.leaf_budget_hit || b.leaf_budget_hit,
    }
}
