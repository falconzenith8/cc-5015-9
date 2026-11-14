//! Geometric validation of a generated plant.
//!
//! These are real geometry checks, not heuristics: world transforms are
//! recomputed from the local chain, child bases are measured against the
//! parent's curved centerline, tube seams and triangle areas are inspected, and
//! leaf normals are verified against their baked transforms.
//!
//! Note the deliberate non-goal: nothing here (or anywhere in the crate)
//! removes a branch because it is occluded. Visibility is a view-dependent
//! rendering concern; the structural hierarchy is always preserved.

use crate::builder::build_meshes;
use crate::geometry::{
    check_leaf_orientation, check_tube_integrity, effective_sides, is_degenerate, WELD_EPS,
};
use crate::math::Mat4;
use crate::model::Plant;
use crate::{
    error::PlantError,
    params::{HARD_MAX_DEPTH, HARD_MAX_LEAVES, HARD_MAX_NODES},
};

/// Run every structural and geometric check. On success the plant is safe to
/// render and export; on failure the returned message names the offending node.
pub fn validate_plant(plant: &Plant) -> Result<(), PlantError> {
    check_hard_budgets(plant)?;
    check_hierarchy(plant)?;
    check_transforms(plant)?;
    check_attachments(plant)?;
    check_no_degenerate(plant)?;
    check_tubes(plant)?;
    check_leaves(plant)?;
    check_finite(plant)?;
    Ok(())
}

fn check_hard_budgets(plant: &Plant) -> Result<(), PlantError> {
    let l = &plant.params.limits;
    if plant.nodes.len() > l.max_nodes || plant.nodes.len() > HARD_MAX_NODES {
        return Err(PlantError::InvalidPlant(format!(
            "node count {} exceeds budget {}",
            plant.nodes.len(),
            l.max_nodes
        )));
    }
    if plant.leaves.len() > l.max_leaves || plant.leaves.len() > HARD_MAX_LEAVES {
        return Err(PlantError::InvalidPlant(format!(
            "leaf count {} exceeds budget {}",
            plant.leaves.len(),
            l.max_leaves
        )));
    }
    let max_depth = plant.nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    if max_depth + 1 > l.max_depth || max_depth + 1 > HARD_MAX_DEPTH {
        return Err(PlantError::InvalidPlant(format!(
            "depth {} exceeds budget {}",
            max_depth, l.max_depth
        )));
    }
    Ok(())
}

fn check_hierarchy(plant: &Plant) -> Result<(), PlantError> {
    if plant.nodes.is_empty() || plant.nodes[0].parent.is_some() {
        return Err(PlantError::InvalidPlant("missing single trunk root".into()));
    }
    for node in &plant.nodes {
        if node.id >= plant.nodes.len() || plant.nodes[node.id].id != node.id {
            return Err(PlantError::InvalidPlant(format!(
                "node id {} does not index its own slot",
                node.id
            )));
        }
        if let Some(p) = node.parent {
            if p >= node.id || !plant.nodes[p].children.contains(&node.id) {
                return Err(PlantError::InvalidPlant(format!(
                    "node {} parent/child linkage broken",
                    node.id
                )));
            }
            if plant.nodes[p].depth + 1 != node.depth {
                return Err(PlantError::InvalidPlant(format!(
                    "node {} depth disagrees with parent",
                    node.id
                )));
            }
        }
        let expected_terminal = node.children.is_empty();
        if node.terminal != expected_terminal {
            return Err(PlantError::InvalidPlant(format!(
                "node {} terminal flag inconsistent",
                node.id
            )));
        }
    }
    Ok(())
}

/// Recompute every world matrix from the local chain and compare. This is the
/// core guarantee that local coordinates compose correctly under scaled or
/// bent parents.
fn check_transforms(plant: &Plant) -> Result<(), PlantError> {
    for node in &plant.nodes {
        let recomputed = match node.parent {
            None => Mat4::IDENTITY,
            Some(p) => plant.nodes[p].world.mul(node.local),
        };
        if !mat4_close(recomputed, node.world) {
            return Err(PlantError::InvalidPlant(format!(
                "node {} world transform does not equal parent.world * local",
                node.id
            )));
        }
    }
    Ok(())
}

/// Every child's local origin must map, through the parent world matrix, to a
/// point lying on the parent's actual curved centerline — not merely to the
/// parent's origin. Distance is measured against the nearest spline sample.
fn check_attachments(plant: &Plant) -> Result<(), PlantError> {
    for node in &plant.nodes {
        let Some(pid) = node.parent else { continue };
        let parent = &plant.nodes[pid];
        let child_base_world = node.world.translation;
        let nearest = parent
            .spline
            .iter()
            .map(|&p| parent.world.mul_point(p))
            .map(|p| p.sub(child_base_world).length_squared())
            .fold(f32::INFINITY, f32::min);
        // Tolerance scales with parent length so tiny plants are not penalized
        // by absolute float error, but a missed attachment is still caught.
        let tol = (WELD_EPS * parent.length.max(1.0) * 4.0).max(WELD_EPS);
        if nearest.sqrt() > tol {
            return Err(PlantError::InvalidPlant(format!(
                "node {} base is {} units off parent centerline",
                node.id,
                nearest.sqrt()
            )));
        }
    }
    Ok(())
}

fn check_no_degenerate(plant: &Plant) -> Result<(), PlantError> {
    for node in &plant.nodes {
        if is_degenerate(node) {
            return Err(PlantError::InvalidPlant(format!(
                "node {} is geometrically degenerate (length={}, radius={})",
                node.id, node.length, node.base_radius
            )));
        }
    }
    Ok(())
}

fn check_tubes(plant: &Plant) -> Result<(), PlantError> {
    let (meshes, _) = build_meshes(plant)?;
    let sides = effective_sides(&plant.params) as usize;
    for (node, mesh) in plant.nodes.iter().zip(meshes.iter()) {
        if mesh.vertices.is_empty() {
            continue; // budget-skipped nodes are allowed and reported separately
        }
        check_tube_integrity(mesh, sides).map_err(|m| {
            PlantError::InvalidPlant(format!("tube seam failure on node {}: {m}", node.id))
        })?;
    }
    Ok(())
}

fn check_leaves(plant: &Plant) -> Result<(), PlantError> {
    let mut attached: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for leaf in &plant.leaves {
        check_leaf_orientation(leaf).map_err(PlantError::InvalidPlant)?;
        if leaf.node_id >= plant.nodes.len() {
            return Err(PlantError::InvalidPlant(
                "leaf references unknown node".into(),
            ));
        }
        // The anchor must sit on or just outside its branch's tip region.
        let node = &plant.nodes[leaf.node_id];
        let tip_world = node.world.mul_point(node.spline[node.spline.len() - 1]);
        let reach = node.tip_radius * 1.5 + node.length * 0.25;
        if leaf.anchor.sub(tip_world).length() > reach {
            return Err(PlantError::InvalidPlant(format!(
                "leaf anchor {} is not attached to its branch tip",
                leaf.node_id
            )));
        }
        attached.insert(leaf.node_id as u64);
    }
    Ok(())
}

fn check_finite(plant: &Plant) -> Result<(), PlantError> {
    for node in &plant.nodes {
        for p in &node.spline {
            if !p.is_finite() {
                return Err(PlantError::InvalidPlant(format!(
                    "non-finite spline point on node {}",
                    node.id
                )));
            }
        }
    }
    Ok(())
}

fn mat4_close(a: Mat4, b: Mat4) -> bool {
    let eps = 1e-5;
    let v = |x: f32, y: f32| (x - y).abs() <= eps * (1.0 + y.abs());
    for i in 0..3 {
        for k in 0..3 {
            if !v(a.cols[i].to_array()[k], b.cols[i].to_array()[k]) {
                return false;
            }
        }
    }
    for k in 0..3 {
        let av = a.translation.to_array()[k];
        let bv = b.translation.to_array()[k];
        if !v(av, bv) {
            return false;
        }
    }
    (a.scale - b.scale).abs() <= eps * (1.0 + b.scale.abs())
}
