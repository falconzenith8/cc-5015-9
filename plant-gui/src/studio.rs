//! Headless engine behind the plant GUI.
//!
//! All state changes and rebuilds live here so the rule-driven workflow can be
//! exercised without a window system. The egui layer in `main.rs` only renders
//! state and forwards intents; the deterministic plant core drives every
//! rebuild and every failure is surfaced back as a status message.

use plant_core::rules::{
    migrate, rule_geometry_evidence, RuleDocument, RuleGeometryEvidence, RulePlan, FORMAT_V1,
    FORMAT_V2,
};
use plant_core::{build_leaves, build_meshes, build_plant, validate_plant, Plant, PlantParams};

/// One parsed rule document plus the text it came from.
#[derive(Clone, Debug)]
pub struct LoadedRule {
    pub format: String,
    pub text: String,
    pub plan: RulePlan,
}

impl LoadedRule {
    /// Number of material bindings carried by the applied document.
    pub fn binding_count(&self) -> usize {
        self.plan.materials.len()
    }

    /// Whether the applied document used the explicit v2 syntax.
    pub fn is_explicit_v2(&self) -> bool {
        self.format == FORMAT_V2
    }

    /// Original rule text of the applied document.
    pub fn source_text(&self) -> &str {
        &self.text
    }
}

/// The complete application state, independent of egui.
#[derive(Default)]
pub struct PlantStudio {
    pub params: PlantParams,
    pub plant: Option<Plant>,
    pub status: String,
    pub auto_rebuild: bool,
    /// Rule text currently shown in the editor.
    pub rule_text: String,
    /// The last successfully applied rule document.
    pub loaded: Option<LoadedRule>,
    /// Evidence from the last rule-driven rebuild.
    pub evidence: Option<RuleGeometryEvidence>,
    pub pending_count: usize,
}

impl PlantStudio {
    pub fn new() -> Self {
        let mut studio = Self {
            params: PlantParams::default(),
            status: "not built yet".to_string(),
            auto_rebuild: true,
            ..Default::default()
        };
        studio.rebuild_params();
        studio
    }

    /// Rebuild from the current [`PlantParams`]. This is the single entry the
    /// control panel uses; validation failures are returned into `status`.
    pub fn rebuild_params(&mut self) {
        let params = self.params.clone();
        match build_plant(params) {
            Ok((mut plant, stats)) => {
                let (meshes, tube_stats) = match build_meshes(&plant) {
                    Ok(v) => v,
                    Err(e) => {
                        self.status = format!("mesh error: {e}");
                        return;
                    }
                };
                let (leaves, leaf_stats) = build_leaves(&plant);
                plant.leaves = leaves;
                match validate_plant(&plant) {
                    Ok(()) => {
                        self.status = format!(
                            "nodes={} depth={} leaves={} v={} t={} prune(d={},b={},n={})",
                            plant.node_count(),
                            stats.depth_reached,
                            plant.leaves.len(),
                            tube_stats.vertices + leaf_stats.vertices,
                            tube_stats.triangles + leaf_stats.triangles,
                            stats.pruned_degenerate,
                            stats.pruned_depth,
                            stats.pruned_node_budget
                        );
                        self.plant = Some(plant);
                        self.evidence = None;
                        drop(meshes);
                    }
                    Err(e) => self.status = format!("validation failed: {e}"),
                }
            }
            Err(e) => self.status = format!("build error: {e}"),
        }
    }

    /// Parse and apply rule text (v1 or v2), then rebuild through the same
    /// core path. Any parse/validation failure is reported back into `status`
    /// and leaves the previous plant intact.
    pub fn apply_rule_text(&mut self, text: &str) -> Result<(), String> {
        let doc = RuleDocument::tokenize(text).map_err(|e| e.to_string())?;
        let plan = match doc.format.as_str() {
            FORMAT_V1 => plant_core::rules::parse_legacy(&doc)
                .map_err(|e| e.to_string())?
                .plan,
            FORMAT_V2 => plant_core::rules::parse_v2(&doc).map_err(|e| e.to_string())?,
            other => return Err(format!("unknown rule format '{other}'")),
        };
        // Validate parameters before committing so a bad rule cannot leave the
        // studio in an inconsistent state.
        let validated = plan.params.clone().validated().map_err(|e| e.to_string())?;
        self.params = validated;
        self.rule_text = text.to_string();
        self.loaded = Some(LoadedRule {
            format: doc.format,
            text: text.to_string(),
            plan: plan.clone(),
        });
        self.rebuild_with_plan(plan);
        Ok(())
    }

    /// Run the v1 -> v2 migration on the editor text. On success the editor
    /// holds the explicit v2 text and a rebuild uses it immediately. Pending
    /// rules are counted and surfaced, never silently dropped.
    pub fn migrate_editor_text(&mut self) -> Result<usize, String> {
        let report = migrate(&self.rule_text).map_err(|e| e.to_string())?;
        let pending = report.pending.len();
        self.rule_text = report.migrated_text.clone();
        // Apply the migrated document through the normal rule path so the
        // virtual plant runs the new implementation immediately.
        self.apply_rule_text(&report.migrated_text)?;
        self.pending_count = pending;
        if pending > 0 {
            self.status = format!(
                "{} (migration: {} pending rule(s) preserved as comments)",
                self.status, pending
            );
        } else {
            self.status = format!("{} (migrated v1 -> v2, no pending rules)", self.status);
        }
        Ok(pending)
    }

    fn rebuild_with_plan(&mut self, plan: RulePlan) {
        // rebuild_params already consumed self.params (set from the plan); the
        // plan additionally carries material bindings for rule evidence.
        self.rebuild_params();
        if let Some(plant) = &self.plant {
            self.evidence = Some(rule_geometry_evidence(&plan, plant));
        }
    }

    pub fn reset_defaults(&mut self) {
        self.params = PlantParams::default();
        self.loaded = None;
        self.evidence = None;
        self.rule_text.clear();
        self.pending_count = 0;
        self.rebuild_params();
    }
}
