//! The hierarchical plant model.
//!
//! Structure (the node tree with parent-relative transforms and per-node
//! seeds) is kept separate from rendering data (baked tube/leaf meshes with
//! world-space instance transforms). Export can therefore either preserve the
//! editable hierarchy or bake flattened geometry.

use crate::math::{Mat4, Vec3};

/// Why a candidate branch was not emitted. Pruning is fully deterministic and
/// is recorded so exported results remain explainable and reproducible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PruneReason {
    /// Child length or radius collapsed below the geometric floor.
    Degenerate,
    /// `max_depth` reached.
    DepthLimit,
    /// `max_nodes` reached.
    NodeBudget,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PruneRecord {
    pub parent_id: NodeId,
    pub child_index: u32,
    pub reason: PruneReason,
}

/// Stable identifier equal to the node's index in [`Plant::nodes`].
pub type NodeId = usize;

#[derive(Clone, Debug, PartialEq)]
pub struct BranchNode {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    /// Depth in the tree; the trunk is depth 0.
    pub depth: u32,
    /// Local transform relative to the parent frame (world for the trunk).
    /// Children attach to the parent's *curved* spline, and this transform is
    /// built so that composing parent transforms always lands the child base
    /// exactly on that attachment point.
    pub local: Mat4,
    /// Cached world transform = parent.world * local.
    pub world: Mat4,
    /// Length of this branch's centerline in its own local units.
    pub length: f32,
    /// Radius at the branch base in local units.
    pub base_radius: f32,
    /// Radius at the tip after taper.
    pub tip_radius: f32,
    /// Centerline samples in *local* space, including both end caps.
    pub spline: Vec<Vec3>,
    /// Deterministic RNG state seed used while building this node's children.
    pub seed: u64,
    /// Child node ids in deterministic emission order.
    pub children: Vec<NodeId>,
    /// True when this node has no children (a tip eligible for leaf clusters).
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeafInstance {
    /// Branch the leaf grows from.
    pub node_id: NodeId,
    /// World-space transform of the shared leaf mesh.
    pub world: Mat4,
    /// Attachment point in world space, redundant with the transform column
    /// but kept explicit for geometric validation.
    pub anchor: Vec3,
    /// World-space facing normal of the leaf blade.
    pub normal: Vec3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plant {
    pub params: crate::params::PlantParams,
    /// Depth-first deterministic node order; node 0 is always the trunk.
    pub nodes: Vec<BranchNode>,
    pub leaves: Vec<LeafInstance>,
    pub pruned: Vec<PruneRecord>,
}

impl Plant {
    pub fn trunk(&self) -> &BranchNode {
        &self.nodes[0]
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Iterate parent -> children without recursion.
    pub fn walk(&self) -> impl Iterator<Item = &BranchNode> {
        self.nodes.iter()
    }
}
