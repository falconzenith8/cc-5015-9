//! Headless front-end for the deterministic plant generator.
//!
//! Examples:
//!   plant generate --seed 42 --branches 4 --out-dir out
//!   plant validate --plant out/plant.json
//!   plant regenerate --params out/params.json --out-dir rebuilt
//!   plant stress --out-dir stress

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use plant_core::rules::{
    migrate, rule_geometry_evidence, verify_geometry, RuleDocument, RulePlan, FORMAT_V1,
    FORMAT_V2,
};
use plant_core::{
    build_leaves, build_meshes, build_plant, export, merge_mesh_stats, validate_plant,
    LeafParams, Limits, PlantParams,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let (cmd, rest) = args
        .split_first()
        .map(|(_, r)| (r.first().map(String::as_str).unwrap_or("help"), r.split_at(1).1.to_vec()))
        .unwrap_or(("help", Vec::new()));
    match cmd {
        "generate" => cmd_generate(rest),
        "validate" => cmd_validate(rest),
        "regenerate" => cmd_regenerate(rest),
        "stress" => cmd_stress(rest),
        "rule-migrate" => cmd_rule_migrate(rest),
        "rule-generate" => cmd_rule_generate(rest),
        "rule-verify" => cmd_rule_verify(rest),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!("unknown command '{other}' (try `plant help`)")),
    }
}

fn print_help() {
    println!(
        "plant — deterministic procedural plant generator\n\
\n\
USAGE:\n  \
plant <COMMAND> [OPTIONS]\n\
\n\
COMMANDS:\n  \
generate      Build a plant from CLI parameters, validate and export\n  \
validate      Re-validate an exported plant-hierarchy JSON file\n  \
regenerate    Rebuild from a parameter archive and compare to the archive\n  \
stress        Run the required edge-case suite (fixed seed, zero branches,\n  \
              sharp angles, many small leaves) and print budget reports\n  \
rule-migrate  Convert legacy plant-rules/v1 (implicit units, positional\n  \
              branch refs) into explicit plant-rules/v2; backs up the source,\n  \
              writes a migration report and lists any rule that cannot map\n  \
rule-generate Build a plant from a rule file (v1 or v2) through the single\n  \
              core generation path, validate and export\n  \
rule-verify   Migrate a v1 file and prove geometry equivalence against the\n  \
              real generated plants (not seed strings), with layer evidence\n  \
help          Show this message\n\
\n\
COMMON OPTIONS (generate):\n  \
--seed <u64> --branches <u32> --angle <radians> --length-ratio <f32>\n  \
--bend <f32> --depth <u32> --max-nodes <usize> --max-leaves <usize>\n  \
--max-vertices <usize> --max-triangles <usize> --no-leaves\n  \
--out-dir <path>"
    );
}

struct GenArgs {
    params: PlantParams,
    out_dir: PathBuf,
}

fn parse_generate_args(rest: &[String]) -> Result<GenArgs, String> {
    let mut p = PlantParams::default();
    let mut out_dir = PathBuf::from("plant-out");
    let mut i = 0;
    let value = |i: usize| -> Result<&String, String> {
        rest.get(i + 1)
            .ok_or_else(|| format!("option {} needs a value", rest[i]))
    };
    while i < rest.len() {
        let key = rest[i].as_str();
        match key {
            "--seed" => {
                p.seed = value(i)?.parse().map_err(|e| format!("bad seed: {e}"))?;
                i += 2;
            }
            "--branches" => {
                p.branch_count = value(i)?.parse().map_err(|e| format!("bad branches: {e}"))?;
                i += 2;
            }
            "--angle" => {
                p.branch_angle = value(i)?.parse().map_err(|e| format!("bad angle: {e}"))?;
                i += 2;
            }
            "--length-ratio" => {
                p.length_ratio = value(i)?.parse().map_err(|e| format!("bad ratio: {e}"))?;
                i += 2;
            }
            "--radius-ratio" => {
                p.radius_ratio = value(i)?.parse().map_err(|e| format!("bad ratio: {e}"))?;
                i += 2;
            }
            "--bend" => {
                p.bend = value(i)?.parse().map_err(|e| format!("bad bend: {e}"))?;
                i += 2;
            }
            "--trunk-length" => {
                p.trunk_length = value(i)?.parse().map_err(|e| format!("bad length: {e}"))?;
                i += 2;
            }
            "--depth" => {
                p.limits.max_depth = value(i)?.parse().map_err(|e| format!("bad depth: {e}"))?;
                i += 2;
            }
            "--max-nodes" => {
                p.limits.max_nodes = value(i)?.parse().map_err(|e| format!("bad value: {e}"))?;
                i += 2;
            }
            "--max-leaves" => {
                p.limits.max_leaves = value(i)?.parse().map_err(|e| format!("bad value: {e}"))?;
                i += 2;
            }
            "--max-vertices" => {
                p.limits.max_vertices = value(i)?.parse().map_err(|e| format!("bad value: {e}"))?;
                i += 2;
            }
            "--max-triangles" => {
                p.limits.max_triangles = value(i)?.parse().map_err(|e| format!("bad value: {e}"))?;
                i += 2;
            }
            "--segments" => {
                p.segments = value(i)?.parse().map_err(|e| format!("bad value: {e}"))?;
                i += 2;
            }
            "--sides" => {
                p.radial_sides = value(i)?.parse().map_err(|e| format!("bad value: {e}"))?;
                i += 2;
            }
            "--leaf-per-tip" => {
                p.leaves.per_tip = value(i)?.parse().map_err(|e| format!("bad value: {e}"))?;
                i += 2;
            }
            "--no-leaves" => {
                p.leaves.enabled = false;
                i += 1;
            }
            "--out-dir" => {
                out_dir = PathBuf::from(value(i)?.clone());
                i += 2;
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    Ok(GenArgs { params: p, out_dir })
}

fn generate_full(params: PlantParams) -> Result<(plant_core::Plant, plant_core::BuildStats, plant_core::MeshStats), String> {
    let params = params.validated().map_err(|e| e.to_string())?;
    let (mut plant, build_stats) = build_plant(params).map_err(|e| e.to_string())?;
    let (meshes, tube_stats) = build_meshes(&plant).map_err(|e| e.to_string())?;
    let (leaves, leaf_stats) = build_leaves(&plant);
    plant.leaves = leaves;
    validate_plant(&plant).map_err(|e| e.to_string())?;
    let stats = merge_mesh_stats(&tube_stats, &leaf_stats);
    // meshes are kept alive through validation; drop explicitly to make the
    // separation of structure and rendering obvious.
    drop(meshes);
    Ok((plant, build_stats, stats))
}

fn cmd_generate(rest: Vec<String>) -> Result<(), String> {
    let args = parse_generate_args(&rest)?;
    let (plant, build_stats, mesh_stats) = generate_full(args.params.clone())?;
    export_plant(&args.out_dir, &plant)?;
    println!(
        "generated: nodes={} depth={} leaves={} pruned(degen={}, depth={}, budget={})",
        plant.node_count(),
        build_stats.depth_reached,
        plant.leaves.len(),
        build_stats.pruned_degenerate,
        build_stats.pruned_depth,
        build_stats.pruned_node_budget
    );
    println!(
        "mesh: vertices={} triangles={} (tube v={}/t={}, leaf v={}/t={}) budget_hits[v={},t={},leaf={}]",
        mesh_stats.vertices,
        mesh_stats.triangles,
        mesh_stats.tube_vertices,
        mesh_stats.tube_triangles,
        mesh_stats.leaf_vertices,
        mesh_stats.leaf_triangles,
        mesh_stats.vertex_budget_hit,
        mesh_stats.triangle_budget_hit,
        mesh_stats.leaf_budget_hit
    );
    Ok(())
}

fn export_plant(out_dir: &PathBuf, plant: &plant_core::Plant) -> Result<(), String> {
    fs::create_dir_all(out_dir).map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    fs::write(out_dir.join("params.json"), export::write_params_archive(&plant.params))
        .map_err(|e| e.to_string())?;
    fs::write(out_dir.join("plant.json"), export::write_plant_json(plant))
        .map_err(|e| e.to_string())?;
    let (obj, manifest) = export::write_obj(plant).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("plant.obj"), obj).map_err(|e| e.to_string())?;
    fs::write(out_dir.join("plant.obj.manifest.json"), manifest).map_err(|e| e.to_string())?;
    println!(
        "wrote: {}/params.json, plant.json, plant.obj (+manifest)",
        out_dir.display()
    );
    Ok(())
}

fn cmd_validate(rest: Vec<String>) -> Result<(), String> {
    let path = rest.first().ok_or("usage: plant validate <plant.json>")?;
    let text = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let mut plant = export::read_plant_json(&text).map_err(|e| e.to_string())?;
    // Re-run geometry generation checks against the re-imported structure.
    if plant.leaves.is_empty() && plant.params.leaves.enabled {
        let (leaves, _) = build_leaves(&plant);
        plant.leaves = leaves;
    }
    validate_plant(&plant).map_err(|e| e.to_string())?;
    println!(
        "valid: {} nodes, {} leaves, depth {} — all geometric checks passed",
        plant.node_count(),
        plant.leaves.len(),
        plant.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    );
    Ok(())
}

fn cmd_regenerate(rest: Vec<String>) -> Result<(), String> {
    let path = rest.first().ok_or("usage: plant regenerate <params.json> [--out-dir <path>]")?;
    let text = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let params = export::read_params_archive(&text).map_err(|e| e.to_string())?;
    let out_dir = match rest.iter().position(|a| a == "--out-dir") {
        Some(i) => PathBuf::from(rest.get(i + 1).ok_or("--out-dir needs a value")?),
        None => PathBuf::from("rebuilt-out"),
    };
    let (plant, _, _) = generate_full(params.clone())?;
    export_plant(&out_dir, &plant)?;

    // Determinism cross-check: build again in the same process and diff.
    let (plant2, _, _) = generate_full(params)?;
    if plant == plant2 {
        println!("determinism check: repeated generation is identical");
    } else {
        return Err("determinism check failed: repeated generation differed".into());
    }
    Ok(())
}

fn cmd_stress(rest: Vec<String>) -> Result<(), String> {
    let out_dir = match rest.iter().position(|a| a == "--out-dir") {
        Some(i) => PathBuf::from(rest.get(i + 1).ok_or("--out-dir needs a value")?),
        None => PathBuf::from("stress-out"),
    };
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let cases: Vec<(String, PlantParams)> = vec![
        ("fixed_seed".into(), PlantParams::default()),
        (
            "zero_branches".into(),
            PlantParams {
                branch_count: 0,
                ..PlantParams::default()
            },
        ),
        (
            "sharp_angle".into(),
            PlantParams {
                branch_angle: std::f32::consts::PI - 0.01,
                angle_jitter: 0.0,
                ..PlantParams::default()
            },
        ),
        (
            "many_small_leaves".into(),
            PlantParams {
                branch_count: 4,
                leaves: LeafParams {
                    enabled: true,
                    per_tip: 16,
                    length: 0.04,
                    width: 0.015,
                    lift: 0.7,
                },
                limits: Limits {
                    max_depth: 5,
                    max_nodes: 2_000,
                    max_leaves: 1_000,
                    max_vertices: 80_000,
                    max_triangles: 40_000,
                },
                ..PlantParams::default()
            },
        ),
        (
            "tiny_budget".into(),
            PlantParams {
                branch_count: 64,
                limits: Limits {
                    max_depth: 3,
                    max_nodes: 8,
                    max_leaves: 10,
                    max_vertices: 5_000,
                    max_triangles: 3_000,
                },
                ..PlantParams::default()
            },
        ),
    ];

    for (name, params) in cases {
        let (plant, build_stats, mesh_stats) = match generate_full(params.clone()) {
            Ok(v) => v,
            Err(e) => return Err(format!("case {name} failed: {e}")),
        };
        // Reproducibility inside the case itself.
        let (again, _, _) = generate_full(params)?;
        assert_eq!(plant, again, "case {name} not reproducible");
        println!(
            "[{name}] nodes={} leaves={} v={} t={} pruned(d={},b={},n={}) budgets_hit(v={},t={},l={})",
            plant.node_count(),
            plant.leaves.len(),
            mesh_stats.vertices,
            mesh_stats.triangles,
            build_stats.pruned_degenerate,
            build_stats.pruned_depth,
            build_stats.pruned_node_budget,
            mesh_stats.vertex_budget_hit,
            mesh_stats.triangle_budget_hit,
            mesh_stats.leaf_budget_hit
        );
        let case_dir = out_dir.join(&name);
        export_plant(&case_dir, &plant)?;
    }
    println!("stress suite: all cases valid and reproducible");
    Ok(())
}

// ---------- rule DSL: migration, generation, verification ----------

fn flag_value<'a>(rest: &'a [String], flag: &str) -> Option<&'a String> {
    rest.iter()
        .position(|a| a == flag)
        .and_then(|i| rest.get(i + 1))
}

fn has_flag(rest: &[String], flag: &str) -> bool {
    rest.iter().any(|a| a == flag)
}

/// Compile either rule syntax into the single shared [`RulePlan`]. Both
/// syntaxes funnel into the same core builder — there is no second generator.
fn compile_rule_file(path: &str) -> Result<(RulePlan, String), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let doc = RuleDocument::tokenize(&text).map_err(|e| e.to_string())?;
    let plan = match doc.format.as_str() {
        FORMAT_V1 => plant_core::rules::parse_legacy(&doc)
            .map_err(|e| e.to_string())?
            .plan,
        FORMAT_V2 => plant_core::rules::parse_v2(&doc).map_err(|e| e.to_string())?,
        other => return Err(format!("unknown rule format '{other}'")),
    };
    Ok((plan, text))
}

fn cmd_rule_migrate(rest: Vec<String>) -> Result<(), String> {
    let src = rest
        .first()
        .ok_or("usage: plant rule-migrate <legacy.rules> [--out <v2.rules>] [--report <report.json>] [--strict]")?;
    let out_path = flag_value(&rest, "--out")
        .map(String::clone)
        .unwrap_or_else(|| src.replace(".rules", ".v2.rules"));
    let report_path = flag_value(&rest, "--report")
        .map(String::clone)
        .unwrap_or_else(|| format!("{out_path}.migration.json"));
    let strict = has_flag(&rest, "--strict");

    let legacy_text = fs::read_to_string(src).map_err(|e| format!("read {src}: {e}"))?;

    // Rollback copy: the original project file is preserved before any
    // overwrite, so the migration is always reversible.
    let backup_path = format!("{src}.v1.bak");
    if !PathBuf::from(&backup_path).exists() {
        fs::write(&backup_path, &legacy_text).map_err(|e| format!("backup failed: {e}"))?;
        println!("backup: {backup_path} (rollback copy of the v1 source)");
    } else {
        println!("backup: {backup_path} already exists; leaving it untouched");
    }

    let report = migrate(&legacy_text).map_err(|e| e.to_string())?;
    fs::write(&out_path, &report.migrated_text).map_err(|e| e.to_string())?;
    println!("migrated: {src} -> {out_path} ({})", report.target_format);

    println!("seed preserved: {} (u64, unchanged)", report.seed);
    println!(
        "implicit defaults made explicit: {}",
        report.implicit_defaults.len()
    );
    for note in &report.implicit_defaults {
        println!("  explicit: {note}");
    }
    println!("material bindings migrated: {}", report.materials_migrated.len());
    for m in &report.materials_migrated {
        println!("  binding: {m}");
    }

    if report.pending.is_empty() {
        println!("pending rules: none (every directive mapped)");
    } else {
        println!("pending rules: {} (not defaulted, preserved as comments)", report.pending.len());
        for item in &report.pending {
            println!(
                "  PENDING [line {}] '{}': {}",
                item.line_no, item.source_text, item.reason
            );
        }
        if strict {
            return Err("strict migration failed: unmapped rules remain pending".into());
        }
    }

    let report_json = render_migration_report(&report);
    fs::write(&report_path, report_json).map_err(|e| e.to_string())?;
    println!("report: {report_path}");
    Ok(())
}

fn render_migration_report(report: &plant_core::rules::MigrationReport) -> String {
    use plant_core::json::Json;
    let s = |v: &str| Json::Str(v.to_string());
    let pending = Json::Array(
        report
            .pending
            .iter()
            .map(|p| {
                Json::Object(vec![
                    ("line_no".into(), Json::UInt(p.line_no as u64)),
                    ("source_text".into(), s(&p.source_text)),
                    ("reason".into(), s(&p.reason)),
                ])
            })
            .collect(),
    );
    let doc = Json::Object(vec![
        ("format".into(), s("rule-migration-report/v1")),
        ("source_format".into(), s(&report.source_format)),
        ("target_format".into(), s(&report.target_format)),
        ("migration_version".into(), s(plant_core::rules::MIGRATION_VERSION)),
        ("seed".into(), Json::UInt(report.seed)),
        (
            "implicit_defaults".into(),
            Json::Array(report.implicit_defaults.iter().map(|n| s(n)).collect()),
        ),
        (
            "materials_migrated".into(),
            Json::Array(report.materials_migrated.iter().map(|n| s(n)).collect()),
        ),
        ("pending".into(), pending),
        (
            "geometry_verified".into(),
            Json::Bool(report.geometry.is_some()),
        ),
    ]);
    plant_core::json::to_string(&doc)
}

fn cmd_rule_generate(rest: Vec<String>) -> Result<(), String> {
    let rule_path = rest
        .first()
        .ok_or("usage: plant rule-generate <file.rules> --out-dir <dir>")?;
    let out_dir = PathBuf::from(
        flag_value(&rest, "--out-dir")
            .cloned()
            .unwrap_or_else(|| "rule-out".into()),
    );
    let (plan, _) = compile_rule_file(rule_path)?;
    let params = plan.to_params().validated().map_err(|e| e.to_string())?;
    let (plant, build_stats, mesh_stats) = generate_full(params)?;

    // Rule-level semantics checked on the real geometry: layer identity and
    // world-space extent of every material binding.
    let evidence = rule_geometry_evidence(&plan, &plant);
    export_plant(&out_dir, &plant)?;
    export_rule_artifacts(&out_dir, &plan, &evidence)?;

    println!(
        "rule-generate [{}]: nodes={} depth={} leaves={} pruned(d={},b={},n={})",
        rule_path,
        plant.node_count(),
        build_stats.depth_reached,
        plant.leaves.len(),
        build_stats.pruned_degenerate,
        build_stats.pruned_depth,
        build_stats.pruned_node_budget
    );
    for b in &evidence.bindings {
        let extent = match (b.bounds_min, b.bounds_max) {
            (Some(lo), Some(hi)) => format!(
                "x[{},{}] y[{},{}] z[{},{}]",
                lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
            ),
            _ => "NO MATCHED GEOMETRY".into(),
        };
        println!(
            "  material {} on {}: matched={} nodes={} extent={extent}",
            b.material,
            b.layer.render(),
            b.matched,
            b.matched_nodes.len()
        );
    }
    if !evidence.uncovered_woody_nodes.is_empty() {
        println!(
            "  uncovered woody nodes: {} (no material binding references their depth)",
            evidence.uncovered_woody_nodes.len()
        );
    }
    println!(
        "mesh: vertices={} triangles={} budget_hits[v={},t={},leaf={}]",
        mesh_stats.vertices,
        mesh_stats.triangles,
        mesh_stats.vertex_budget_hit,
        mesh_stats.triangle_budget_hit,
        mesh_stats.leaf_budget_hit
    );
    Ok(())
}

fn export_rule_artifacts(
    out_dir: &PathBuf,
    plan: &RulePlan,
    evidence: &plant_core::rules::RuleGeometryEvidence,
) -> Result<(), String> {
    use plant_core::json::Json;
    let s = |v: &str| Json::Str(v.to_string());
    let bindings = Json::Array(
        plan.materials
            .iter()
            .map(|b| {
                Json::Object(vec![
                    ("material".into(), s(&b.material)),
                    ("layer".into(), s(&b.layer.render())),
                ])
            })
            .collect(),
    );
    let evidence_json = Json::Array(
        evidence
            .bindings
            .iter()
            .map(|b| {
                let bounds = match (b.bounds_min, b.bounds_max) {
                    (Some(lo), Some(hi)) => Json::Object(vec![
                        ("min".into(), Json::Array(lo.iter().map(|x| Json::Float(*x)).collect())),
                        ("max".into(), Json::Array(hi.iter().map(|x| Json::Float(*x)).collect())),
                    ]),
                    _ => Json::Null,
                };
                Json::Object(vec![
                    ("material".into(), s(&b.material)),
                    ("layer".into(), s(&b.layer.render())),
                    ("matched".into(), Json::Bool(b.matched)),
                    (
                        "matched_nodes".into(),
                        Json::Array(b.matched_nodes.iter().map(|n| Json::UInt(*n as u64)).collect()),
                    ),
                    ("world_bounds".into(), bounds),
                ])
            })
            .collect(),
    );
    let doc = Json::Object(vec![
        ("format".into(), s("rule-material-bindings/v1")),
        ("seed".into(), Json::UInt(plan.params.seed)),
        ("bindings".into(), bindings),
        ("geometry_evidence".into(), evidence_json),
        (
            "uncovered_woody_nodes".into(),
            Json::Array(
                evidence
                    .uncovered_woody_nodes
                    .iter()
                    .map(|n| Json::UInt(*n as u64))
                    .collect(),
            ),
        ),
    ]);
    fs::write(
        out_dir.join("rule-bindings.json"),
        plant_core::json::to_string(&doc),
    )
    .map_err(|e| e.to_string())
}

fn cmd_rule_verify(rest: Vec<String>) -> Result<(), String> {
    let legacy_path = rest
        .first()
        .ok_or("usage: plant rule-verify <legacy.rules> [--migrated <v2.rules>]")?;
    let legacy_text = fs::read_to_string(legacy_path).map_err(|e| format!("read {legacy_path}: {e}"))?;

    // Default: perform the migration in-memory so verification also covers the
    // converter output. An explicit --migrated path verifies a file on disk.
    let migrated_text = match flag_value(&rest, "--migrated") {
        Some(path) => fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?,
        None => migrate(&legacy_text).map_err(|e| e.to_string())?.migrated_text,
    };

    let mut evidence = Vec::new();
    let fingerprint = verify_geometry(&legacy_text, &migrated_text, Some(&mut evidence))
        .map_err(|e| e.to_string())?;

    println!("geometry verification (built from real generated plants):");
    println!(
        "  legacy:   seed={} nodes={} depth={} leaves={} spline_samples={}",
        fingerprint.legacy.seed,
        fingerprint.legacy.node_count,
        fingerprint.legacy.max_depth,
        fingerprint.legacy.leaf_count,
        fingerprint.legacy.spline_sample_count
    );
    println!(
        "  explicit: seed={} nodes={} depth={} leaves={} spline_samples={}",
        fingerprint.explicit.seed,
        fingerprint.explicit.node_count,
        fingerprint.explicit.max_depth,
        fingerprint.explicit.leaf_count,
        fingerprint.explicit.spline_sample_count
    );
    println!("  result: {}", fingerprint.comparison);
    if !fingerprint.equal {
        return Err("rule migration verification failed: geometry diverged".into());
    }

    // The seed match is necessary but not sufficient; the equality decision was
    // made on geometry above. Surface the layer/extent evidence for review.
    for (tag, ev) in [("legacy", &evidence[0]), ("explicit", &evidence[1])] {
        println!("  [{tag}] attachment evidence ({} child branches):", ev.attachments.len());
        for a in ev.attachments.iter().take(8) {
            println!(
                "    node {} depth {} -> parent {} sample {} t={:.3} weld={:.2e}",
                a.node_id, a.depth, a.parent_id, a.parent_sample_index, a.measured_t, a.weld_distance
            );
        }
    }
    println!("verification passed: identical generated geometry on both rule paths");
    Ok(())
}
