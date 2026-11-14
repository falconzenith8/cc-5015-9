//! Rule documents and the v1 -> v2 migration machinery.
//!
//! Two rule syntaxes describe the same deterministic generator:
//!
//! * **v1 — legacy, implicit.** Bare numbers are interpreted in documented
//!   default units (lengths in metres, angles in radians) and branch targets
//!   are positional shorthand (`attach trunk 0.5`). The implicit contract is
//!   exactly what the migration has to make explicit.
//! * **v2 — explicit.** Every dimensional value carries a unit suffix
//!   (`m`/`cm`/`mm` for lengths, `rad`/`deg` for angles), dimensionless ratios
//!   are written as plain fractions, and every branch/material reference is a
//!   qualified selector (`target=branch:trunk`, `layer=branch:1..`). A bare
//!   number where a unit is required is a hard parse error, never a default.
//!
//! The converter is deterministic and lossless by construction: the seed, the
//! branching levels and every material binding are carried over verbatim,
//! anything that cannot be mapped is recorded as a [`MigrationPending`] entry
//! instead of being silently replaced by a default.

pub mod ast;
pub mod legacy;
pub mod migration;
pub mod syntax_v2;

pub use ast::{
    AttachRule, AttachTarget, LayerSelector, LegacyMeasure, Measure, MeasureUnit, RawRuleLine,
    RuleDocument,
};
pub use legacy::{parse_legacy, LegacyParseResult, UnknownDirective};
pub use migration::{
    migrate, migrate_and_verify, verify_geometry, GeometryFingerprint, MigrationPending,
    MigrationReport, PlantFingerprint,
};
pub use syntax_v2::parse_v2;

use crate::error::PlantError;
use crate::model::Plant;
use crate::params::PlantParams;

/// Document format marker stored on the first non-comment line.
pub const FORMAT_V1: &str = "plant-rules/v1";
pub const FORMAT_V2: &str = "plant-rules/v2";
/// Pinned migration contract version. Emitted into every converted file header
/// and surfaced by the CLI/GUI so generated-rule migrations are clearly marked.
pub const MIGRATION_VERSION: &str = "rule-migration/v1-to-v2";

/// Resolved rule set shared by both syntaxes. Compiling either syntax yields
/// the same plan, which is what makes a geometry-level migration check
/// possible. Material bindings are rule-level metadata; the builder itself is
/// unchanged, so the geometry contract cannot drift between the two paths.
#[derive(Clone, Debug, PartialEq)]
pub struct RulePlan {
    pub params: PlantParams,
    pub materials: Vec<MaterialBinding>,
    pub attach: Option<AttachRule>,
    /// Original value/unit spelling per engine field, captured by the legacy
    /// parser so migration can emit the source numbers losslessly (e.g.
    /// `320cm`) instead of a re-converted base-unit float. The builder always
    /// consumes [`RulePlan::params`]; this map only controls rendering.
    pub displays: FieldDisplays,
}

impl Default for RulePlan {
    fn default() -> Self {
        Self {
            params: PlantParams::default(),
            materials: Vec::new(),
            attach: None,
            displays: FieldDisplays::default(),
        }
    }
}

/// Engine field -> explicit display token (`"320cm"`, `"0.65rad"`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FieldDisplays {
    pub map: std::collections::BTreeMap<String, String>,
}

impl FieldDisplays {
    pub fn set(&mut self, field: &str, token: String) {
        self.map.insert(field.to_string(), token);
    }

    pub fn get(&self, field: &str) -> Option<&str> {
        self.map.get(field).map(String::as_str)
    }
}

impl RulePlan {
    /// Compile the plan into the exact [`PlantParams`] the deterministic core
    /// consumes. Both syntaxes funnel through this single entry point, so the
    /// virtual plant always runs one generation path.
    pub fn to_params(&self) -> PlantParams {
        self.params.clone()
    }
}

/// A material bound to a set of branch layers (by depth identity) or to
/// leaves. `range_end = None` means an open-ended range (`a..`).
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialBinding {
    pub material: String,
    pub layer: LayerSelector,
}

/// Evidence tying a rule-level concept to real generated geometry. Produced by
/// [`rule_geometry_evidence`] so migration review never relies on matching
/// seed *strings* — every claim is measured on the actual node tree.
#[derive(Clone, Debug, PartialEq)]
pub struct BindingEvidence {
    pub material: String,
    pub layer: LayerSelector,
    /// Node ids whose depth identity satisfies the layer selector.
    pub matched_nodes: Vec<usize>,
    /// World-space bounding box of the matched nodes' centerline samples:
    /// `[min, max]`. Empty when no node matched.
    pub bounds_min: Option<[f32; 3]>,
    pub bounds_max: Option<[f32; 3]>,
    pub matched: bool,
}

/// Per-node attachment evidence: where a child actually sits on its parent's
/// curved centerline, measured on generated geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct AttachEvidence {
    pub node_id: usize,
    pub parent_id: usize,
    pub depth: u32,
    /// Nearest spline sample index on the parent and its normalized position
    /// in `[0, 1]` along the parent centerline.
    pub parent_sample_index: usize,
    pub measured_t: f32,
    /// Distance from the child base to the nearest parent centerline sample.
    pub weld_distance: f32,
}

/// The full rule -> geometry evidence bundle used by the migration review.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RuleGeometryEvidence {
    pub node_count: usize,
    pub max_depth: u32,
    pub leaf_count: usize,
    pub bindings: Vec<BindingEvidence>,
    pub attachments: Vec<AttachEvidence>,
    /// Woody node ids not covered by any branch material binding. The
    /// converter treats an uncovered layer set as an explicit finding.
    pub uncovered_woody_nodes: Vec<usize>,
}

/// Measure rule semantics against a real generated plant. Layer identity is
/// the node depth; geometric extent is measured on world-space spline samples
/// rather than assumed from the rule text.
pub fn rule_geometry_evidence(plan: &RulePlan, plant: &Plant) -> RuleGeometryEvidence {
    let mut evidence = RuleGeometryEvidence {
        node_count: plant.nodes.len(),
        max_depth: plant.nodes.iter().map(|n| n.depth).max().unwrap_or(0),
        leaf_count: plant.leaves.len(),
        ..Default::default()
    };

    let mut covered: Vec<bool> = vec![false; plant.nodes.len()];
    for binding in &plan.materials {
        // Leaf bindings are evaluated against real leaf instances, using their
        // world anchors as the geometric extent, rather than branch depth.
        if let LayerSelector::Leaves = binding.layer {
            let leaf_node_ids: Vec<usize> = plant.leaves.iter().map(|l| l.node_id).collect();
            let (bounds_min, bounds_max) = leaf_world_bounds(plant);
            evidence.bindings.push(BindingEvidence {
                material: binding.material.clone(),
                layer: binding.layer.clone(),
                matched: !plant.leaves.is_empty(),
                bounds_min,
                bounds_max,
                matched_nodes: leaf_node_ids,
            });
            continue;
        }
        let mut matched_nodes = Vec::new();
        for node in &plant.nodes {
            if binding.layer.matches(node.depth) {
                matched_nodes.push(node.id);
                covered[node.id] = true;
            }
        }
        let (bounds_min, bounds_max) = world_bounds(plant, &matched_nodes);
        evidence.bindings.push(BindingEvidence {
            material: binding.material.clone(),
            layer: binding.layer.clone(),
            matched: !matched_nodes.is_empty(),
            bounds_min,
            bounds_max,
            matched_nodes,
        });
    }
    evidence.uncovered_woody_nodes = plant
        .nodes
        .iter()
        .filter(|n| !covered[n.id])
        .map(|n| n.id)
        .collect();

    for node in plant.nodes.iter().skip(1) {
        let Some(parent_id) = node.parent else { continue };
        let parent = &plant.nodes[parent_id];
        let base = node.world.translation;
        let mut best = (usize::MAX, f32::INFINITY);
        for (i, &sample) in parent.spline.iter().enumerate() {
            let world = parent.world.mul_point(sample);
            let d = world.sub(base).length_squared();
            if d < best.1 {
                best = (i, d);
            }
        }
        let denom = (parent.spline.len() - 1).max(1) as f32;
        evidence.attachments.push(AttachEvidence {
            node_id: node.id,
            parent_id,
            depth: node.depth,
            parent_sample_index: best.0,
            measured_t: best.0 as f32 / denom,
            weld_distance: best.1.sqrt(),
        });
    }
    evidence
}

fn world_bounds(plant: &Plant, ids: &[usize]) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut any = false;
    for &id in ids {
        let node = &plant.nodes[id];
        for &sample in &node.spline {
            let p = node.world.mul_point(sample);
            any = true;
            min[0] = min[0].min(p.x);
            min[1] = min[1].min(p.y);
            min[2] = min[2].min(p.z);
            max[0] = max[0].max(p.x);
            max[1] = max[1].max(p.y);
            max[2] = max[2].max(p.z);
        }
    }
    if any {
        (Some(min), Some(max))
    } else {
        (None, None)
    }
}

/// World-space bounds of every leaf anchor — the geometric extent of a
/// `layer=leaves` material binding measured on real instances.
fn leaf_world_bounds(plant: &Plant) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
    if plant.leaves.is_empty() {
        return (None, None);
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for leaf in &plant.leaves {
        let p = leaf.anchor;
        min[0] = min[0].min(p.x);
        min[1] = min[1].min(p.y);
        min[2] = min[2].min(p.z);
        max[0] = max[0].max(p.x);
        max[1] = max[1].max(p.y);
        max[2] = max[2].max(p.z);
    }
    (Some(min), Some(max))
}

/// Field-level geometry equality used by the migration review. Two plants are
/// the same plant only when their generated geometry agrees; an identical seed
/// string is deliberately never treated as sufficient on its own.
pub fn plants_geometrically_equal(a: &Plant, b: &Plant) -> Result<(), String> {
    if a.nodes.len() != b.nodes.len() {
        return Err(format!(
            "node count differs: {} vs {}",
            a.nodes.len(),
            b.nodes.len()
        ));
    }
    if a.leaves.len() != b.leaves.len() {
        return Err(format!(
            "leaf count differs: {} vs {}",
            a.leaves.len(),
            b.leaves.len()
        ));
    }
    for (i, (na, nb)) in a.nodes.iter().zip(b.nodes.iter()).enumerate() {
        if na.depth != nb.depth {
            return Err(format!("node {i} depth differs: {} vs {}", na.depth, nb.depth));
        }
        if na.spline.len() != nb.spline.len() {
            return Err(format!("node {i} spline sample count differs"));
        }
        for (k, (pa, pb)) in na.spline.iter().zip(nb.spline.iter()).enumerate() {
            if !vec3_close(*pa, *pb) {
                return Err(format!("node {i} spline sample {k} differs: {pa:?} vs {pb:?}"));
            }
        }
        if !vec3_close(na.world.translation, nb.world.translation) {
            return Err(format!("node {i} world translation differs"));
        }
        if (na.base_radius - nb.base_radius).abs() > 1e-6 {
            return Err(format!("node {i} base radius differs"));
        }
        if (na.length - nb.length).abs() > 1e-6 {
            return Err(format!("node {i} length differs"));
        }
    }
    for (i, (la, lb)) in a.leaves.iter().zip(b.leaves.iter()).enumerate() {
        if !vec3_close(la.anchor, lb.anchor) {
            return Err(format!("leaf {i} anchor differs"));
        }
        if !vec3_close(la.normal, lb.normal) {
            return Err(format!("leaf {i} normal differs"));
        }
    }
    Ok(())
}

fn vec3_close(a: crate::math::Vec3, b: crate::math::Vec3) -> bool {
    let close = |x: f32, y: f32| (x - y).abs() <= 1e-6 * (1.0 + y.abs());
    close(a.x, b.x) && close(a.y, b.y) && close(a.z, b.z)
}

/// Split a rule line into the directive and its whitespace-separated tokens.
/// Comments run from `#` to end of line; blank lines yield [`None`].
pub fn tokenize_line(line: &str) -> Option<Vec<String>> {
    let stripped = match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    };
    let tokens: Vec<String> = stripped
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens)
    }
}

pub(crate) fn rule_err<T>(msg: impl Into<String>) -> Result<T, PlantError> {
    Err(PlantError::Rule(msg.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_selector_matches_depth_identity() {
        assert!(LayerSelector::Trunk.matches(0));
        assert!(!LayerSelector::Trunk.matches(1));
        assert!(LayerSelector::Exact(2).matches(2));
        assert!(!LayerSelector::Exact(2).matches(3));
        assert!(LayerSelector::From(1).matches(1) && LayerSelector::From(1).matches(9));
        assert!(!LayerSelector::From(2).matches(1));
        assert!(LayerSelector::Range(1, 3).matches(2));
        assert!(!LayerSelector::Range(1, 3).matches(4));
        // Leaves never match a branch depth.
        assert!(!LayerSelector::Leaves.matches(0));
    }

    #[test]
    fn qualified_selector_round_trips() {
        for (text, selector) in [
            ("branch:trunk", LayerSelector::Trunk),
            ("branch:2", LayerSelector::Exact(2)),
            ("branch:1..", LayerSelector::From(1)),
            ("branch:1..3", LayerSelector::Range(1, 3)),
            ("leaves", LayerSelector::Leaves),
        ] {
            assert_eq!(LayerSelector::parse_qualified(text).unwrap(), selector);
            assert_eq!(selector.render(), text);
        }
        assert!(LayerSelector::parse_qualified("trunk").is_err());
        assert!(LayerSelector::parse_qualified("branch:3..1").is_err());
    }

    #[test]
    fn tokenizer_strips_comments_and_blanks() {
        assert!(tokenize_line("   # full comment").is_none());
        assert!(tokenize_line("").is_none());
        assert_eq!(
            tokenize_line("seed 42 # trailing").unwrap(),
            vec!["seed".to_string(), "42".to_string()]
        );
    }

    #[test]
    fn field_displays_last_write_wins() {
        let mut d = FieldDisplays::default();
        d.set("trunk.length", "320cm".into());
        assert_eq!(d.get("trunk.length"), Some("320cm"));
        assert!(d.get("missing").is_none());
    }
}
