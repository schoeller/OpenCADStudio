// AREA command — compute area and perimeter of a polygon picked point by point.
// Press Enter to close and calculate.

use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::scene::model::wire_model::WireModel;

pub struct AreaCommand {
    points: Vec<DVec3>,
}

impl AreaCommand {
    pub fn new() -> Self {
        Self { points: vec![] }
    }
}

impl CadCommand for AreaCommand {
    fn name(&self) -> &'static str {
        "AREA"
    }

    fn prompt(&self) -> String {
        if self.points.is_empty() {
            "AREA  Specify first corner point (Enter to cancel):".into()
        } else {
            format!(
                "AREA  Specify next point ({} picked, Enter to calculate):",
                self.points.len()
            )
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        self.points.push(pt);
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        if self.points.len() < 3 {
            return CmdResult::Cancel;
        }
        // Shoelace formula in the world XY plane, evaluated in f64 relative to
        // the first vertex. Subtracting that origin keeps the cross-product
        // terms small even at large (survey/UTM) coordinates, avoiding the
        // catastrophic cancellation a raw f32/absolute evaluation suffers.
        let n = self.points.len();
        let origin = self.points[0];
        let mut area_sum = 0.0f64;
        let mut perimeter = 0.0f64;
        for idx in 0..n {
            let a = self.points[idx] - origin;
            let b = self.points[(idx + 1) % n] - origin;
            area_sum += a.x * b.y - b.x * a.y;
            perimeter += (self.points[(idx + 1) % n] - self.points[idx]).length();
        }
        let area = (area_sum * 0.5).abs();
        let msg = format!("Area = {area:.4},  Perimeter = {perimeter:.4}");
        CmdResult::Measurement(msg)
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        if self.points.is_empty() {
            return None;
        }
        let f = |p: DVec3| [p.x as f32, p.y as f32, p.z as f32];
        let mut pts: Vec<[f32; 3]> = self.points.iter().map(|p| f(*p)).collect();
        pts.push(f(pt));
        pts.push(f(self.points[0]));
        Some(WireModel {
            taper_widths: Vec::new(),
            world_width: 0.0,
            depth_override: None,
            fill_is_3d: false,
            pick_tris: Vec::new(),
            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
            name: "area_preview".into(),
            points: pts,
            points_low: Vec::new(),
            color: WireModel::CYAN,
            selected: false,
            pattern_length: 0.0,
            pattern: [0.0; 8],
            line_weight_px: 1.0,
            snap_pts: vec![],
            tangent_geoms: vec![],
            aci: 0,
            key_vertices: vec![],
            aabb: WireModel::UNBOUNDED_AABB,
            plinegen: true,
            fill_tris: vec![],
            fill_tris_low: Vec::new(),
        })
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["AREA"] });  // AreaCommand
