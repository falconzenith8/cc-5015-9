//! User-tunable plant parameters plus the hard resource budgets that no
//! parameter combination can exceed.

use crate::error::PlantError;

/// Absolute safety ceiling for recursion depth. The builder is iterative, but
/// this also bounds stack-sized helper arrays.
pub const HARD_MAX_DEPTH: u32 = 16;
/// Absolute safety ceiling for total branch nodes (trunk included).
pub const HARD_MAX_NODES: usize = 100_000;
/// Absolute safety ceiling for leaf instances.
pub const HARD_MAX_LEAVES: usize = 500_000;
/// Absolute safety ceiling for generated mesh vertices.
pub const HARD_MAX_VERTICES: usize = 2_000_000;
/// Absolute safety ceiling for generated mesh triangles.
pub const HARD_MAX_TRIANGLES: usize = 1_000_000;
/// Geometry detail limits per branch.
pub const HARD_MAX_SEGMENTS: u32 = 64;
pub const HARD_MAX_RADIAL_SIDES: u32 = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// Max branching depth actually used (trunk = depth 0).
    pub max_depth: u32,
    /// Total branch node budget (trunk included).
    pub max_nodes: usize,
    /// Leaf instance budget.
    pub max_leaves: usize,
    /// Generated mesh vertex budget.
    pub max_vertices: usize,
    /// Generated mesh triangle budget.
    pub max_triangles: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_nodes: 4_000,
            max_leaves: 20_000,
            max_vertices: 200_000,
            max_triangles: 100_000,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LeafParams {
    pub enabled: bool,
    /// Leaves per terminal branch tip (also scattered along deep branches).
    pub per_tip: u32,
    pub length: f32,
    pub width: f32,
    /// 0 = flat along the branch tangent, 1 = fully upward facing.
    pub lift: f32,
}

impl Default for LeafParams {
    fn default() -> Self {
        Self {
            enabled: true,
            per_tip: 3,
            length: 0.35,
            width: 0.18,
            lift: 0.4,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlantParams {
    /// Root of every deterministic RNG stream.
    pub seed: u64,
    /// Trunk length in plant-local units.
    pub trunk_length: f32,
    /// Trunk radius at the base.
    pub trunk_radius: f32,
    /// Number of child branches spawned at each branching level.
    /// `0` produces a trunk-only plant (explicitly supported).
    pub branch_count: u32,
    /// Mean angle [rad] between a child branch and its parent's tangent.
    pub branch_angle: f32,
    /// Length of a child relative to its parent's length.
    pub length_ratio: f32,
    /// Radius of a child relative to its parent's radius.
    pub radius_ratio: f32,
    /// Radius taper from branch base to tip.
    pub taper: f32,
    /// Random +/- spread applied to branch angles [rad].
    pub angle_jitter: f32,
    /// Random +/- relative spread applied to child lengths.
    pub length_jitter: f32,
    /// Curvature of each branch: sideways bend as a fraction of length.
    pub bend: f32,
    /// Longitudinal segments per branch (spline resolution).
    pub segments: u32,
    /// Radial sides of the tube cross section.
    pub radial_sides: u32,
    pub leaves: LeafParams,
    pub limits: Limits,
}

impl Default for PlantParams {
    fn default() -> Self {
        Self {
            seed: 1337,
            trunk_length: 6.0,
            trunk_radius: 0.35,
            branch_count: 3,
            branch_angle: 0.7,
            length_ratio: 0.72,
            radius_ratio: 0.62,
            taper: 0.55,
            angle_jitter: 0.15,
            length_jitter: 0.12,
            bend: 0.08,
            segments: 8,
            radial_sides: 7,
            leaves: LeafParams::default(),
            limits: Limits::default(),
        }
    }
}

impl PlantParams {
    /// Validate and clamp every field against the hard safety budgets.
    /// Structural nonsense (NaN, negative sizes) is rejected; mere over-budget
    /// requests are clamped and reported through the builder report instead.
    pub fn validated(mut self) -> Result<Self, PlantError> {
        let check_finite = |name: &str, v: f32| -> Result<(), PlantError> {
            if !v.is_finite() {
                Err(PlantError::InvalidParam(format!("{name} must be finite")))
            } else {
                Ok(())
            }
        };
        check_finite("trunk_length", self.trunk_length)?;
        check_finite("trunk_radius", self.trunk_radius)?;
        check_finite("branch_angle", self.branch_angle)?;
        check_finite("length_ratio", self.length_ratio)?;
        check_finite("radius_ratio", self.radius_ratio)?;
        check_finite("taper", self.taper)?;
        check_finite("angle_jitter", self.angle_jitter)?;
        check_finite("length_jitter", self.length_jitter)?;
        check_finite("bend", self.bend)?;
        check_finite("leaf length", self.leaves.length)?;
        check_finite("leaf width", self.leaves.width)?;
        check_finite("leaf lift", self.leaves.lift)?;

        if self.trunk_length <= 0.0 {
            return Err(PlantError::InvalidParam("trunk_length must be > 0".into()));
        }
        if self.trunk_radius <= 0.0 {
            return Err(PlantError::InvalidParam("trunk_radius must be > 0".into()));
        }
        if self.leaves.enabled && (self.leaves.length <= 0.0 || self.leaves.width <= 0.0) {
            return Err(PlantError::InvalidParam("leaf size must be > 0".into()));
        }
        if !(0.0..=1.0).contains(&self.leaves.lift) {
            return Err(PlantError::InvalidParam("leaf lift must be in [0, 1]".into()));
        }

        self.branch_angle = self.branch_angle.clamp(0.0, std::f32::consts::PI);
        self.angle_jitter = self.angle_jitter.max(0.0);
        self.length_jitter = self.length_jitter.clamp(0.0, 0.95);
        self.length_ratio = self.length_ratio.clamp(0.0, 2.0);
        self.radius_ratio = self.radius_ratio.clamp(0.0, 1.0);
        self.taper = self.taper.clamp(0.0, 1.0);
        self.bend = self.bend.clamp(0.0, 1.0);
        self.segments = self.segments.clamp(1, HARD_MAX_SEGMENTS);
        self.radial_sides = self.radial_sides.clamp(3, HARD_MAX_RADIAL_SIDES);
        self.leaves.per_tip = self.leaves.per_tip.min(32);

        self.limits.max_depth = self.limits.max_depth.clamp(1, HARD_MAX_DEPTH);
        self.limits.max_nodes = self.limits.max_nodes.clamp(1, HARD_MAX_NODES);
        self.limits.max_leaves = self.limits.max_leaves.clamp(0, HARD_MAX_LEAVES);
        self.limits.max_vertices = self.limits.max_vertices.clamp(1, HARD_MAX_VERTICES);
        self.limits.max_triangles = self.limits.max_triangles.clamp(1, HARD_MAX_TRIANGLES);
        Ok(self)
    }
}
