//! # plant-core
//!
//! Deterministic procedural virtual plant generation.
//!
//! Design guarantees:
//!
//! * **Reproducible rules, no global RNG.** Every random stream is an explicit
//!   [`rng::Rng`] derived from a 64-bit seed; the same [`params::PlantParams`]
//!   always builds the identical plant across processes and machines.
//! * **Hierarchical local coordinates.** Each branch stores a local transform
//!   relative to its parent; children attach to sampled points on the parent's
//!   curved centerline, so scaled or bent parents still place children at
//!   exact, deterministic connection points.
//! * **Bounded generation.** Depth, node, leaf, vertex and triangle budgets are
//!   hard limits enforced before allocation; the builder is iterative and
//!   cannot recurse or allocate without bound.
//! * **Real geometry with checks.** Tubes are welded ring-by-ring, degenerate
//!   branches are pruned deterministically, and leaf orientations are verified
//!   against their baked transforms. Occlusion never deletes a branch.
//! * **Structure separate from rendering.** The node tree and leaf instances
//!   are distinct from baked meshes; exports either preserve the editable
//!   hierarchy or bake OBJ geometry with an id-mapping manifest.

// Explicit method names (`add`, `sub`, `mul`) are preferred to operator
// overloading: every float operation stays visible in the geometry code.
#![allow(clippy::should_implement_trait)]

pub mod builder;
pub mod error;
pub mod export;
pub mod geometry;
pub mod json;
pub mod math;
pub mod model;
pub mod params;
pub mod rng;
pub mod validation;

pub use builder::{build_leaves, build_meshes, build_plant, merge_mesh_stats, BuildStats, MeshStats};
pub use error::PlantError;
pub use model::{BranchNode, LeafInstance, Plant, PruneReason, PruneRecord};
pub use params::{
    LeafParams, Limits, PlantParams, HARD_MAX_DEPTH, HARD_MAX_LEAVES, HARD_MAX_NODES,
    HARD_MAX_RADIAL_SIDES, HARD_MAX_SEGMENTS, HARD_MAX_TRIANGLES, HARD_MAX_VERTICES,
};
pub use validation::validate_plant;

/// One-call convenience: build structure, geometry and instances, then run the
/// full geometric validation suite.
pub fn generate(
    params: PlantParams,
) -> Result<(Plant, Vec<geometry::MeshData>, BuildStats, MeshStats), PlantError> {
    let (mut plant, build_stats) = build_plant(params)?;
    let (meshes, tube_stats) = build_meshes(&plant)?;
    let (leaves, leaf_stats) = build_leaves(&plant);
    plant.leaves = leaves;
    validate_plant(&plant)?;
    Ok((plant, meshes, build_stats, merge_mesh_stats(&tube_stats, &leaf_stats)))
}
