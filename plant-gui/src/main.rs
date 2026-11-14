//! Realtime parameter editor for the procedural plant generator.
//!
//! The left panel edits the same [`PlantParams`] the CLI uses — including seed,
//! branch count, angle, length ratio and the resource budgets — and every
//! change rebuilds the plant through the deterministic core. The right panel
//! renders the actual generated splines and leaf instances with an orbit
//! camera; it never displays a pre-made model.

use eframe::egui::{self, Color32, Pos2, Slider, Ui, Vec2};
use plant_core::{
    build_leaves, build_meshes, build_plant, validate_plant, LeafParams, Limits, PlantParams,
};

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
    params: PlantParams,
    plant: Option<plant_core::Plant>,
    status: String,
    cam_yaw: f32,
    cam_pitch: f32,
    cam_dist: f32,
    auto_rebuild: bool,
    rebuild_requested: bool,
}

impl Default for PlantApp {
    fn default() -> Self {
        let mut app = Self {
            params: PlantParams::default(),
            plant: None,
            status: "not built yet".into(),
            cam_yaw: 0.6,
            cam_pitch: 0.35,
            cam_dist: 14.0,
            auto_rebuild: true,
            rebuild_requested: true,
        };
        app.rebuild();
        app
    }
}

impl PlantApp {
    fn rebuild(&mut self) {
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
                        drop(meshes);
                    }
                    Err(e) => self.status = format!("validation failed: {e}"),
                }
            }
            Err(e) => self.status = format!("build error: {e}"),
        }
        self.rebuild_requested = false;
    }

    fn request_rebuild(&mut self) {
        if self.auto_rebuild {
            self.rebuild();
        } else {
            self.rebuild_requested = true;
        }
    }
}

impl eframe::App for PlantApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::SidePanel::left("controls").default_width(360.0).show(ctx, |ui| {
            ui.heading("Procedural Plant");
            ui.separator();
            let before = self.params.clone();
            parameter_panel(ui, &mut self.params);
            ui.separator();
            ui.checkbox(&mut self.auto_rebuild, "Rebuild on change");
            if !self.auto_rebuild && ui.button("Rebuild now").clicked() {
                self.rebuild_requested = true;
            }
            if ui.button("Reset to defaults").clicked() {
                self.params = PlantParams::default();
                self.rebuild_requested = true;
            }
            if self.params != before {
                self.request_rebuild();
            }
            if self.rebuild_requested {
                self.rebuild();
            }
            ui.separator();
            ui.label(egui::RichText::new(&self.status).small().color(Color32::LIGHT_GREEN));
            ui.label(
                egui::RichText::new(format!("seed = {:#018x}", self.params.seed)).small(),
            );
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            handle_camera(ui, self);
            let painter = ui.painter();
            let rect = ui.available_rect_before_wrap();
            painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 22, 20));
            if let Some(plant) = &self.plant {
                draw_plant(painter, rect, plant, self);
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }
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

fn draw_plant(
    painter: &egui::Painter,
    rect: egui::Rect,
    plant: &plant_core::Plant,
    app: &PlantApp,
) {
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
