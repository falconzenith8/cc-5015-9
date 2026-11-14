//! Syntax-level types shared by the legacy and explicit rule parsers.

use crate::error::PlantError;

use super::{rule_err, tokenize_line, FORMAT_V1, FORMAT_V2};

/// Physical dimension of a measured value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasureUnit {
    /// Metres, centimetres or millimetres.
    Meter,
    CentiMeter,
    MilliMeter,
    /// Radians or degrees.
    Radian,
    Degree,
}

impl MeasureUnit {
    pub fn parse_suffix(suffix: &str) -> Option<MeasureUnit> {
        Some(match suffix {
            "m" => MeasureUnit::Meter,
            "cm" => MeasureUnit::CentiMeter,
            "mm" => MeasureUnit::MilliMeter,
            "rad" => MeasureUnit::Radian,
            "deg" => MeasureUnit::Degree,
            _ => return None,
        })
    }

    /// Factor converting a value with this unit into the base unit the engine
    /// stores internally (metres for lengths, radians for angles).
    pub fn to_base_factor(self) -> f32 {
        match self {
            MeasureUnit::Meter => 1.0,
            MeasureUnit::CentiMeter => 0.01,
            MeasureUnit::MilliMeter => 0.001,
            MeasureUnit::Radian => 1.0,
            MeasureUnit::Degree => std::f32::consts::PI / 180.0,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            MeasureUnit::Meter => "m",
            MeasureUnit::CentiMeter => "cm",
            MeasureUnit::MilliMeter => "mm",
            MeasureUnit::Radian => "rad",
            MeasureUnit::Degree => "deg",
        }
    }

    pub fn is_length(self) -> bool {
        matches!(
            self,
            MeasureUnit::Meter | MeasureUnit::CentiMeter | MeasureUnit::MilliMeter
        )
    }
}

/// A value plus its written unit, as it appears after parsing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    pub value: f32,
    pub unit: MeasureUnit,
}

impl Measure {
    /// Value in the engine's base unit (m or rad).
    pub fn base_value(&self) -> f32 {
        self.value * self.unit.to_base_factor()
    }
}

/// A measure token before resolution. Legacy documents may omit the suffix;
/// the explicit v2 parser rejects that. `raw` keeps the original spelling so
/// the migration report can show exactly what was made explicit.
#[derive(Clone, Debug, PartialEq)]
pub struct LegacyMeasure {
    pub raw: String,
    pub number: f32,
    pub explicit_unit: Option<MeasureUnit>,
}

impl LegacyMeasure {
    pub fn parse(token: &str) -> Result<Self, PlantError> {
        let split = token
            .find(|c: char| c.is_alphabetic())
            .unwrap_or(token.len());
        let (num_part, suffix) = token.split_at(split);
        let number = num_part
            .parse::<f32>()
            .map_err(|_| PlantError::Rule(format!("'{token}' is not a number with optional unit")))?;
        if !number.is_finite() {
            return rule_err(format!("measure '{token}' is not finite"));
        }
        let explicit_unit = if suffix.is_empty() {
            None
        } else {
            Some(
                MeasureUnit::parse_suffix(suffix)
                    .ok_or_else(|| PlantError::Rule(format!("unknown unit suffix '{suffix}'")))?,
            )
        };
        Ok(LegacyMeasure {
            raw: token.to_string(),
            number,
            explicit_unit,
        })
    }

    /// Legacy resolution: an implicit bare number is interpreted in the
    /// document's documented default unit. Callers name the default explicitly
    /// so no hidden fallback can mask a missing unit.
    pub fn resolve_legacy(&self, default_unit: MeasureUnit) -> Result<Measure, PlantError> {
        let unit = match self.explicit_unit {
            Some(u) => {
                if u.is_length() != default_unit.is_length() {
                    return rule_err(format!(
                        "unit '{}' has the wrong dimension for this field",
                        u.suffix()
                    ));
                }
                u
            }
            None => default_unit,
        };
        Ok(Measure {
            value: self.number,
            unit,
        })
    }

    /// Explicit v2 resolution: the suffix is mandatory.
    pub fn resolve_explicit(&self) -> Result<Measure, PlantError> {
        match self.explicit_unit {
            Some(unit) => Ok(Measure {
                value: self.number,
                unit,
            }),
            None => rule_err(format!(
                "'{}' is missing an explicit unit suffix (m/cm/mm or rad/deg)",
                self.raw
            )),
        }
    }
}

/// Branch layer selector used by material bindings and references.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerSelector {
    /// The trunk only (depth 0).
    Trunk,
    /// One exact depth.
    Exact(u32),
    /// Every depth from `start` onward.
    From(u32),
    /// Inclusive depth range.
    Range(u32, u32),
    /// Leaf instances rather than woody nodes.
    Leaves,
}

impl LayerSelector {
    pub fn matches(&self, depth: u32) -> bool {
        match *self {
            LayerSelector::Trunk => depth == 0,
            LayerSelector::Exact(d) => depth == d,
            LayerSelector::From(start) => depth >= start,
            LayerSelector::Range(a, b) => depth >= a && depth <= b,
            LayerSelector::Leaves => false,
        }
    }

    pub fn render(&self) -> String {
        match self {
            LayerSelector::Trunk => "branch:trunk".into(),
            LayerSelector::Exact(d) => format!("branch:{d}"),
            LayerSelector::From(d) => format!("branch:{d}.."),
            LayerSelector::Range(a, b) => format!("branch:{a}..{b}"),
            LayerSelector::Leaves => "leaves".into(),
        }
    }

    /// Parse the qualified v2 form (`branch:trunk`, `branch:2`, `branch:1..`,
    /// `branch:1..3`, `leaves`).
    pub fn parse_qualified(text: &str) -> Result<Self, PlantError> {
        if text == "leaves" {
            return Ok(LayerSelector::Leaves);
        }
        let body = text
            .strip_prefix("branch:")
            .ok_or_else(|| PlantError::Rule(format!("layer '{text}' must be branch:<selector> or leaves")))?;
        if body == "trunk" {
            return Ok(LayerSelector::Trunk);
        }
        if let Some((a, b)) = body.split_once("..") {
            let start = parse_depth(a, text)?;
            if b.is_empty() {
                return Ok(LayerSelector::From(start));
            }
            let end = parse_depth(b, text)?;
            if end < start {
                return rule_err(format!("layer '{text}' has an inverted range"));
            }
            return Ok(LayerSelector::Range(start, end));
        }
        Ok(LayerSelector::Exact(parse_depth(body, text)?))
    }
}

fn parse_depth(s: &str, whole: &str) -> Result<u32, PlantError> {
    s.parse::<u32>()
        .map_err(|_| PlantError::Rule(format!("layer '{whole}' contains a non-integer depth")))
}

/// Target of an attachment rule. Legacy positional names resolve through
/// [`AttachTarget::legacy_name`]; v2 only writes the qualified spelling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachTarget {
    Trunk,
    /// `parent` — the immediate parent branch.
    Parent,
}

impl AttachTarget {
    pub fn legacy_name(name: &str) -> Result<Self, PlantError> {
        match name {
            "trunk" => Ok(AttachTarget::Trunk),
            "parent" | "shoot" => Ok(AttachTarget::Parent),
            other => rule_err(format!("unknown branch reference '{other}'")),
        }
    }

    pub fn parse_qualified(text: &str) -> Result<Self, PlantError> {
        match text {
            "branch:trunk" => Ok(AttachTarget::Trunk),
            "branch:parent" => Ok(AttachTarget::Parent),
            other => rule_err(format!(
                "attach target '{other}' must be branch:trunk or branch:parent"
            )),
        }
    }

    pub fn render(&self) -> &'static str {
        match self {
            AttachTarget::Trunk => "branch:trunk",
            AttachTarget::Parent => "branch:parent",
        }
    }
}

/// Attachment reference plus its normalized position along the target.
#[derive(Clone, Debug, PartialEq)]
pub struct AttachRule {
    pub target: AttachTarget,
    pub t: f32,
}

/// One raw, uninterpreted line. Unknown directives are kept verbatim by the
/// legacy parser and surfaced as pending migration items instead of being
/// dropped or defaulted.
#[derive(Clone, Debug, PartialEq)]
pub struct RawRuleLine {
    pub line_no: usize,
    pub tokens: Vec<String>,
}

/// A parsed rule document at the syntax level. The body lines stay raw until
/// the version-specific parser interprets them.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleDocument {
    pub format: String,
    pub lines: Vec<RawRuleLine>,
}

impl RuleDocument {
    /// Tokenize a rule file and verify its format marker. Both syntaxes begin
    /// with `plant-rules/v1` or `plant-rules/v2` on the first non-comment line.
    pub fn tokenize(text: &str) -> Result<Self, PlantError> {
        let mut lines = Vec::new();
        for (idx, raw) in text.lines().enumerate() {
            if let Some(tokens) = tokenize_line(raw) {
                lines.push(RawRuleLine {
                    line_no: idx + 1,
                    tokens,
                });
            }
        }
        let first = lines
            .first()
            .ok_or_else(|| PlantError::Rule("empty rule document".into()))?;
        if first.tokens.len() != 1
            || (first.tokens[0] != FORMAT_V1 && first.tokens[0] != FORMAT_V2)
        {
            return rule_err(format!(
                "first line must be exactly '{FORMAT_V1}' or '{FORMAT_V2}', found '{}'",
                first.tokens.join(" ")
            ));
        }
        Ok(RuleDocument {
            format: first.tokens[0].clone(),
            lines: lines.into_iter().skip(1).collect(),
        })
    }
}

/// Convenience for the common `directive key=value ...` v2 spelling: split a
/// token into its key and value around the first `=`.
pub fn split_key_value(token: &str) -> Result<(&str, &str), PlantError> {
    token
        .split_once('=')
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .ok_or_else(|| PlantError::Rule(format!("expected key=value, found '{token}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_measure_resolves_bare_number_in_default_unit() {
        let m = LegacyMeasure::parse("320").unwrap();
        assert!(m.explicit_unit.is_none());
        let resolved = m.resolve_legacy(MeasureUnit::CentiMeter).unwrap();
        assert_eq!(resolved.unit, MeasureUnit::CentiMeter);
        assert!((resolved.base_value() - 3.2).abs() < 1e-6);
    }

    #[test]
    fn explicit_measure_keeps_suffix_and_converts_base() {
        let m = LegacyMeasure::parse("45deg").unwrap();
        let resolved = m.resolve_explicit().unwrap();
        assert_eq!(resolved.unit, MeasureUnit::Degree);
        assert!((resolved.base_value() - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
    }

    #[test]
    fn explicit_resolution_rejects_bare_number() {
        let m = LegacyMeasure::parse("0.7").unwrap();
        assert!(m.resolve_explicit().is_err());
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let m = LegacyMeasure::parse("0.7rad").unwrap();
        // A radian cannot resolve against a length default.
        assert!(m.resolve_legacy(MeasureUnit::Meter).is_err());
    }

    #[test]
    fn unknown_suffix_is_rejected() {
        assert!(LegacyMeasure::parse("3furlong").is_err());
    }
}
