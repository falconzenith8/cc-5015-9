//! Hierarchy composition, geometric integrity and export round-trip tests.

use plant_core::{
    build_leaves, build_meshes, build_plant, export, geometry, validate_plant, PlantParams,
};

#[test]
fn local_transforms_compose_under_scaled_and_bent_parents() {
    let p = PlantParams {
        bend: 0.35, // strongly curved parents
        branch_count: 3,
        limits: plant_core::Limits {
            max_depth: 5,
            ..Default::default()
        },
        ..PlantParams::default()
    };
    let (plant, _) = build_plant(p).unwrap();

    // Recompute every world matrix independently from the local chain and
    // compare with the stored one.
    for node in &plant.nodes {
        let expected = match node.parent {
            None => plant_core::math::Mat4::IDENTITY,
            Some(pid) => {
                let parent = &plant.nodes[pid];
                parent.world.mul(node.local)
            }
        };
        let t_diff = expected.translation.sub(node.world.translation).length();
        assert!(t_diff < 1e-5, "world translation mismatch at node {}", node.id);
        assert!((expected.scale - node.world.scale).abs() < 1e-5);
    }

    // Children must attach to the parent's *curved* spline, not its origin.
    for node in plant.nodes.iter().skip(1) {
        let parent = &plant.nodes[node.parent.unwrap()];
        let base = node.world.translation;
        let on_spline = parent
            .spline
            .iter()
            .any(|&sp| parent.world.mul_point(sp).sub(base).length() < 1e-3);
        assert!(on_spline, "node {} not attached to parent spline", node.id);
        assert_ne!(base, parent.world.translation, "child collapsed onto parent base");
    }
}

#[test]
fn tube_seams_are_welded_and_triangles_have_area() {
    let p = PlantParams::default();
    let (plant, _) = build_plant(p).unwrap();
    let (meshes, _) = build_meshes(&plant).unwrap();
    let sides = geometry::effective_sides(&plant.params) as usize;
    for (node, mesh) in plant.nodes.iter().zip(meshes.iter()) {
        geometry::check_tube_integrity(mesh, sides)
            .unwrap_or_else(|e| panic!("node {}: {e}", node.id));
        for tri in &mesh.triangles {
            let a = mesh.vertices[tri[0] as usize];
            let b = mesh.vertices[tri[1] as usize];
            let c = mesh.vertices[tri[2] as usize];
            let area = b.sub(a).cross(c.sub(a)).length_squared();
            assert!(area > 0.0 && area.is_finite(), "zero-area triangle on node {}", node.id);
        }
    }
}

#[test]
fn leaf_normals_agree_with_their_instance_transforms() {
    let p = PlantParams::default();
    let (mut plant, _) = build_plant(p).unwrap();
    let (leaves, _) = build_leaves(&plant);
    assert!(!leaves.is_empty(), "default plant should have leaves");
    plant.leaves = leaves;
    for leaf in &plant.leaves {
        geometry::check_leaf_orientation(leaf).unwrap();
    }
    validate_plant(&plant).unwrap();
}

#[test]
fn hierarchy_json_round_trips_exactly() {
    let p = PlantParams {
        seed: 0xDEAD_BEEF_CAFE_BABE,
        ..PlantParams::default()
    };
    let (mut plant, _) = build_plant(p).unwrap();
    let (leaves, _) = build_leaves(&plant);
    plant.leaves = leaves;

    let text = export::write_plant_json(&plant);
    let restored = export::read_plant_json(&text).unwrap();
    assert_eq!(restored.params, plant.params);
    assert_eq!(restored.nodes, plant.nodes);
    assert_eq!(restored.leaves, plant.leaves);
    validate_plant(&restored).unwrap();
}

#[test]
fn param_archive_round_trips_and_regenerates_identical_plant() {
    let p = PlantParams::default();
    let archive = export::write_params_archive(&p);
    let loaded = export::read_params_archive(&archive).unwrap();
    assert_eq!(loaded, p.clone().validated().unwrap());

    let (a, _) = build_plant(p).unwrap();
    let (b, _) = build_plant(loaded).unwrap();
    assert_eq!(a, b);
}

#[test]
fn obj_bake_contains_every_node_and_leaf_with_manifest() {
    let p = PlantParams {
        branch_count: 2,
        limits: plant_core::Limits {
            max_depth: 3,
            ..Default::default()
        },
        ..PlantParams::default()
    };
    let (mut plant, _) = build_plant(p).unwrap();
    let (leaves, _) = build_leaves(&plant);
    plant.leaves = leaves;

    let (obj, manifest) = export::write_obj(&plant).unwrap();
    let branch_groups = plant
        .nodes
        .iter()
        .filter(|n| !geometry::is_degenerate(n))
        .count();
    for node in &plant.nodes {
        if geometry::is_degenerate(node) {
            continue;
        }
        assert!(obj.contains(&format!("g branch_{}", node.id)));
    }
    for i in 0..plant.leaves.len() {
        assert!(obj.contains(&format!("g leaf_{i}")));
    }
    assert!(manifest.contains("plant-obj-manifest/v1"));
    assert!(branch_groups > 0);
    // OBJ indices must be 1-based and in range: every `f` line parses.
    for line in obj.lines().filter(|l| l.starts_with("f ")) {
        let idx: Vec<u32> = line
            .split_whitespace()
            .skip(1)
            .map(|t| t.parse().unwrap())
            .collect();
        assert_eq!(idx.len(), 3);
        assert!(idx.iter().all(|&i| i >= 1));
    }
}

#[test]
fn disabling_leaves_produces_clean_branch_only_plant() {
    let p = PlantParams {
        leaves: plant_core::LeafParams {
            enabled: false,
            ..Default::default()
        },
        ..PlantParams::default()
    };
    let (mut plant, _) = build_plant(p).unwrap();
    let (leaves, stats) = build_leaves(&plant);
    plant.leaves = leaves;
    assert!(plant.leaves.is_empty());
    assert_eq!(stats.vertices, 0);
    validate_plant(&plant).unwrap();
}

#[test]
fn invalid_params_are_rejected_not_panicked() {
    let bad_nan = PlantParams {
        trunk_length: f32::NAN,
        ..PlantParams::default()
    };
    assert!(bad_nan.validated().is_err());
    let bad_radius = PlantParams {
        trunk_radius: -1.0,
        ..PlantParams::default()
    };
    assert!(bad_radius.validated().is_err());
}

#[test]
fn no_occlusion_based_deletion_even_with_dense_overlap() {
    // A dense, narrow plant where branches heavily overlap visually: the full
    // hierarchy must still be present — nothing is culled for being hidden.
    let p = PlantParams {
        branch_count: 6,
        branch_angle: 0.05,
        angle_jitter: 0.0,
        limits: plant_core::Limits {
            max_depth: 4,
            max_nodes: 500,
            ..Default::default()
        },
        ..PlantParams::default()
    };
    let (plant, _) = build_plant(p).unwrap();
    let expected_min = 1 + 6 + 36; // trunk + two full branching levels
    assert!(
        plant.node_count() >= expected_min,
        "dense plant lost branches: {} < {expected_min}",
        plant.node_count()
    );
    validate_plant(&plant).unwrap();
}
