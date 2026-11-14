//! Focused checks for transform composition under non-unit inherited scale.
//! The builder keeps per-node local scale at 1 and carries size explicitly,
//! but the matrix math must still compose correctly if an editor later
//! re-parents or scales nodes — these tests pin that contract.

use plant_core::math::{Mat4, Quat, Vec3};
use plant_core::{build_plant, PlantParams};

#[test]
fn scaled_parent_places_child_at_scaled_attachment() {
    // Simulate an externally applied parent scale of 2.5: every child world
    // position must be the scaled parent-local attachment, and orientation
    // columns must remain unit length (no skew from the scale).
    let p = PlantParams {
        bend: 0.25,
        branch_count: 3,
        limits: plant_core::Limits {
            max_depth: 4,
            ..Default::default()
        },
        ..PlantParams::default()
    };
    let (plant, _) = build_plant(p).unwrap();

    let scaled_parent = Mat4::from_trs(
        Vec3::new(1.0, 0.0, -2.0),
        Quat::axis_angle(Vec3::UP, 0.7),
        2.5,
    );

    for node in plant.nodes.iter().skip(1) {
        // Manually recompute the world matrix as if the root carried scale.
        let chain = scaled_parent.mul(node.local);
        let expected_origin = scaled_parent.mul_point(node.local.translation);
        assert!(
            chain.translation.sub(expected_origin).length() < 1e-5,
            "scaled composition moved the attachment point"
        );
        // Rotation columns stay orthonormal regardless of scale.
        for axis in chain.cols {
            assert!((axis.length_squared() - 1.0).abs() < 1e-5);
        }
        assert!((chain.scale - 2.5).abs() < 1e-5);
    }
}

#[test]
fn identity_composition_is_a_noop() {
    let p = PlantParams::default();
    let (plant, _) = build_plant(p).unwrap();
    for node in &plant.nodes {
        let composed = Mat4::IDENTITY.mul(node.local);
        assert_eq!(composed.translation, node.local.translation);
        assert!((composed.scale - node.local.scale).abs() < 1e-6);
    }
}

#[test]
fn antiparallel_rotation_arc_is_deterministic_and_unit() {
    let q = Quat::rotation_arc(Vec3::UP, Vec3::new(0.0, -1.0, 0.0));
    let turned = q.rotate_vec(Vec3::UP);
    assert!(turned.sub(Vec3::new(0.0, -1.0, 0.0)).length() < 1e-5);
    let norm = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
    assert!((norm - 1.0).abs() < 1e-6);
    // Same inputs twice -> same quaternion (no random fallback axis).
    let q2 = Quat::rotation_arc(Vec3::UP, Vec3::new(0.0, -1.0, 0.0));
    assert_eq!(q, q2);
}

#[test]
fn every_generated_node_has_finite_seed_driven_spline() {
    for seed in [0u64, 1, u64::MAX, 42, 0xDEAD_BEEF] {
        let p = PlantParams {
            seed,
            branch_count: 4,
            limits: plant_core::Limits {
                max_depth: 4,
                max_nodes: 200,
                ..Default::default()
            },
            ..PlantParams::default()
        };
        let (plant, _) = build_plant(p).unwrap();
        for node in &plant.nodes {
            for point in &node.spline {
                assert!(point.is_finite(), "non-finite spline at seed {seed}");
            }
            assert_eq!(node.spline[0], Vec3::ZERO, "branch must start at local origin");
        }
    }
}
