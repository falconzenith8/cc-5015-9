//! Export and re-import.
//!
//! Two distinct artifacts are supported:
//!
//! * **hierarchy JSON** — the editable procedural result: the full node tree
//!   with parent ids, local/world transforms, centerlines, per-node seeds and
//!   leaf instances, plus the parameter archive. A re-import yields a plant
//!   equal to the exported one, and the archive can be fed back into the
//!   builder to regenerate it from scratch.
//! * **baked OBJ** — flattened render geometry with per-branch/per-leaf
//!   groups; a sidecar manifest records the node id each group came from, so
//!   the hierarchy information is not silently destroyed.

use crate::builder::{build_meshes, shared_leaf_mesh};
use crate::error::PlantError;
use crate::geometry::bake_world;
use crate::json::{self, Json};
use crate::math::{Mat4, Vec3};
use crate::model::{LeafInstance, Plant, PruneReason};
use crate::params::{LeafParams, Limits, PlantParams};

// ---------- parameter archive ----------

pub fn params_to_json(p: &PlantParams) -> Json {
    Json::Object(vec![
        ("seed".into(), Json::UInt(p.seed)),
        ("trunk_length".into(), Json::Float(p.trunk_length)),
        ("trunk_radius".into(), Json::Float(p.trunk_radius)),
        ("branch_count".into(), Json::UInt(p.branch_count as u64)),
        ("branch_angle".into(), Json::Float(p.branch_angle)),
        ("length_ratio".into(), Json::Float(p.length_ratio)),
        ("radius_ratio".into(), Json::Float(p.radius_ratio)),
        ("taper".into(), Json::Float(p.taper)),
        ("angle_jitter".into(), Json::Float(p.angle_jitter)),
        ("length_jitter".into(), Json::Float(p.length_jitter)),
        ("bend".into(), Json::Float(p.bend)),
        ("segments".into(), Json::UInt(p.segments as u64)),
        ("radial_sides".into(), Json::UInt(p.radial_sides as u64)),
        ("leaves".into(), leaf_params_to_json(&p.leaves)),
        ("limits".into(), limits_to_json(&p.limits)),
    ])
}

fn leaf_params_to_json(l: &LeafParams) -> Json {
    Json::Object(vec![
        ("enabled".into(), Json::Bool(l.enabled)),
        ("per_tip".into(), Json::UInt(l.per_tip as u64)),
        ("length".into(), Json::Float(l.length)),
        ("width".into(), Json::Float(l.width)),
        ("lift".into(), Json::Float(l.lift)),
    ])
}

fn limits_to_json(l: &Limits) -> Json {
    Json::Object(vec![
        ("max_depth".into(), Json::UInt(l.max_depth as u64)),
        ("max_nodes".into(), Json::UInt(l.max_nodes as u64)),
        ("max_leaves".into(), Json::UInt(l.max_leaves as u64)),
        ("max_vertices".into(), Json::UInt(l.max_vertices as u64)),
        ("max_triangles".into(), Json::UInt(l.max_triangles as u64)),
    ])
}

pub fn params_from_json(v: &Json) -> Result<PlantParams, PlantError> {
    let f = |key: &str| -> Result<f32, PlantError> {
        v.must_get(key)?.as_f32().ok_or_else(|| PlantError::Json(format!("field '{key}' is not a number")))
    };
    let u = |key: &str| -> Result<u64, PlantError> {
        v.must_get(key)?.as_u64().ok_or_else(|| PlantError::Json(format!("field '{key}' is not an integer")))
    };
    let leaves_v = v.must_get("leaves")?;
    let limits_v = v.must_get("limits")?;
    Ok(PlantParams {
        seed: u("seed")?,
        trunk_length: f("trunk_length")?,
        trunk_radius: f("trunk_radius")?,
        branch_count: u("branch_count")? as u32,
        branch_angle: f("branch_angle")?,
        length_ratio: f("length_ratio")?,
        radius_ratio: f("radius_ratio")?,
        taper: f("taper")?,
        angle_jitter: f("angle_jitter")?,
        length_jitter: f("length_jitter")?,
        bend: f("bend")?,
        segments: u("segments")? as u32,
        radial_sides: u("radial_sides")? as u32,
        leaves: LeafParams {
            enabled: leaves_v.must_get("enabled")?.as_bool().ok_or_else(|| PlantError::Json("leaves.enabled".into()))?,
            per_tip: leaves_v.must_get("per_tip")?.as_u64().ok_or_else(|| PlantError::Json("leaves.per_tip".into()))? as u32,
            length: leaves_v.must_get("length")?.as_f32().ok_or_else(|| PlantError::Json("leaves.length".into()))?,
            width: leaves_v.must_get("width")?.as_f32().ok_or_else(|| PlantError::Json("leaves.width".into()))?,
            lift: leaves_v.must_get("lift")?.as_f32().ok_or_else(|| PlantError::Json("leaves.lift".into()))?,
        },
        limits: Limits {
            max_depth: limits_v.must_get("max_depth")?.as_u64().ok_or_else(|| PlantError::Json("limits.max_depth".into()))? as u32,
            max_nodes: limits_v.must_get("max_nodes")?.as_u64().ok_or_else(|| PlantError::Json("limits.max_nodes".into()))? as usize,
            max_leaves: limits_v.must_get("max_leaves")?.as_u64().ok_or_else(|| PlantError::Json("limits.max_leaves".into()))? as usize,
            max_vertices: limits_v.must_get("max_vertices")?.as_u64().ok_or_else(|| PlantError::Json("limits.max_vertices".into()))? as usize,
            max_triangles: limits_v.must_get("max_triangles")?.as_u64().ok_or_else(|| PlantError::Json("limits.max_triangles".into()))? as usize,
        },
    })
}

pub fn write_params_archive(params: &PlantParams) -> String {
    let doc = Json::Object(vec![
        ("format".into(), Json::Str("plant-params/v1".into())),
        ("params".into(), params_to_json(params)),
    ]);
    json::to_string(&doc)
}

pub fn read_params_archive(text: &str) -> Result<PlantParams, PlantError> {
    let doc = json::parse(text)?;
    if doc.get("format").and_then(|v| v.as_str()) != Some("plant-params/v1") {
        return Err(PlantError::Json("not a plant-params/v1 archive".into()));
    }
    params_from_json(doc.must_get("params")?).and_then(|p| p.validated())
}

// ---------- hierarchical plant export ----------

fn vec3_to_json(v: Vec3) -> Json {
    Json::Array(vec![Json::Float(v.x), Json::Float(v.y), Json::Float(v.z)])
}

fn vec3_from_json(v: &Json) -> Result<Vec3, PlantError> {
    let a = v.as_array().ok_or_else(|| PlantError::Json("expected [x,y,z]".into()))?;
    if a.len() != 3 {
        return Err(PlantError::Json("vec3 must have 3 components".into()));
    }
    Ok(Vec3::new(
        a[0].as_f32().ok_or_else(|| PlantError::Json("vec3.x".into()))?,
        a[1].as_f32().ok_or_else(|| PlantError::Json("vec3.y".into()))?,
        a[2].as_f32().ok_or_else(|| PlantError::Json("vec3.z".into()))?,
    ))
}

fn mat4_to_json(m: &Mat4) -> Json {
    Json::Object(vec![
        ("x_axis".into(), vec3_to_json(m.cols[0])),
        ("y_axis".into(), vec3_to_json(m.cols[1])),
        ("z_axis".into(), vec3_to_json(m.cols[2])),
        ("translation".into(), vec3_to_json(m.translation)),
        ("scale".into(), Json::Float(m.scale)),
    ])
}

fn mat4_from_json(v: &Json) -> Result<Mat4, PlantError> {
    Ok(Mat4 {
        cols: [
            vec3_from_json(v.must_get("x_axis")?)?,
            vec3_from_json(v.must_get("y_axis")?)?,
            vec3_from_json(v.must_get("z_axis")?)?,
        ],
        translation: vec3_from_json(v.must_get("translation")?)?,
        scale: v.must_get("scale")?.as_f32().ok_or_else(|| PlantError::Json("scale".into()))?,
    })
}

fn node_to_json(node: &crate::model::BranchNode) -> Json {
    Json::Object(vec![
        ("id".into(), Json::UInt(node.id as u64)),
        (
            "parent".into(),
            match node.parent {
                Some(p) => Json::UInt(p as u64),
                None => Json::Null,
            },
        ),
        ("depth".into(), Json::UInt(node.depth as u64)),
        ("local".into(), mat4_to_json(&node.local)),
        ("world".into(), mat4_to_json(&node.world)),
        ("length".into(), Json::Float(node.length)),
        ("base_radius".into(), Json::Float(node.base_radius)),
        ("tip_radius".into(), Json::Float(node.tip_radius)),
        (
            "spline".into(),
            Json::Array(node.spline.iter().map(|&p| vec3_to_json(p)).collect()),
        ),
        ("seed".into(), Json::UInt(node.seed)),
        (
            "children".into(),
            Json::Array(node.children.iter().map(|c| Json::UInt(*c as u64)).collect()),
        ),
        ("terminal".into(), Json::Bool(node.terminal)),
    ])
}

fn leaf_to_json(leaf: &LeafInstance) -> Json {
    Json::Object(vec![
        ("node_id".into(), Json::UInt(leaf.node_id as u64)),
        ("world".into(), mat4_to_json(&leaf.world)),
        ("anchor".into(), vec3_to_json(leaf.anchor)),
        ("normal".into(), vec3_to_json(leaf.normal)),
    ])
}

/// Full editable export. Leaves are baked as explicit instances; the prune log
/// is included so omitted branches remain explainable.
pub fn write_plant_json(plant: &Plant) -> String {
    let nodes = Json::Array(plant.nodes.iter().map(node_to_json).collect());
    let leaves = Json::Array(plant.leaves.iter().map(leaf_to_json).collect());
    let pruned = Json::Array(
        plant
            .pruned
            .iter()
            .map(|r| {
                Json::Object(vec![
                    ("parent_id".into(), Json::UInt(r.parent_id as u64)),
                    ("child_index".into(), Json::UInt(r.child_index as u64)),
                    (
                        "reason".into(),
                        Json::Str(match r.reason {
                            PruneReason::Degenerate => "degenerate",
                            PruneReason::DepthLimit => "depth_limit",
                            PruneReason::NodeBudget => "node_budget",
                        }
                        .into()),
                    ),
                ])
            })
            .collect(),
    );
    let doc = Json::Object(vec![
        ("format".into(), Json::Str("plant-hierarchy/v1".into())),
        ("params".into(), params_to_json(&plant.params)),
        ("nodes".into(), nodes),
        ("leaves".into(), leaves),
        ("pruned".into(), pruned),
    ]);
    json::to_string(&doc)
}

/// Re-import a hierarchy export, preserving the exact stored transforms and
/// geometry rather than regenerating.
pub fn read_plant_json(text: &str) -> Result<Plant, PlantError> {
    let doc = json::parse(text)?;
    if doc.get("format").and_then(|v| v.as_str()) != Some("plant-hierarchy/v1") {
        return Err(PlantError::Json("not a plant-hierarchy/v1 document".into()));
    }
    let params = params_from_json(doc.must_get("params")?)?.validated()?;
    let nodes_v = doc
        .must_get("nodes")?
        .as_array()
        .ok_or_else(|| PlantError::Json("nodes must be an array".into()))?;
    let mut nodes = Vec::with_capacity(nodes_v.len());
    for nv in nodes_v {
        let spline_v = nv
            .must_get("spline")?
            .as_array()
            .ok_or_else(|| PlantError::Json("spline array".into()))?;
        let spline = spline_v
            .iter()
            .map(vec3_from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let children = nv
            .must_get("children")?
            .as_array()
            .ok_or_else(|| PlantError::Json("children array".into()))?
            .iter()
            .map(|c| c.as_u64().ok_or_else(|| PlantError::Json("child id".into())))
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .map(|v| *v as usize)
            .collect();
        nodes.push(crate::model::BranchNode {
            id: nv.must_get("id")?.as_u64().ok_or_else(|| PlantError::Json("id".into()))? as usize,
            parent: match nv.must_get("parent")? {
                Json::Null => None,
                other => Some(other.as_u64().ok_or_else(|| PlantError::Json("parent".into()))? as usize),
            },
            depth: nv.must_get("depth")?.as_u64().ok_or_else(|| PlantError::Json("depth".into()))? as u32,
            local: mat4_from_json(nv.must_get("local")?)?,
            world: mat4_from_json(nv.must_get("world")?)?,
            length: nv.must_get("length")?.as_f32().ok_or_else(|| PlantError::Json("length".into()))?,
            base_radius: nv.must_get("base_radius")?.as_f32().ok_or_else(|| PlantError::Json("base_radius".into()))?,
            tip_radius: nv.must_get("tip_radius")?.as_f32().ok_or_else(|| PlantError::Json("tip_radius".into()))?,
            spline,
            seed: nv.must_get("seed")?.as_u64().ok_or_else(|| PlantError::Json("seed".into()))?,
            children,
            terminal: nv.must_get("terminal")?.as_bool().ok_or_else(|| PlantError::Json("terminal".into()))?,
        });
    }
    let leaves_v = doc.must_get("leaves")?.as_array().cloned().unwrap_or_default();
    let mut leaves = Vec::with_capacity(leaves_v.len());
    for lv in &leaves_v {
        leaves.push(LeafInstance {
            node_id: lv.must_get("node_id")?.as_u64().ok_or_else(|| PlantError::Json("leaf node_id".into()))? as usize,
            world: mat4_from_json(lv.must_get("world")?)?,
            anchor: vec3_from_json(lv.must_get("anchor")?)?,
            normal: vec3_from_json(lv.must_get("normal")?)?,
        });
    }
    Ok(Plant {
        params,
        nodes,
        leaves,
        pruned: Vec::new(),
    })
}

// ---------- baked OBJ ----------

/// Bake the whole plant into one OBJ string. Groups (`g branch_<id>`,
/// `g leaf_<n>`) preserve the origin of every face; the returned manifest JSON
/// maps group names back to node ids and instance indices.
pub fn write_obj(plant: &Plant) -> Result<(String, String), PlantError> {
    let (tubes, _) = build_meshes(plant)?;
    let leaf_mesh = shared_leaf_mesh(&plant.params);

    let mut obj = String::new();
    let mut manifest_entries: Vec<(String, Json)> = Vec::new();
    let mut vertex_offset: u32 = 0; // OBJ indices are 1-based

    obj.push_str("# procedural plant, baked geometry\n");
    obj.push_str("# groups preserve node ids; see sidecar manifest\n");

    for (node, mesh) in plant.nodes.iter().zip(tubes.iter()) {
        if mesh.vertices.is_empty() {
            continue;
        }
        obj.push_str(&format!("g branch_{}\n", node.id));
        for v in &mesh.vertices {
            obj.push_str(&format!(
                "v {} {} {}\n",
                json::format_f32(v.x),
                json::format_f32(v.y),
                json::format_f32(v.z)
            ));
        }
        for tri in &mesh.triangles {
            obj.push_str(&format!(
                "f {} {} {}\n",
                vertex_offset + tri[0] + 1,
                vertex_offset + tri[1] + 1,
                vertex_offset + tri[2] + 1
            ));
        }
        manifest_entries.push((
            format!("branch_{}", node.id),
            Json::Object(vec![
                ("kind".into(), Json::Str("branch".into())),
                ("node_id".into(), Json::UInt(node.id as u64)),
                ("depth".into(), Json::UInt(node.depth as u64)),
            ]),
        ));
        vertex_offset += mesh.vertices.len() as u32;
    }

    for (i, leaf) in plant.leaves.iter().enumerate() {
        let baked = bake_world(&leaf_mesh, leaf.world);
        obj.push_str(&format!("g leaf_{i}\n"));
        for v in &baked.vertices {
            obj.push_str(&format!(
                "v {} {} {}\n",
                json::format_f32(v.x),
                json::format_f32(v.y),
                json::format_f32(v.z)
            ));
        }
        for tri in &baked.triangles {
            obj.push_str(&format!(
                "f {} {} {}\n",
                vertex_offset + tri[0] + 1,
                vertex_offset + tri[1] + 1,
                vertex_offset + tri[2] + 1
            ));
        }
        manifest_entries.push((
            format!("leaf_{i}"),
            Json::Object(vec![
                ("kind".into(), Json::Str("leaf".into())),
                ("instance".into(), Json::UInt(i as u64)),
                ("node_id".into(), Json::UInt(leaf.node_id as u64)),
            ]),
        ));
        vertex_offset += baked.vertices.len() as u32;
    }

    let manifest = json::to_string(&Json::Object(vec![
        ("format".into(), Json::Str("plant-obj-manifest/v1".into())),
        ("seed".into(), Json::UInt(plant.params.seed)),
        ("groups".into(), Json::Object(manifest_entries)),
    ]));
    Ok((obj, manifest))
}
