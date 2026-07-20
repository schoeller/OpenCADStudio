// DIST command — measure distance and angle between two picked points.

use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::scene::model::wire_model::WireModel;

pub struct DistCommand {
    // Keep the picked point in full f64 precision. Downcasting to f32 here
    // loses ~0.03–0.06 units at survey-scale coordinates (e.g. eastings near
    // 5e5), which made snapped-endpoint measurements read off by that much.
    first: Option<DVec3>,
}

impl DistCommand {
    pub fn new() -> Self {
        Self { first: None }
    }
}

impl CadCommand for DistCommand {
    fn name(&self) -> &'static str {
        "DIST"
    }

    fn prompt(&self) -> String {
        if self.first.is_none() {
            "DIST  Specify first point:".into()
        } else {
            "DIST  Specify second point:".into()
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        if let Some(p1) = self.first {
            let delta = pt - p1;
            let dist = delta.length();
            let dx = delta.x;
            let dy = delta.y; // drawing plane is world XY
            let dz = delta.z; // elevation

            // Angle in XY plane — degrees from +X
            let angle_xy = dy.atan2(dx).to_degrees();
            // Angle from XY plane toward Z (elevation angle)
            let dist_xy = dx.hypot(dy);
            let angle_z = dz.atan2(dist_xy).to_degrees();

            let msg = format!(
                "Distance = {dist:.4},  Angle in XY Plane = {angle_xy:.4}°,  Angle from XY Plane = {angle_z:.4}°\n  Delta X = {dx:.4},  Delta Y = {dy:.4},  Delta Z = {dz:.4}",
            );
            CmdResult::Measurement(msg)
        } else {
            self.first = Some(pt);
            CmdResult::NeedPoint
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        let p1 = self.first?;
        // The preview wire is purely visual, so f32 vertices are fine here.
        Some(WireModel {
            taper_widths: Vec::new(),
            world_width: 0.0,
            fill_is_3d: false,
            pick_tris: Vec::new(),
            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
            name: "dist_preview".into(),
            points: vec![
                [p1.x as f32, p1.y as f32, p1.z as f32],
                [pt.x as f32, pt.y as f32, pt.z as f32],
            ],
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
inventory::submit!(crate::command::CommandRegistration { names: &["DIST"] });  // DistCommand
