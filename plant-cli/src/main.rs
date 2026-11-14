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
