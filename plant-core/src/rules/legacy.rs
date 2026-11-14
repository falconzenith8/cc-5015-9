//! Legacy `plant-rules/v1` parser.
//!
//! The legacy contract is implicit on purpose:
//!
//! * a `unit <m|cm|mm>` header selects the default length unit; absent it,
//!   bare length numbers mean metres (`m`);
//! * bare angle numbers always mean radians (`rad`);
//! * branch references are positional shorthand (`trunk`, `parent`/`shoot`);
//! * material bindings name a material then a layer word.
//!
//! Every time the parser falls back to one of those conventions it records the
//! fact in [`LegacyParseResult::implicit_defaults`], so the migration step can
//! make each one explicit rather than silently inheriting an undocumented
//! default. Unknown directives are never interpreted: they are returned as
//! [`LegacyParseResult::unknown`] and become pending migration items.

use crate::error::PlantError;
use crate::params::PlantParams;

use super::ast::{AttachRule, AttachTarget, LayerSelector, LegacyMeasure, RawRuleLine, RuleDocument};
use super::{FieldDisplays, MaterialBinding, MeasureUnit, RulePlan, FORMAT_V1};

/// What the legacy parser produced, including an audit trail of everything it
/// had to infer. The audit trail is the input to an honest migration report.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LegacyParseResult {
    pub plan: RulePlan,
    /// Human-readable records of implicit conventions applied, e.g.
    /// `"trunk length: bare number -> default unit m"`.
    pub implicit_defaults: Vec<String>,
    /// Lines no legacy grammar rule recognizes; migration must list them as
    /// pending instead of guessing.
    pub unknown: Vec<UnknownDirective>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnknownDirective {
    pub line_no: usize,
    pub text: String,
    pub reason: String,
}

/// Parse a v1 document. Missing optional fields are filled from
/// [`PlantParams::default`] and each fill is logged; missing *required*
/// identity fields (`seed`) are hard errors.
pub fn parse_legacy(doc: &RuleDocument) -> Result<LegacyParseResult, PlantError> {
    if doc.format != FORMAT_V1 {
        return Err(PlantError::Rule(format!(
            "parse_legacy requires {FORMAT_V1}, got {}",
            doc.format
        )));
    }

    let mut params = PlantParams::default();
    let mut default_length_unit = MeasureUnit::Meter;
    let mut unit_header_seen = false;
    let mut materials: Vec<MaterialBinding> = Vec::new();
    let mut attach: Option<AttachRule> = None;
    let mut implicit_defaults: Vec<String> = Vec::new();
    let mut unknown: Vec<UnknownDirective> = Vec::new();
    let mut displays = FieldDisplays::default();
    let mut seed_seen = false;
    // Tracks fields the document actually stated, so a value that happens to
    // equal the engine default is never misreported as an implicit default.
    let mut seen_fields: Vec<&'static str> = Vec::new();

    for line in &doc.lines {
        let head = line.tokens[0].as_str();
        match head {
            "unit" => {
                require_args(line, 1)?;
                if unit_header_seen {
                    return Err(PlantError::Rule(format!(
                        "line {}: duplicate unit header",
                        line.line_no
                    )));
                }
                match line.tokens[1].as_str() {
                    "m" | "meter" | "meters" => default_length_unit = MeasureUnit::Meter,
                    "cm" => default_length_unit = MeasureUnit::CentiMeter,
                    "mm" => default_length_unit = MeasureUnit::MilliMeter,
                    other => {
                        unknown.push(UnknownDirective {
                            line_no: line.line_no,
                            text: line.tokens.join(" "),
                            reason: format!("unknown default length unit '{other}'"),
                        });
                        continue;
                    }
                }
                unit_header_seen = true;
                implicit_defaults.push(format!(
                    "default length unit declared as {}",
                    default_length_unit.suffix()
                ));
            }
            "seed" => {
                require_args(line, 1)?;
                params.seed = line.tokens[1]
                    .parse::<u64>()
                    .map_err(|_| PlantError::Rule(format!("line {}: bad seed", line.line_no)))?;
                seed_seen = true;
            }
            "trunk" => {
                require_args(line, 2)?;
                match line.tokens[1].as_str() {
                    "length" => {
                        params.trunk_length = parse_length(
                            &line.tokens[2],
                            "trunk.length",
                            default_length_unit,
                            &mut implicit_defaults,
                            &mut displays,
                        )?;
                        seen_fields.push("trunk_length");
                    }
                    "radius" => {
                        params.trunk_radius = parse_length(
                            &line.tokens[2],
                            "trunk.radius",
                            default_length_unit,
                            &mut implicit_defaults,
                            &mut displays,
                        )?;
                        seen_fields.push("trunk_radius");
                    }
                    other => {
                        return Err(PlantError::Rule(format!(
                            "line {}: unknown trunk field '{other}'",
                            line.line_no
                        )))
                    }
                }
            }
            "branch" => {
                require_args(line, 2)?;
                apply_branch(
                    line,
                    &mut params,
                    default_length_unit,
                    &mut implicit_defaults,
                    &mut displays,
                )?;
                seen_fields.push(branch_field_key(&line.tokens[1]));
            }
            "leaf" => {
                apply_leaf(
                    line,
                    &mut params,
                    default_length_unit,
                    &mut implicit_defaults,
                    &mut displays,
                )?;
                seen_fields.push("leaf");
            }
            "limit" => {
                require_args(line, 2)?;
                let value = parse_usize(&line.tokens[2], &line.tokens[1], line.line_no)?;
                match line.tokens[1].as_str() {
                    "depth" => params.limits.max_depth = value as u32,
                    "nodes" => params.limits.max_nodes = value,
                    "leaves" => params.limits.max_leaves = value,
                    "vertices" => params.limits.max_vertices = value,
                    "triangles" => params.limits.max_triangles = value,
                    other => {
                        return Err(PlantError::Rule(format!(
                            "line {}: unknown limit '{other}'",
                            line.line_no
                        )))
                    }
                }
                seen_fields.push("limits");
            }
            "material" => {
                require_args(line, 2)?;
                let material = line.tokens[1].clone();
                let layer = parse_legacy_layer(&line.tokens[2], line.line_no)?;
                materials.push(MaterialBinding { material, layer });
            }
            "attach" => {
                require_args(line, 2)?;
                let target = AttachTarget::legacy_name(&line.tokens[1])?;
                let t = line.tokens[2].parse::<f32>().map_err(|_| {
                    PlantError::Rule(format!(
                        "line {}: attach position must be a number",
                        line.line_no
                    ))
                })?;
                if !(0.0..=1.0).contains(&t) {
                    return Err(PlantError::Rule(format!(
                        "line {}: attach position must be in [0, 1]",
                        line.line_no
                    )));
                }
                attach = Some(AttachRule { target, t });
            }
            _ => unknown.push(UnknownDirective {
                line_no: line.line_no,
                text: line.tokens.join(" "),
                reason: format!("no v1 grammar rule for directive '{head}'"),
            }),
        }
    }

    if !seed_seen {
        return Err(PlantError::Rule(
            "v1 rule document is missing required 'seed <u64>'".into(),
        ));
    }

    if !unit_header_seen {
        implicit_defaults.push(
            "no 'unit' header: all bare length numbers interpreted as metres (m)".into(),
        );
    }
    record_default_gaps(&seen_fields, &mut implicit_defaults);

    Ok(LegacyParseResult {
        plan: RulePlan {
            params,
            materials,
            attach,
            displays,
        },
        implicit_defaults,
        unknown,
    })
}

fn require_args(line: &RawRuleLine, n: usize) -> Result<(), PlantError> {
    if line.tokens.len() != n + 1 {
        return Err(PlantError::Rule(format!(
            "line {}: '{}' expects {n} argument(s)",
            line.line_no, line.tokens[0]
        )));
    }
    Ok(())
}

fn parse_length(
    token: &str,
    field: &str,
    default_unit: MeasureUnit,
    notes: &mut Vec<String>,
    displays: &mut FieldDisplays,
) -> Result<f32, PlantError> {
    let measure = LegacyMeasure::parse(token)?;
    let resolved = measure.resolve_legacy(default_unit)?;
    if measure.explicit_unit.is_none() {
        notes.push(format!(
            "{field}: bare number '{token}' resolved with default length unit {}",
            default_unit.suffix()
        ));
        // Make the implicit unit explicit while preserving the original number.
        displays.set(field, format!("{}{}", measure.number, default_unit.suffix()));
    } else {
        // Keep the source spelling (value and unit) losslessly.
        displays.set(field, token.to_string());
    }
    Ok(resolved.base_value())
}

fn parse_angle(
    token: &str,
    field: &str,
    notes: &mut Vec<String>,
    displays: &mut FieldDisplays,
) -> Result<f32, PlantError> {
    let measure = LegacyMeasure::parse(token)?;
    let resolved = measure.resolve_legacy(MeasureUnit::Radian)?;
    if measure.explicit_unit.is_none() {
        notes.push(format!(
            "{field}: bare number '{token}' resolved with default angle unit rad"
        ));
        displays.set(field, format!("{}{}", measure.number, MeasureUnit::Radian.suffix()));
    } else {
        displays.set(field, token.to_string());
    }
    Ok(resolved.base_value())
}

fn parse_ratio(token: &str, field: &str) -> Result<f32, PlantError> {
    let measure = LegacyMeasure::parse(token)?;
    if measure.explicit_unit.is_some() {
        return Err(PlantError::Rule(format!(
            "{field} is a dimensionless ratio; unit suffixes are not allowed in v1"
        )));
    }
    Ok(measure.number)
}

fn apply_branch(
    line: &RawRuleLine,
    params: &mut PlantParams,
    default_unit: MeasureUnit,
    notes: &mut Vec<String>,
    displays: &mut FieldDisplays,
) -> Result<(), PlantError> {
    let token = &line.tokens[2];
    match line.tokens[1].as_str() {
        "count" => params.branch_count = parse_u32(token, "branch count", line.line_no)?,
        "angle" => {
            params.branch_angle = parse_angle(token, "branch.angle", notes, displays)?
        }
        "jitter" => params.angle_jitter = parse_angle(token, "branch.jitter", notes, displays)?,
        "length-ratio" => params.length_ratio = parse_ratio(token, "length ratio")?,
        "length-jitter" => params.length_jitter = parse_ratio(token, "length jitter")?,
        "radius-ratio" => params.radius_ratio = parse_ratio(token, "radius ratio")?,
        "taper" => params.taper = parse_ratio(token, "taper")?,
        "bend" => params.bend = parse_ratio(token, "bend")?,
        "segments" => params.segments = parse_u32(token, "segments", line.line_no)?,
        "sides" => params.radial_sides = parse_u32(token, "radial sides", line.line_no)?,
        other => {
            return Err(PlantError::Rule(format!(
                "line {}: unknown branch field '{other}'",
                line.line_no
            )))
        }
    }
    let _ = default_unit;
    Ok(())
}

fn apply_leaf(
    line: &RawRuleLine,
    params: &mut PlantParams,
    default_unit: MeasureUnit,
    notes: &mut Vec<String>,
    displays: &mut FieldDisplays,
) -> Result<(), PlantError> {
    let lp = &mut params.leaves;
    match line.tokens.get(1).map(String::as_str) {
        Some("off") => {
            require_args(line, 1)?;
            lp.enabled = false;
        }
        Some("on") => {
            require_args(line, 1)?;
            lp.enabled = true;
        }
        Some("per-tip") => {
            require_args(line, 2)?;
            lp.per_tip = parse_u32(&line.tokens[2], "leaf per-tip", line.line_no)?;
        }
        Some("length") => {
            require_args(line, 2)?;
            lp.length =
                parse_length(&line.tokens[2], "leaf.length", default_unit, notes, displays)?;
        }
        Some("width") => {
            require_args(line, 2)?;
            lp.width =
                parse_length(&line.tokens[2], "leaf.width", default_unit, notes, displays)?;
        }
        Some("lift") => {
            require_args(line, 2)?;
            lp.lift = parse_ratio(&line.tokens[2], "leaf lift")?;
        }
        other => {
            return Err(PlantError::Rule(format!(
                "line {}: unknown leaf field '{}'",
                line.line_no,
                other.unwrap_or("?")
            )))
        }
    }
    Ok(())
}

fn parse_legacy_layer(token: &str, line_no: usize) -> Result<LayerSelector, PlantError> {
    match token {
        "trunk" => Ok(LayerSelector::Trunk),
        "twigs" | "tips" => Ok(LayerSelector::From(1)),
        "leaves" => Ok(LayerSelector::Leaves),
        other => {
            if let Some(digits) = other.strip_prefix('L') {
                let depth = digits
                    .parse::<u32>()
                    .map_err(|_| PlantError::Rule(format!("line {line_no}: bad layer '{other}'")))?;
                Ok(LayerSelector::Exact(depth))
            } else {
                Err(PlantError::Rule(format!(
                    "line {line_no}: unknown legacy layer word '{other}' (expected trunk, twigs, leaves or L<n>)"
                )))
            }
        }
    }
}

/// Map a legacy `branch <field>` token to the engine field whose presence it
/// satisfies. Unknown keys never reach here (the apply function rejects them).
fn branch_field_key(token: &str) -> &'static str {
    match token {
        "count" => "branch_count",
        "angle" => "branch_angle",
        "jitter" => "angle_jitter",
        "length-ratio" => "length_ratio",
        "length-jitter" => "length_jitter",
        "radius-ratio" => "radius_ratio",
        "taper" => "taper",
        "bend" => "bend",
        "segments" => "segments",
        "sides" => "radial_sides",
        _ => "branch_unknown",
    }
}

/// Record only the fields the document never stated. Presence is tracked
/// explicitly so a stated value equal to the engine default is not mistaken
/// for an inherited default — the converter must not flag explicit input.
fn record_default_gaps(seen: &[&str], out: &mut Vec<String>) {
    macro_rules! gap {
        ($key:literal, $name:literal) => {
            if !seen.iter().any(|k| *k == $key) {
                out.push(format!(
                    concat!($name, " not stated: implicit engine default retained")
                ));
            }
        };
    }
    gap!("trunk_length", "trunk length");
    gap!("trunk_radius", "trunk radius");
    gap!("branch_count", "branch count");
    gap!("branch_angle", "branch angle");
    gap!("length_ratio", "length ratio");
    gap!("radius_ratio", "radius ratio");
    gap!("taper", "taper");
    gap!("angle_jitter", "angle jitter");
    gap!("length_jitter", "length jitter");
    gap!("bend", "bend");
    gap!("segments", "segments");
    gap!("radial_sides", "radial sides");
    if !seen.iter().any(|k| *k == "leaf") {
        out.push("leaf block not stated: implicit leaf defaults retained".into());
    }
    if !seen.iter().any(|k| *k == "limits") {
        out.push("resource limits not stated: implicit engine budgets retained".into());
    }
}

fn parse_u32(token: &str, name: &str, line_no: usize) -> Result<u32, PlantError> {
    token
        .parse::<u32>()
        .map_err(|_| PlantError::Rule(format!("line {line_no}: {name} must be an integer")))
}

fn parse_usize(token: &str, name: &str, line_no: usize) -> Result<usize, PlantError> {
    token
        .parse::<usize>()
        .map_err(|_| PlantError::Rule(format!("line {line_no}: limit {name} must be an integer")))
}
