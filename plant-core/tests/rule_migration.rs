//! Rule DSL migration tests.
//!
//! The review contract these tests pin down:
//!
//! * implicit legacy units and positional branch references become explicit;
//! * seed, branching levels and material bindings survive the migration;
//! * unknown rules are reported as pending, never defaulted away;
//! * equivalence is decided on real generated geometry — identical seed
//!   strings alone are never treated as equal results across rule paths;
//! * material layer identity is checked against node depth and world-space
//!   geometric extents measured on the generated plant.

use plant_core::builder::{build_leaves, build_plant};
use plant_core::rules::{
    migrate, migrate_and_verify, parse_legacy, parse_v2, plants_geometrically_equal,
    rule_geometry_evidence, RuleDocument, LayerSelector, MeasureUnit, FORMAT_V2,
};
use plant_core::validate_plant;

const SHRUB: &str = include_str!("fixtures/shrub.legacy.rules");
const CM_TREE: &str = include_str!("fixtures/cm_tree.legacy.rules");
const UNKNOWN: &str = include_str!("fixtures/unknown.legacy.rules");

fn compile(text: &str) -> plant_core::rules::RulePlan {
    let doc = RuleDocument::tokenize(text).unwrap();
    if doc.format == FORMAT_V2 {
        parse_v2(&doc).unwrap()
    } else {
        parse_legacy(&doc).unwrap().plan
    }
}

fn build(plan: &plant_core::rules::RulePlan) -> plant_core::Plant {
    let (mut plant, _) = build_plant(plan.to_params()).unwrap();
    let (leaves, _) = build_leaves(&plant);
    plant.leaves = leaves;
    validate_plant(&plant).unwrap();
    plant
}

#[test]
fn legacy_parser_records_every_implicit_default() {
    let doc = RuleDocument::tokenize(SHRUB).unwrap();
    let result = parse_legacy(&doc).unwrap();
    // No `unit` header: metre default must be audited.
    assert!(result
        .implicit_defaults
        .iter()
        .any(|n| n.contains("bare length numbers interpreted as metres")));
    // Every bare angle produced an explicit radian note.
    assert!(result
        .implicit_defaults
        .iter()
        .any(|n| n.contains("branch.angle") && n.contains("rad")));
    // Seed and the recursive branching levels parsed through.
    assert_eq!(result.plan.params.seed, 20260114);
    assert_eq!(result.plan.params.branch_count, 3);
    assert_eq!(result.plan.params.limits.max_depth, 5);
    assert!(result.unknown.is_empty());
}

#[test]
fn centimetre_header_changes_resolution_but_v2_is_always_metres() {
    let result = parse_legacy(&RuleDocument::tokenize(CM_TREE).unwrap()).unwrap();
    // 320 cm -> 3.2 m in engine base units.
    assert!((result.plan.params.trunk_length - 3.2).abs() < 1e-6);
    // 24 cm leaf -> 0.24 m.
    assert!((result.plan.params.leaves.length - 0.24).abs() < 1e-6);

    let report = migrate(CM_TREE).unwrap();
    let explicit = parse_v2(&RuleDocument::tokenize(&report.migrated_text).unwrap()).unwrap();
    assert!((explicit.params.trunk_length - 3.2).abs() < 1e-6);
    // The explicit file preserves the source number and makes the unit
    // explicit: 320 cm rather than a re-converted, rounding-prone 3.1999998 m.
    assert!(report.migrated_text.contains("length=320cm"));
    assert!(report.migrated_text.contains("radius=22cm"));
    assert!(report.migrated_text.contains("length=24cm"));
    // Angles keep their radian meaning and gain an explicit suffix.
    assert!(report.migrated_text.contains("angle=0.5rad"));
    // The compiled value is still in engine base units (metres).
    assert!((explicit.params.trunk_radius - 0.22).abs() < 1e-6);
}

#[test]
fn migration_preserves_seed_levels_and_material_bindings() {
    let report = migrate(SHRUB).unwrap();
    assert_eq!(report.seed, 20260114);
    assert!(report.pending.is_empty());
    // Three distinct bindings, each rendered with a qualified layer.
    assert_eq!(report.materials_migrated.len(), 3);
    assert!(report.migrated_text.contains("material name=bark layer=branch:trunk"));
    assert!(report.migrated_text.contains("material name=wood layer=branch:1.."));
    assert!(report.migrated_text.contains("material name=foliage layer=leaves"));
    // The positional attach reference becomes a qualified selector.
    assert!(report.migrated_text.contains("attach target=branch:trunk at=0.55"));
    // Every emitted measure carries a unit; the migration version is noted.
    assert!(report.migrated_text.contains("migration-version: rule-migration/v1-to-v2"));
    assert!(report.migrated_text.contains("trunk length=4.5m"));
    assert!(report.migrated_text.contains("radius=0.28m"));
}

#[test]
fn unknown_rules_are_pending_not_defaulted() {
    let report = migrate(UNKNOWN).unwrap();
    let pending_text: Vec<&str> = report.pending.iter().map(|p| p.source_text.as_str()).collect();
    assert!(pending_text.contains(&"season autumn"));
    assert!(pending_text.contains(&"wind-field curl 0.3"));
    assert!(pending_text.contains(&"phyllotaxy spiral golden"));
    // Original lines are preserved verbatim in the output as comments.
    for p in &report.pending {
        assert!(report
            .migrated_text
            .contains(&format!("[line {}] {}", p.line_no, p.source_text)));
    }
    // The mappable portion still migrates cleanly.
    assert_eq!(report.seed, 4242);
    assert!(report.migrated_text.contains("material name=bark layer=branch:trunk"));
}

#[test]
fn legacy_and_explicit_paths_produce_identical_generated_geometry() {
    for source in [SHRUB, CM_TREE] {
        let report = migrate(source).unwrap();
        let legacy_plan = compile(source);
        let explicit_plan = compile(&report.migrated_text);

        let legacy_plant = build(&legacy_plan);
        let explicit_plant = build(&explicit_plan);

        // The real contract: field-level geometry equality, not seed strings.
        plants_geometrically_equal(&legacy_plant, &explicit_plant)
            .unwrap_or_else(|e| panic!("geometry diverged: {e}"));
        assert_eq!(legacy_plant, explicit_plant);
    }
}

#[test]
fn same_seed_string_alone_is_not_cross_algorithm_equality() {
    // Two different rule sets that happen to carry the SAME seed must still
    // produce different geometry — proving the review never equates seed text.
    let mut a = compile(SHRUB);
    let mut b = compile(CM_TREE);
    b.params.seed = a.params.seed;
    a.params.seed = 999;
    b.params.seed = 999;
    let pa = build(&a);
    let pb = build(&b);
    // Equal seed, different structural parameters -> different splines.
    assert_eq!(pa.params.seed, pb.params.seed);
    assert_ne!(pa.nodes[0].spline, pb.nodes[0].spline);
    // And the geometry comparator must flag the divergence rather than pass.
    assert!(plants_geometrically_equal(&pa, &pb).is_err());
}

#[test]
fn verify_geometry_reports_real_fingerprints_and_evidence() {
    let (report, legacy_evidence, explicit_evidence) = migrate_and_verify(SHRUB).unwrap();
    let fp = report.geometry.as_ref().expect("fingerprint attached");
    assert!(fp.equal);
    assert_eq!(fp.legacy.node_count, fp.explicit.node_count);
    assert_eq!(fp.legacy.spline_sample_count, fp.explicit.spline_sample_count);
    assert!(fp.legacy.node_count > 1); // recursive branching really happened

    let legacy_plant = build(&compile(SHRUB));

    // Layer identity: trunk binding matches exactly depth 0.
    let bark = legacy_evidence
        .bindings
        .iter()
        .find(|b| b.material == "bark")
        .unwrap();
    assert_eq!(bark.layer, LayerSelector::Trunk);
    assert_eq!(bark.matched_nodes, vec![0]);
    assert!(bark.bounds_min.is_some());

    // Recursive branch binding covers every non-trunk depth and has measured
    // world-space extent; matched ids are real nodes at depth >= 1.
    let wood = legacy_evidence
        .bindings
        .iter()
        .find(|b| b.material == "wood")
        .unwrap();
    assert!(wood.matched_nodes.len() > 1);
    assert!(wood
        .matched_nodes
        .iter()
        .all(|&id| legacy_plant.nodes[id].depth >= 1));
    assert!(wood.bounds_min.is_some() && wood.bounds_max.is_some());
    // Both paths yield the same matched node identity sets.
    assert_eq!(legacy_evidence.bindings, explicit_evidence.bindings);
}

#[test]
fn leaf_material_binding_matches_real_leaf_instances() {
    let plan = compile(SHRUB);
    let plant = build(&plan);
    assert!(!plant.leaves.is_empty());
    let evidence = rule_geometry_evidence(&plan, &plant);
    let foliage = evidence
        .bindings
        .iter()
        .find(|b| b.material == "foliage")
        .expect("foliage binding");
    assert_eq!(foliage.layer, LayerSelector::Leaves);
    assert!(foliage.matched, "leaf binding must match real leaf instances");
    // One matched node id per leaf instance, all referencing real branches.
    assert_eq!(foliage.matched_nodes.len(), plant.leaves.len());
    assert!(foliage.matched_nodes.iter().all(|&id| id < plant.nodes.len()));
    assert!(foliage.bounds_min.is_some() && foliage.bounds_max.is_some());
}

#[test]
fn attachment_evidence_lands_children_on_parent_centerlines() {
    let plan = compile(SHRUB);
    let plant = build(&plan);
    let evidence = rule_geometry_evidence(&plan, &plant);
    assert!(!evidence.attachments.is_empty());
    for a in &evidence.attachments {
        assert!((0.0..=1.0).contains(&a.measured_t));
        // Weld distance is numerically tiny: real attachment, not a heuristic.
        assert!(a.weld_distance < 1e-3, "weld gap {} too large", a.weld_distance);
        assert_eq!(a.depth, plant.nodes[a.node_id].depth);
    }
}

#[test]
fn explicit_syntax_rejects_bare_numbers_and_bad_references() {
    let missing_unit = "plant-rules/v2\nseed value=1\ntrunk length=4.5 radius=0.2m\n";
    let err = parse_v2(&RuleDocument::tokenize(missing_unit).unwrap()).unwrap_err();
    assert!(err.to_string().contains("missing an explicit unit"));

    let bad_ref = "plant-rules/v2\nseed value=1\nattach target=trunk at=0.5\n";
    let err = parse_v2(&RuleDocument::tokenize(bad_ref).unwrap()).unwrap_err();
    assert!(err.to_string().contains("branch:trunk"));

    let unknown_directive = "plant-rules/v2\nseed value=1\nseason value=autumn\n";
    assert!(parse_v2(&RuleDocument::tokenize(unknown_directive).unwrap()).is_err());
}

#[test]
fn explicit_syntax_accepts_degree_units_and_qualified_layers() {
    let text = "plant-rules/v2\n\
seed value=5\n\
trunk length=2m radius=10cm\n\
branch count=2 angle=45deg jitter=5deg ratio.length=0.7 ratio.radius=0.6 taper=0.5 bend=0.1 segments=4 sides=5\n\
limit depth=3 nodes=100 leaves=100 vertices=10000 triangles=10000\n\
material name=bark layer=branch:trunk\n\
material name=twig layer=branch:1..3\n";
    let plan = parse_v2(&RuleDocument::tokenize(text).unwrap()).unwrap();
    // 45 degrees -> pi/4 radians.
    assert!((plan.params.branch_angle - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    // 10 cm -> 0.1 m.
    assert!((plan.params.trunk_radius - 0.1).abs() < 1e-6);
    let plant = build(&plan);
    let evidence = rule_geometry_evidence(&plan, &plant);
    let twig = evidence
        .bindings
        .iter()
        .find(|b| b.material == "twig")
        .unwrap();
    assert_eq!(twig.layer, LayerSelector::Range(1, 3));
    assert!(twig.matched_nodes.iter().all(|&id| plant.nodes[id].depth >= 1));
}

#[test]
fn round_tripped_v2_file_rebuilds_the_same_plant() {
    let report = migrate(SHRUB).unwrap();
    let rebuilt_plan = compile(&report.migrated_text);
    let plant = build(&rebuilt_plan);
    // Re-parse the migrated text once more to ensure stable serialization.
    let again = compile(&report.migrated_text);
    let plant_again = build(&again);
    assert_eq!(plant, plant_again);
}

#[test]
fn unit_suffix_dimensions_are_validated() {
    assert_eq!(MeasureUnit::CentiMeter.to_base_factor(), 0.01);
    assert!((MeasureUnit::Degree.to_base_factor() - std::f32::consts::PI / 180.0).abs() < 1e-9);
    assert!(MeasureUnit::Meter.is_length());
    assert!(!MeasureUnit::Radian.is_length());
}
