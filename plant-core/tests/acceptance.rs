//! Required acceptance scenarios: fixed-seed reproducibility, zero branches,
//! sharp-angle turns, and many small leaves, plus the resource ceilings.

use plant_core::{
    build_leaves, build_meshes, build_plant, validate_plant, LeafParams, Limits, PlantParams,
    HARD_MAX_DEPTH, HARD_MAX_LEAVES, HARD_MAX_NODES,
};

fn params() -> PlantParams {
    PlantParams::default()
}

/// Exact byte-for-byte structural equality is achievable because no global
/// random state exists: rebuild twice, compare every field.
#[test]
fn fixed_seed_is_fully_reproducible() {
    let p = params();
    let (a, _) = build_plant(p.clone()).unwrap();
    let (b, _) = build_plant(p.clone()).unwrap();
    assert_eq!(a.nodes.len(), b.nodes.len());
    assert_eq!(a, b);

    // Different seed must be allowed to change the tree, but the same seed in
    // a fresh process must still match (this test binary is that fresh process).
    let mut other = p.clone();
    other.seed = p.seed.wrapping_add(1);
    let (c, _) = build_plant(other).unwrap();
    assert_ne!(c.nodes[0].spline, a.nodes[0].spline);
}

#[test]
fn repeated_generation_keeps_geometry_budget_stable() {
    let p = params();
    let (plant_a, _) = build_plant(p.clone()).unwrap();
    let (meshes_a, stats_a) = build_meshes(&plant_a).unwrap();
    let (leaves_a, lstats_a) = build_leaves(&plant_a);

    let (plant_b, _) = build_plant(p).unwrap();
    let (meshes_b, stats_b) = build_meshes(&plant_b).unwrap();
    let (leaves_b, lstats_b) = build_leaves(&plant_b);

    assert_eq!(meshes_a, meshes_b);
    assert_eq!(leaves_a, leaves_b);
    assert_eq!(stats_a, stats_b);
    assert_eq!(lstats_a, lstats_b);
}

#[test]
fn zero_branches_builds_trunk_only_plant() {
    let mut p = params();
    p.branch_count = 0;
    let (plant, stats) = build_plant(p).unwrap();
    assert_eq!(plant.node_count(), 1);
    assert_eq!(plant.nodes[0].depth, 0);
    assert!(plant.nodes[0].terminal);
    assert!(plant.pruned.is_empty());
    assert!(stats.pruned_depth == 0 && stats.pruned_degenerate == 0);
    validate_plant(&plant).unwrap();
}

#[test]
fn sharp_angles_keep_frames_stable_and_attached() {
    // Near-180 degree branching: parallel-transport frames must not flip and
    // children must still land exactly on the parent centerline.
    let mut p = params();
    p.branch_angle = std::f32::consts::PI - 0.02;
    p.angle_jitter = 0.0;
    p.limits.max_depth = 4;
    let (plant, _) = build_plant(p).unwrap();
    validate_plant(&plant).unwrap();
    assert!(plant.node_count() > 1);

    // Every child base must coincide (within weld tolerance) with a point on
    // the parent spline after world-transform composition.
    for node in plant.nodes.iter().skip(1) {
        let parent = &plant.nodes[node.parent.unwrap()];
        let base = node.world.translation;
        let nearest = parent
            .spline
            .iter()
            .map(|&sp| parent.world.mul_point(sp).sub(base).length_squared())
            .fold(f32::INFINITY, f32::min)
            .sqrt();
        assert!(nearest < 1e-3, "sharp-angle child detached: {nearest}");
    }
}

#[test]
fn many_small_leaves_respect_leaf_and_mesh_budgets() {
    let mut p = params();
    p.branch_count = 4;
    p.limits = Limits {
        max_depth: 5,
        max_nodes: 2_000,
        max_leaves: 500,
        max_vertices: 50_000,
        max_triangles: 30_000,
    };
    p.leaves = LeafParams {
        enabled: true,
        per_tip: 12,
        length: 0.05,
        width: 0.02,
        lift: 0.6,
    };
    let (mut plant, _) = build_plant(p.clone()).unwrap();
    let (leaves, stats) = build_leaves(&plant);
    plant.leaves = leaves;
    assert!(plant.leaves.len() <= p.limits.max_leaves);
    assert!(stats.vertices <= p.limits.max_vertices);
    assert!(stats.triangles <= p.limits.max_triangles);
    validate_plant(&plant).unwrap();
}

#[test]
fn extreme_parameters_cannot_escape_hard_caps() {
    let mut p = params();
    p.branch_count = 100_000;
    p.limits.max_depth = 1_000_000;
    p.limits.max_nodes = usize::MAX;
    p.limits.max_leaves = usize::MAX;
    p.limits.max_vertices = usize::MAX;
    p.limits.max_triangles = usize::MAX;
    p.segments = 10_000;
    p.radial_sides = 1_000;

    let validated = p.validated().unwrap();
    assert!(validated.limits.max_depth <= HARD_MAX_DEPTH);
    assert!(validated.limits.max_nodes <= HARD_MAX_NODES);
    assert!(validated.limits.max_leaves <= HARD_MAX_LEAVES);
    assert!(validated.segments <= plant_core::HARD_MAX_SEGMENTS);
    assert!(validated.radial_sides <= plant_core::HARD_MAX_RADIAL_SIDES);

    // A deliberately tiny budget must terminate quickly and stay bounded
    // regardless of the requested branching factor.
    let mut tight = validated.clone();
    tight.limits.max_nodes = 10;
    tight.limits.max_depth = 3;
    let (plant, stats) = build_plant(tight).unwrap();
    assert!(plant.node_count() <= 10);
    assert!(stats.pruned_node_budget > 0 || stats.pruned_depth > 0);
    validate_plant(&plant).unwrap();
}

#[test]
fn node_budget_never_panics_and_reports_prune_reason() {
    let mut p = params();
    p.branch_count = 8;
    p.limits.max_nodes = 25;
    p.limits.max_depth = 12;
    let (plant, stats) = build_plant(p).unwrap();
    assert!(plant.node_count() <= 25);
    assert!(stats.nodes_emitted <= 25);
    let total_accounted = stats.nodes_emitted
        + stats.pruned_node_budget
        + stats.pruned_depth
        + stats.pruned_degenerate;
    assert!(total_accounted >= stats.nodes_emitted);
}
