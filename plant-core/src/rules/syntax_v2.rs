//! Explicit `plant-rules/v2` parser.
//!
//! The v2 contract removes every implicit convention:
//!
//! * every dimensional value is written `value<unit>` (`6m`, `35cm`,
//!   `0.7rad`, `40deg`) — a bare number where a unit belongs is a hard error;
//! * dimensionless ratios are plain fractions with a leading `ratio.` field
//!   name so they can never be confused with lengths;
//! * branch references and material layers are qualified selectors
//!   (`target=branch:trunk`, `layer=branch:1..`, `layer=leaves`).
//!
//! Lines use a uniform `directive key=value [key=value ...]` spelling.

use crate::error::PlantError;
use crate::params::PlantParams;

use super::ast::{
    split_key_value, AttachRule, AttachTarget, LayerSelector, LegacyMeasure, RuleDocument,
};
use super::{rule_err, FieldDisplays, MaterialBinding, MeasureUnit, RulePlan, FORMAT_V2};

/// Parse a v2 document into the shared [`RulePlan`]. Unlike the legacy path
/// there is no audit list: nothing is inferred, so there is nothing to audit.
pub fn parse_v2(doc: &RuleDocument) -> Result<RulePlan, PlantError> {
    if doc.format != FORMAT_V2 {
        return rule_err(format!("parse_v2 requires {FORMAT_V2}, got {}", doc.format));
    }
    let mut params = PlantParams::default();
    let mut materials = Vec::new();
    let mut attach: Option<AttachRule> = None;
    let mut displays = FieldDisplays::default();
    let mut seed_seen = false;

    for line in &doc.lines {
        let head = line.tokens[0].as_str();
        let pairs = parse_pairs(&line.tokens[1..], line.line_no)?;
        let get = |key: &str| -> Result<&String, PlantError> {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v)
                .ok_or_else(|| {
                    PlantError::Rule(format!(
                        "line {}: '{head}' is missing required key '{key}'",
                        line.line_no
                    ))
                })
        };
        match head {
            "seed" => {
                require_keys(&pairs, &["value"], line.line_no, head)?;
                params.seed = get("value")?
                    .parse::<u64>()
                    .map_err(|_| PlantError::Rule(format!("line {}: bad seed", line.line_no)))?;
                seed_seen = true;
            }
            "trunk" => {
                require_keys(&pairs, &["length", "radius"], line.line_no, head)?;
                params.trunk_length =
                    explicit_length(get("length")?, line.line_no, "trunk.length", &mut displays)?;
                params.trunk_radius =
                    explicit_length(get("radius")?, line.line_no, "trunk.radius", &mut displays)?;
            }
            "branch" => {
                for (key, value) in &pairs {
                    match key.as_str() {
                        "count" => params.branch_count = parse_int(value, line.line_no)?,
                        "angle" => {
                            params.branch_angle =
                                explicit_angle(value, line.line_no, "branch.angle", &mut displays)?
                        }
                        "jitter" => {
                            params.angle_jitter =
                                explicit_angle(value, line.line_no, "branch.jitter", &mut displays)?
                        }
                        "ratio.length" => params.length_ratio = ratio(value, line.line_no)?,
                        "jitter.length" => params.length_jitter = ratio(value, line.line_no)?,
                        "ratio.radius" => params.radius_ratio = ratio(value, line.line_no)?,
                        "taper" => params.taper = ratio(value, line.line_no)?,
                        "bend" => params.bend = ratio(value, line.line_no)?,
                        "segments" => params.segments = parse_int(value, line.line_no)?,
                        "sides" => params.radial_sides = parse_int(value, line.line_no)?,
                        other => {
                            return rule_err(format!(
                                "line {}: unknown branch key '{other}'",
                                line.line_no
                            ))
                        }
                    }
                }
            }
            "leaf" => {
                for (key, value) in &pairs {
                    match key.as_str() {
                        "enabled" => {
                            params.leaves.enabled = parse_bool(value, line.line_no)?;
                        }
                        "per-tip" => params.leaves.per_tip = parse_int(value, line.line_no)?,
                        "length" => {
                            params.leaves.length =
                                explicit_length(value, line.line_no, "leaf.length", &mut displays)?
                        }
                        "width" => {
                            params.leaves.width =
                                explicit_length(value, line.line_no, "leaf.width", &mut displays)?
                        }
                        "lift" => params.leaves.lift = ratio(value, line.line_no)?,
                        other => {
                            return rule_err(format!(
                                "line {}: unknown leaf key '{other}'",
                                line.line_no
                            ))
                        }
                    }
                }
            }
            "limit" => {
                for (key, value) in &pairs {
                    let n = value
                        .parse::<usize>()
                        .map_err(|_| PlantError::Rule(format!("line {}: limit {key} must be an integer", line.line_no)))?;
                    match key.as_str() {
                        "depth" => params.limits.max_depth = n as u32,
                        "nodes" => params.limits.max_nodes = n,
                        "leaves" => params.limits.max_leaves = n,
                        "vertices" => params.limits.max_vertices = n,
                        "triangles" => params.limits.max_triangles = n,
                        other => {
                            return rule_err(format!(
                                "line {}: unknown limit key '{other}'",
                                line.line_no
                            ))
                        }
                    }
                }
            }
            "material" => {
                require_keys(&pairs, &["name", "layer"], line.line_no, head)?;
                let layer = LayerSelector::parse_qualified(get("layer")?)?;
                materials.push(MaterialBinding {
                    material: get("name")?.clone(),
                    layer,
                });
            }
            "attach" => {
                require_keys(&pairs, &["target", "at"], line.line_no, head)?;
                let target = AttachTarget::parse_qualified(get("target")?)?;
                let t = get("at")?
                    .parse::<f32>()
                    .map_err(|_| PlantError::Rule(format!("line {}: attach at must be a fraction", line.line_no)))?;
                if !(0.0..=1.0).contains(&t) {
                    return rule_err(format!("line {}: attach at must be in [0, 1]", line.line_no));
                }
                attach = Some(AttachRule { target, t });
            }
            other => {
                return rule_err(format!(
                    "line {}: unknown v2 directive '{other}'",
                    line.line_no
                ))
            }
        }
    }

    if !seed_seen {
        return rule_err("v2 rule document is missing required 'seed value=<u64>'");
    }
    Ok(RulePlan {
        params,
        materials,
        attach,
        displays,
    })
}

fn parse_pairs(
    tokens: &[String],
    line_no: usize,
) -> Result<Vec<(String, String)>, PlantError> {
    tokens
        .iter()
        .map(|tok| {
            let (k, v) = split_key_value(tok)?;
            Ok((k.to_string(), v.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e: PlantError| PlantError::Rule(format!("line {line_no}: {e}")))
}

fn require_keys(
    pairs: &[(String, String)],
    required: &[&str],
    line_no: usize,
    head: &str,
) -> Result<(), PlantError> {
    for key in required {
        if !pairs.iter().any(|(k, _)| k == key) {
            return rule_err(format!(
                "line {line_no}: '{head}' requires key '{key}'"
            ));
        }
    }
    Ok(())
}

fn explicit_length(
    token: &str,
    line_no: usize,
    field: &str,
    displays: &mut FieldDisplays,
) -> Result<f32, PlantError> {
    let measure = LegacyMeasure::parse(token)?;
    let resolved = measure
        .resolve_explicit()
        .map_err(|e| PlantError::Rule(format!("line {line_no}: {field}: {e}")))?;
    if !resolved.unit.is_length() {
        return rule_err(format!(
            "line {line_no}: {field} needs a length unit (m/cm/mm), got {}",
            resolved.unit.suffix()
        ));
    }
    displays.set(field, token.to_string());
    Ok(resolved.base_value())
}

fn explicit_angle(
    token: &str,
    line_no: usize,
    field: &str,
    displays: &mut FieldDisplays,
) -> Result<f32, PlantError> {
    let measure = LegacyMeasure::parse(token)?;
    let resolved = measure
        .resolve_explicit()
        .map_err(|e| PlantError::Rule(format!("line {line_no}: {field}: {e}")))?;
    if resolved.unit.is_length() {
        return rule_err(format!(
            "line {line_no}: {field} needs an angle unit (rad/deg), got {}",
            resolved.unit.suffix()
        ));
    }
    displays.set(field, token.to_string());
    Ok(resolved.base_value())
}

fn ratio(token: &str, line_no: usize) -> Result<f32, PlantError> {
    let measure = LegacyMeasure::parse(token)?;
    if measure.explicit_unit.is_some() {
        return rule_err(format!(
            "line {line_no}: dimensionless ratio '{token}' must not carry a unit suffix"
        ));
    }
    Ok(measure.number)
}

fn parse_int(token: &str, line_no: usize) -> Result<u32, PlantError> {
    token
        .parse::<u32>()
        .map_err(|_| PlantError::Rule(format!("line {line_no}: '{token}' must be an integer")))
}

fn parse_bool(token: &str, line_no: usize) -> Result<bool, PlantError> {
    match token {
        "true" => Ok(true),
        "false" => Ok(false),
        other => rule_err(format!("line {line_no}: boolean must be true/false, got '{other}'")),
    }
}

/// Render a base-unit engine value back into an explicit v2 token. Lengths are
/// emitted in metres (the engine base unit), angles in radians.
pub(crate) fn render_length(metres: f32) -> String {
    format!("{}{}", crate::json::format_f32(metres), MeasureUnit::Meter.suffix())
}

pub(crate) fn render_angle(radians: f32) -> String {
    format!("{}{}", crate::json::format_f32(radians), MeasureUnit::Radian.suffix())
}

pub(crate) fn render_ratio(value: f32) -> String {
    crate::json::format_f32(value)
}
