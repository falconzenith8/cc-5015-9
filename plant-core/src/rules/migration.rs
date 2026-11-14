//! Deterministic v1 -> v2 rule migration.
//!
//! The converter is structural and lossless:
//!
//! * the 64-bit seed is copied unchanged;
//! * branching levels (`count`, depth limit, ratios) map one-to-one;
//! * every material binding is re-emitted with its qualified layer selector;
//! * every implicit default the legacy parser applied is written out
//!   explicitly, never silently inherited;
//! * directives with no v2 equivalent become [`MigrationPending`] entries and
//!   are preserved as clearly-marked comments, so the converter cannot hide an
//!   unsupported rule behind a default.
//!
//! Geometry equivalence is checked on real generated plants (see
//! [`MigrationReport::geometry`]); matching seed strings alone are never
//! accepted as proof.

use crate::builder::{build_leaves, build_plant};
use crate::error::PlantError;
use crate::model::Plant;
use crate::params::PlantParams;

use super::ast::RuleDocument;
use super::legacy::{LegacyParseResult, UnknownDirective};
use super::{
    parse_legacy, plants_geometrically_equal, rule_err, RuleGeometryEvidence, RulePlan,
    FORMAT_V2, MIGRATION_VERSION,
};
use super::syntax_v2::{render_angle, render_length, render_ratio};

/// Output of one migration run.
#[derive(Clone, Debug, PartialEq)]
pub struct MigrationReport {
    pub source_format: String,
    pub target_format: String,
    pub seed: u64,
    /// Every implicit convention that was made explicit, in document order.
    pub implicit_defaults: Vec<String>,
    /// Rules that could not be mapped automatically.
    pub pending: Vec<MigrationPending>,
    /// Material bindings carried over, with their qualified selectors.
    pub materials_migrated: Vec<String>,
    /// Geometry-level verification, populated by [`verify_geometry`].
    pub geometry: Option<GeometryFingerprint>,
    pub migrated_text: String,
}

/// A rule the converter refuses to translate by guessing.
#[derive(Clone, Debug, PartialEq)]
pub struct MigrationPending {
    pub line_no: usize,
    pub source_text: String,
    pub reason: String,
}

/// Measured geometry fingerprints for both generation paths plus the field
/// comparison outcome. Both fingerprints come from actual builds, never from
/// seed text.
#[derive(Clone, Debug, PartialEq)]
pub struct GeometryFingerprint {
    pub legacy: PlantFingerprint,
    pub explicit: PlantFingerprint,
    pub equal: bool,
    pub comparison: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlantFingerprint {
    pub seed: u64,
    pub node_count: usize,
    pub max_depth: u32,
    pub leaf_count: usize,
    pub spline_sample_count: usize,
}

impl PlantFingerprint {
    pub fn of(params: &PlantParams, plant: &Plant) -> Self {
        Self {
            seed: params.seed,
            node_count: plant.nodes.len(),
            max_depth: plant.nodes.iter().map(|n| n.depth).max().unwrap_or(0),
            leaf_count: plant.leaves.len(),
            spline_sample_count: plant.nodes.iter().map(|n| n.spline.len()).sum(),
        }
    }
}

/// Parse and convert a legacy document, without running the generator. Use
/// [`verify_geometry`] afterwards when real geometry evidence is required.
pub fn migrate(text: &str) -> Result<MigrationReport, PlantError> {
    let doc = RuleDocument::tokenize(text)?;
    let legacy = parse_legacy(&doc)?;
    let LegacyParseResult {
        plan,
        implicit_defaults,
        unknown,
    } = legacy;

    let pending: Vec<MigrationPending> = unknown
        .iter()
        .map(|u: &UnknownDirective| MigrationPending {
            line_no: u.line_no,
            source_text: u.text.clone(),
            reason: u.reason.clone(),
        })
        .collect();

    let materials_migrated = plan
        .materials
        .iter()
        .map(|b| format!("{} -> {}", b.material, b.layer.render()))
        .collect();

    let migrated_text = render_v2(&plan, &implicit_defaults, &pending);

    Ok(MigrationReport {
        source_format: doc.format,
        target_format: FORMAT_V2.into(),
        seed: plan.params.seed,
        implicit_defaults,
        pending,
        materials_migrated,
        geometry: None,
        migrated_text,
    })
}

/// Build the plant through both rule paths and compare the real geometry.
/// Also attaches full rule-geometry evidence (layer identity and world-space
/// extents) to `evidence_out` when provided.
pub fn verify_geometry(
    legacy_text: &str,
    migrated_text: &str,
    evidence_out: Option<&mut Vec<RuleGeometryEvidence>>,
) -> Result<GeometryFingerprint, PlantError> {
    let legacy_plan = super::legacy::parse_legacy(&RuleDocument::tokenize(legacy_text)?)?.plan;
    let explicit_plan = super::syntax_v2::parse_v2(&RuleDocument::tokenize(migrated_text)?)?;

    // The seeds must be the same numeric value, but equality is decided on the
    // generated geometry below — not on this string/number match.
    if legacy_plan.params.seed != explicit_plan.params.seed {
        return rule_err(
            "migration changed the seed; deterministic equivalence cannot be established",
        );
    }

    let legacy_plant = build_generated(&legacy_plan)?;
    let explicit_plant = build_generated(&explicit_plan)?;

    let fp_legacy = PlantFingerprint::of(&legacy_plan.params, &legacy_plant);
    let fp_explicit = PlantFingerprint::of(&explicit_plan.params, &explicit_plant);

    let comparison = match plants_geometrically_equal(&legacy_plant, &explicit_plant) {
        Ok(()) => "field-level geometry identical across both rule paths".to_string(),
        Err(diff) => format!("GEOMETRY DIVERGENCE: {diff}"),
    };
    let equal = !comparison.starts_with("GEOMETRY DIVERGENCE");

    if let Some(out) = evidence_out {
        out.push(super::rule_geometry_evidence(&legacy_plan, &legacy_plant));
        out.push(super::rule_geometry_evidence(&explicit_plan, &explicit_plant));
    }

    Ok(GeometryFingerprint {
        legacy: fp_legacy,
        explicit: fp_explicit,
        equal,
        comparison,
    })
}

fn build_generated(plan: &RulePlan) -> Result<Plant, PlantError> {
    let (mut plant, _) = build_plant(plan.to_params())?;
    let (leaves, _) = build_leaves(&plant);
    plant.leaves = leaves;
    Ok(plant)
}

/// Render the explicit v2 document. Every engine field is written so the new
/// file stands alone with no inherited defaults. Fields the legacy file
/// stated keep their original numeric value and gain an explicit unit; fields
/// it omitted fall back to the engine base-unit rendering.
fn render_v2(plan: &RulePlan, implicit: &[String], pending: &[MigrationPending]) -> String {
    let p = &plan.params;
    let length = |field: &str, base: f32| match plan.displays.get(field) {
        Some(token) => token.to_string(),
        None => render_length(base),
    };
    let angle = |field: &str, base: f32| match plan.displays.get(field) {
        Some(token) => token.to_string(),
        None => render_angle(base),
    };
    let mut out = String::new();
    out.push_str(&format!("{FORMAT_V2}\n"));
    out.push_str("# Generated by the plant rule converter (migration v1 -> v2).\n");
    out.push_str("# Every dimensional value carries an explicit unit; references are qualified selectors.\n");
    out.push_str(&format!(
        "# migration-version: {MIGRATION_VERSION}\n"
    ));
    if implicit.is_empty() {
        out.push_str("# implicit-defaults-resolved: none\n");
    } else {
        out.push_str("# implicit-defaults-resolved:\n");
        for note in implicit {
            out.push_str(&format!("#   - {note}\n"));
        }
    }
    if !pending.is_empty() {
        out.push_str("# PENDING RULES — not mapped automatically, original lines preserved:\n");
        for item in pending {
            out.push_str(&format!(
                "#   [line {}] {} ({})\n",
                item.line_no, item.source_text, item.reason
            ));
        }
    }
    out.push('\n');

    out.push_str(&format!("seed value={}\n", p.seed));
    out.push_str(&format!(
        "trunk length={} radius={}\n",
        length("trunk.length", p.trunk_length),
        length("trunk.radius", p.trunk_radius)
    ));
    out.push_str(&format!(
        "branch count={} angle={} jitter={} ratio.length={} jitter.length={} ratio.radius={} taper={} bend={} segments={} sides={}\n",
        p.branch_count,
        angle("branch.angle", p.branch_angle),
        angle("branch.jitter", p.angle_jitter),
        render_ratio(p.length_ratio),
        render_ratio(p.length_jitter),
        render_ratio(p.radius_ratio),
        render_ratio(p.taper),
        render_ratio(p.bend),
        p.segments,
        p.radial_sides
    ));
    out.push_str(&format!(
        "leaf enabled={} per-tip={} length={} width={} lift={}\n",
        p.leaves.enabled,
        p.leaves.per_tip,
        length("leaf.length", p.leaves.length),
        length("leaf.width", p.leaves.width),
        render_ratio(p.leaves.lift)
    ));
    out.push_str(&format!(
        "limit depth={} nodes={} leaves={} vertices={} triangles={}\n",
        p.limits.max_depth,
        p.limits.max_nodes,
        p.limits.max_leaves,
        p.limits.max_vertices,
        p.limits.max_triangles
    ));

    for binding in &plan.materials {
        out.push_str(&format!(
            "material name={} layer={}\n",
            binding.material,
            binding.layer.render()
        ));
    }
    if let Some(attach) = &plan.attach {
        out.push_str(&format!(
            "attach target={} at={}\n",
            attach.target.render(),
            render_ratio(attach.t)
        ));
    }
    out
}

/// Convenience used by tests/CLI: full migration plus verification in one call.
pub fn migrate_and_verify(
    legacy_text: &str,
) -> Result<(MigrationReport, RuleGeometryEvidence, RuleGeometryEvidence), PlantError> {
    let mut report = migrate(legacy_text)?;
    let mut evidence = Vec::new();
    let fingerprint = verify_geometry(legacy_text, &report.migrated_text.clone(), Some(&mut evidence))?;
    if !fingerprint.equal {
        return rule_err(fingerprint.comparison.clone());
    }
    report.geometry = Some(fingerprint);
    let explicit_evidence = evidence.pop().expect("explicit evidence");
    let legacy_evidence = evidence.pop().expect("legacy evidence");
    Ok((report, legacy_evidence, explicit_evidence))
}
