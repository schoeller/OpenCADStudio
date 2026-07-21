// MVIEW — interactive paper-space viewport creation.
//
// Two clicks define opposite corners of a new viewport rectangle.
// The created Viewport entity is routed to add_entity_to_layout by apply_cmd_result
// because we're in paper space.

use acadrust::entities::Viewport;
use acadrust::types::Vector3;
use acadrust::EntityType;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

// ── Ribbon definition ─────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "MVIEW",
        label: "Viewport",
        icon: IconKind::Svg(include_bytes!("../../../assets/icons/viewport.svg")),
        event: ModuleEvent::Command("MVIEW".to_string()),
    }
}

// ── Command ───────────────────────────────────────────────────────────────

pub struct MviewCommand {
    corner1: Option<DVec3>,
}

impl MviewCommand {
    pub fn new() -> Self {
        Self { corner1: None }
    }
}

impl CadCommand for MviewCommand {
    fn name(&self) -> &'static str {
        "MVIEW"
    }

    fn prompt(&self) -> String {
        if self.corner1.is_none() {
            "MVIEW  Specify first corner:".to_string()
        } else {
            "MVIEW  Specify opposite corner:".to_string()
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        if let Some(c1) = self.corner1 {
            let w = (pt.x - c1.x).abs();
            let h = (pt.y - c1.y).abs();
            if w < 1.0 || h < 1.0 {
                return CmdResult::Cancel;
            }
            let cx = (c1.x + pt.x) / 2.0;
            let cy = (c1.y + pt.y) / 2.0;
            let cz = c1.z;

            let mut vp = Viewport::new();
            vp.center = Vector3::new(cx, cy, cz);
            vp.width = w;
            vp.height = h;
            vp.id = 2; // user viewport (id > 1)

            CmdResult::CommitAndExit(EntityType::Viewport(vp))
        } else {
            self.corner1 = Some(pt);
            CmdResult::NeedPoint
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        let pt = pt.as_vec3();
        let c1 = self.corner1?;
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
            name: "mview_preview".to_string(),
            points: vec![
                [c1.x as f32, c1.y as f32, c1.z as f32],
                [pt.x, c1.y as f32, c1.z as f32],
                [pt.x, pt.y, c1.z as f32],
                [c1.x as f32, pt.y, c1.z as f32],
                [c1.x as f32, c1.y as f32, c1.z as f32],
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
inventory::submit!(crate::command::CommandRegistration { names: &["MVIEW"] });  // MviewCommand
