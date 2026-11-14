//! Realtime rule/parameter editor for the procedural plant generator.
//!
//! The egui layer is intentionally thin: every state change and every rebuild
//! goes through [`studio::PlantStudio`], which drives the deterministic core.
//! The left panel edits either the classic [`PlantParams`] sliders or a rule
//! document (legacy v1 / explicit v2, including one-click migration); the right
//! panel renders the actual generated splines and leaf instances with an orbit
//! camera. It never displays a pre-made model.
//!
//! # Launching the interface
//!
//! ```bash
//! cargo run --release -p plant-gui
//! ```
//!
//! A native display (Wayland/X11) is required. On a headless host the binary
//! exits with a clear window-system error; the headless rule workflow is
//! covered by the `studio` module tests and the `plant` CLI.

mod studio;

use eframe::egui::{self, Color32, Pos2, Slider, Ui, Vec2};
use plant_core::{LeafParams, Limits, PlantParams};
use studio::PlantStudio;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Procedural Plant Studio",
        options,
        Box::new(|_cc| Ok(Box::new(PlantApp::default()))),
    )
}

struct PlantApp {
    studio: PlantStudio,
    rebuild_requested: bool,
    cam_yaw: f32,
    cam_pitch: f32,
    cam_dist: f32,
    rule_status: String,
}

impl Default for PlantApp {
    fn default() -> Self {
        Self {
            studio: PlantStudio::new(),
            rebuild_requested: false,
            cam_yaw: 0.6,
            cam_pitch: 0.35,
            cam_dist: 14.0,
            rule_status: String::new(),
        }
    }
}

impl PlantApp {
    fn request_rebuild(&mut self) {
        if self.studio.auto_rebuild {
            self.studio.rebuild_params();
        } else {
            self.rebuild_requested = true;
        }
    }
}

impl eframe::App for PlantApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::SidePanel::left("controls").default_width(380.0).show(ctx, |ui| {
            ui.heading("Procedural Plant");
            ui.separator();
            let before = self.studio.params.clone();
            parameter_panel(ui, &mut self.studio.params);
            ui.separator();
            rule_panel(ui, self);
            ui.separator();
            ui.checkbox(&mut self.studio.auto_rebuild, "Rebuild on change");
            if !self.studio.auto_rebuild && ui.button("Rebuild now").clicked() {
                self.rebuild_requested = true;
            }
            if ui.button("Reset to defaults").clicked() {
                self.studio.reset_defaults();
                self.rebuild_requested = true;
            }
            if self.studio.params != before {
                // Editing a slider detaches any applied rule document: the
                // params are now the source of truth again.
                self.studio.loaded = None;
                self.studio.evidence = None;
                self.request_rebuild();
            }
            if self.rebuild_requested {
                self.studio.rebuild_params();
                self.rebuild_requested = false;
            }
            ui.separator();
            ui.label(egui::RichText::new(&self.studio.status).small().color(Color32::LIGHT_GREEN));
            if !self.rule_status.is_empty() {
                ui.label(egui::RichText::new(&self.rule_status).small().color(Color32::LIGHT_BLUE));
            }
            ui.label(
                egui::RichText::new(format!("seed = {:#018x}", self.studio.params.seed)).small(),
            );
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            handle_camera(ui, self);
            let painter = ui.painter();
            let rect = ui.available_rect_before_wrap();
            painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 22, 20));
            if let Some(plant) = &self.studio.plant {
                draw_plant(painter, rect, plant, self);
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }
}

/// Rule document editor: load a file, apply v1/v2 text, or migrate v1 -> v2.
/// Every action delegates to the studio engine; failures come back as text.
fn rule_panel(ui: &mut Ui, app: &mut PlantApp) {
    ui.collapsing("Rule document (v1 legacy / v2 explicit)", |ui| {
        let available = ui.available_width();
        ui.add(
            egui::TextEdit::multiline(&mut app.studio.rule_text)
                .desired_width(available)
                .desired_rows(8)
                .code_editor(),
        );
        ui.horizontal(|ui| {
            if ui.button("Apply rule & rebuild").clicked() {
                match app.studio.apply_rule_text(&app.studio.rule_text.clone()) {
                    Ok(()) => {
                        let loaded = app.studio.loaded.as_ref().expect("rule just applied");
                        app.rule_status = format!(
                            "applied [{}] with {} material binding(s); rebuilt through the core path",
                            loaded.format,
                            loaded.binding_count()
                        );
                    }
                    Err(e) => app.rule_status = format!("rule error: {e}"),
                }
            }
            if ui.button("Migrate v1 -> v2").clicked() {
                match app.studio.migrate_editor_text() {
                    Ok(pending) => {
                        let explicit = app
                            .studio
                            .loaded
                            .as_ref()
                            .is_some_and(|l| l.is_explicit_v2());
                        app.rule_status = if pending == 0 {
                            format!(
                                "migrated: explicit v2 applied ({explicit}), no pending rules"
                            )
                        } else {
                            format!("migrated with {pending} pending rule(s); see comments")
                        };
                    }
                    Err(e) => app.rule_status = format!("migration error: {e}"),
                }
            }
        });
        if ui.button("Load rule file…").clicked() {
            // egui has no native file dialog without extra dependencies; the
            // path is entered in the field below and loaded on demand.
            app.rule_status = "type a path into the box and press Load".into();
        }
        ui.horizontal(|ui| {
            let mut path = String::new();
            ui.text_edit_singleline(&mut path);
            if ui.button("Load").clicked() && !path.is_empty() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        app.studio.rule_text = text;
                        match app.studio.apply_rule_text(&app.studio.rule_text.clone()) {
                            Ok(()) => app.rule_status = format!("loaded {path} and rebuilt"),
                            Err(e) => app.rule_status = format!("rule error in {path}: {e}"),
                        }
                    }
                    Err(e) => app.rule_status = format!("load error: {e}"),
                }
                if let Some(loaded) = app.studio.loaded.as_ref() {
                    app.rule_status = format!(
                        "{} ({} source bytes)",
                        app.rule_status,
                        loaded.source_text().len()
                    );
                }
            }
        });

        // Surface the rule-level geometry evidence the engine computed.
        if let Some(evidence) = &app.studio.evidence {
            ui.separator();
            ui.label(egui::RichText::new("Material / layer evidence").strong());
            for b in &evidence.bindings {
                let color = if b.matched {
                    Color32::LIGHT_GREEN
                } else {
                    Color32::from_rgb(230, 150, 60)
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{} on {}: {} matched node(s)",
                        b.material,
                        b.layer.render(),
                        b.matched_nodes.len()
                    ))
                    .small()
                    .color(color),
                );
            }
            if !evidence.uncovered_woody_nodes.is_empty() {
                ui.label(
                    egui::RichText::new(format!(
                        "uncovered woody nodes: {}",
                        evidence.uncovered_woody_nodes.len()
                    ))
                    .small()
                    .color(Color32::from_rgb(230, 150, 60)),
                );
            }
        }
    });
}

fn parameter_panel(ui: &mut Ui, p: &mut PlantParams) {
    ui.collapsing("Structure", |ui| {
        ui.add(Slider::new(&mut p.seed, 0..=u64::MAX).text("seed"));
        ui.add(Slider::new(&mut p.branch_count, 0..=12).text("branch count"));
        ui.add(Slider::new(&mut p.branch_angle, 0.0..=std::f32::consts::PI).text("branch angle"));
        ui.add(Slider::new(&mut p.angle_jitter, 0.0..=1.0).text("angle jitter"));
        ui.add(Slider::new(&mut p.length_ratio, 0.1..=1.2).text("length ratio"));
        ui.add(Slider::new(&mut p.length_jitter, 0.0..=0.6).text("length jitter"));
        ui.add(Slider::new(&mut p.radius_ratio, 0.2..=0.95).text("radius ratio"));
        ui.add(Slider::new(&mut p.taper, 0.0..=1.0).text("tip taper"));
        ui.add(Slider::new(&mut p.bend, 0.0..=0.6).text("branch bend"));
        ui.add(Slider::new(&mut p.trunk_length, 1.0..=12.0).text("trunk length"));
        ui.add(Slider::new(&mut p.trunk_radius, 0.05..=1.0).text("trunk radius"));
        ui.add(Slider::new(&mut p.segments, 1..=32).text("spline segments"));
        ui.add(Slider::new(&mut p.radial_sides, 3..=12).text("tube sides"));
    });
    ui.collapsing("Leaves", |ui| {
        ui.checkbox(&mut p.leaves.enabled, "leaves enabled");
        let lp: &mut LeafParams = &mut p.leaves;
        ui.add(Slider::new(&mut lp.per_tip, 0..=16).text("leaves per tip"));
        ui.add(Slider::new(&mut lp.length, 0.02..=1.0).text("leaf length"));
        ui.add(Slider::new(&mut lp.width, 0.01..=0.6).text("leaf width"));
        ui.add(Slider::new(&mut lp.lift, 0.0..=1.0).text("leaf lift"));
    });
    ui.collapsing("Resource budgets", |ui| {
        let l: &mut Limits = &mut p.limits;
        ui.add(Slider::new(&mut l.max_depth, 1..=16).text("max depth"));
        ui.add(Slider::new(&mut l.max_nodes, 1..=20_000).text("max nodes"));
        ui.add(Slider::new(&mut l.max_leaves, 0..=100_000).text("max leaves"));
        ui.add(Slider::new(&mut l.max_vertices, 100..=500_000).text("max vertices"));
        ui.add(Slider::new(&mut l.max_triangles, 100..=300_000).text("max triangles"));
    });
}

fn handle_camera(ui: &Ui, app: &mut PlantApp) {
    let resp = ui.interact(
        ui.available_rect_before_wrap(),
        ui.make_persistent_id("viewport"),
        egui::Sense::drag(),
    );
    if resp.dragged_by(egui::PointerButton::Primary) {
        app.cam_yaw += resp.drag_delta().x * 0.01;
        app.cam_pitch = (app.cam_pitch + resp.drag_delta().y * 0.01).clamp(-1.4, 1.4);
    }
    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
    if scroll != 0.0 {
        app.cam_dist = (app.cam_dist * (1.0 - scroll * 0.001)).clamp(3.0, 60.0);
    }
}

/// Project a world point into screen space with a simple orbit camera.
fn project(center: Pos2, scale: f32, yaw: f32, pitch: f32, dist: f32, p: plant_core::math::Vec3) -> Pos2 {
    let cy = yaw.cos();
    let sy = yaw.sin();
    let cp = pitch.cos();
    let sp = pitch.sin();
    // Rotate around Y then X, then perspective-divide by camera distance.
    let x1 = p.x * cy - p.z * sy;
    let z1 = p.x * sy + p.z * cy;
    let y2 = p.y * cp - z1 * sp;
    let z2 = p.y * sp + z1 * cp;
    let perspective = dist / (dist + z2 + 0.001);
    Pos2::new(
        center.x + x1 * scale * perspective,
        center.y - y2 * scale * perspective,
    )
}

fn draw_plant(painter: &egui::Painter, rect: egui::Rect, plant: &plant_core::Plant, app: &PlantApp) {
    let center = rect.center() + Vec2::new(0.0, rect.height() * 0.32);
    let scale = (rect.height() / (app.cam_dist * 1.2)).max(8.0);

    // Depth-sorted line segments: collect then paint back-to-front.
    let mut segments: Vec<(f32, Pos2, Pos2, Color32)> = Vec::new();
    for node in &plant.nodes {
        let depth_shade = 200 - (node.depth * 18).min(140) as u8;
        let color = Color32::from_rgb(95, depth_shade, 70);
        for w in node.spline.windows(2) {
            let a = node.world.mul_point(w[0]);
            let b = node.world.mul_point(w[1]);
            let depth = (a.z + b.z) * 0.5;
            segments.push((
                depth,
                project(center, scale, app.cam_yaw, app.cam_pitch, app.cam_dist, a),
                project(center, scale, app.cam_yaw, app.cam_pitch, app.cam_dist, b),
                color,
            ));
        }
    }
    segments.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, a, b, color) in segments {
        painter.line_segment([a, b], (2.0, color));
    }

    // Leaves drawn as small quads from the instance transforms — the same
    // instances the OBJ export bakes, not billboarded particles.
    let leaf_color = Color32::from_rgb(120, 220, 110);
    for leaf in &plant.leaves {
        let base = leaf.anchor;
        let tip = leaf.world.mul_point(plant_core::math::Vec3::new(0.0, plant.params.leaves.length, 0.0));
        let side = leaf.world.mul_point(plant_core::math::Vec3::new(
            plant.params.leaves.width * 0.5,
            plant.params.leaves.length * 0.45,
            0.0,
        ));
        let p0 = project(center, scale, app.cam_yaw, app.cam_pitch, app.cam_dist, base);
        let p1 = project(center, scale, app.cam_yaw, app.cam_pitch, app.cam_dist, tip);
        let p2 = project(center, scale, app.cam_yaw, app.cam_pitch, app.cam_dist, side);
        painter.line_segment([p0, p1], (1.0, leaf_color));
        painter.line_segment([p0, p2], (1.0, leaf_color));
    }
}

#[cfg(test)]
mod tests {
    use super::studio::PlantStudio;

    const LEGACY_RULE: &str = "plant-rules/v1\n\
seed 555\n\
trunk length 4\n\
branch count 3\n\
branch angle 0.7\n\
limit depth 4\n\
material bark trunk\n\
material wood twigs\n\
material foliage leaves\n";

    /// The viewport-free engine must drive a full rule -> rebuild cycle,
    /// including migration, so the virtual plant is not "done" as scripts only.
    #[test]
    fn studio_applies_legacy_rule_and_rebuilds_plant() {
        let mut studio = PlantStudio::new();
        studio.rule_text = LEGACY_RULE.to_string();
        studio.apply_rule_text(LEGACY_RULE).expect("legacy rule applies");
        let plant = studio.plant.as_ref().expect("plant rebuilt from rule");
        assert!(plant.node_count() > 1);
        assert_eq!(studio.params.seed, 555);
        let evidence = studio.evidence.as_ref().expect("rule evidence present");
        assert!(evidence.bindings.iter().any(|b| b.material == "bark" && b.matched));
        // Leaf binding resolves against real leaf instances.
        let foliage = evidence
            .bindings
            .iter()
            .find(|b| b.material == "foliage")
            .unwrap();
        assert!(foliage.matched);
    }

    #[test]
    fn studio_migrates_legacy_to_explicit_and_rebuilds() {
        let mut studio = PlantStudio::new();
        studio.rule_text = LEGACY_RULE.to_string();
        let pending = studio.migrate_editor_text().expect("migration succeeds");
        assert_eq!(pending, 0);
        assert!(studio.rule_text.contains("plant-rules/v2"));
        assert!(studio.rule_text.contains("migration-version: rule-migration/v1-to-v2"));
        // The migrated document is now the loaded source of truth.
        let loaded = studio.loaded.as_ref().expect("loaded rule present");
        assert_eq!(loaded.format, "plant-rules/v2");
        assert!(studio.plant.is_some());
    }

    #[test]
    fn studio_reports_rule_errors_without_dropping_the_plant() {
        let mut studio = PlantStudio::new();
        let before = studio.plant.as_ref().map(|p| p.node_count());
        let bad = "plant-rules/v2\nseed value=1\ntrunk length=4 radius=0.2\n";
        let err = studio.apply_rule_text(bad).unwrap_err();
        assert!(err.contains("missing an explicit unit"));
        // Previous plant survives the failed apply.
        assert_eq!(studio.plant.as_ref().map(|p| p.node_count()), before);
    }

    #[test]
    fn studio_pending_rules_are_counted_not_hidden() {
        let mut studio = PlantStudio::new();
        studio.rule_text = format!("{LEGACY_RULE}season winter\n");
        let pending = studio.migrate_editor_text().expect("migration still completes");
        assert_eq!(pending, 1);
        assert!(studio.rule_text.contains("PENDING"));
        assert!(studio.rule_text.contains("season winter"));
    }
}
