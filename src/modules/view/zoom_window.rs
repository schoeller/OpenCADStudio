// ZOOM WINDOW command — pick two corners to define the zoom area.

use glam::{DVec3, Vec3};

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub const ICON: IconKind =
    IconKind::Svg(include_bytes!("../../../assets/icons/zoom_window.svg"));

/// Ribbon button: zoom into a rectangle picked by two corners.
pub fn tool() -> ToolDef {
    ToolDef {
        id: "ZOOM_WINDOW",
        label: "Zoom Window",
        icon: ICON,
        event: ModuleEvent::Command("ZOOM WINDOW".to_string()),
    }
}

pub struct ZoomWindowCommand {
    first: Option<Vec3>,
}

impl ZoomWindowCommand {
    pub fn new() -> Self {
        Self { first: None }
    }
}

impl CadCommand for ZoomWindowCommand {
    fn name(&self) -> &'static str {
        "ZOOM WINDOW"
    }

    fn prompt(&self) -> String {
        if self.first.is_none() {
            "ZOOM WINDOW  Specify first corner:".into()
        } else {
            "ZOOM WINDOW  Specify opposite corner:".into()
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult { let pt = pt.as_vec3();
        if let Some(p1) = self.first {
            CmdResult::ZoomToWindow { p1: p1.as_dvec3(), p2: pt.as_dvec3() }
        } else {
            self.first = Some(pt);
            CmdResult::NeedPoint
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        let p1 = self.first?;
        let min = p1.min(pt);
        let max = p1.max(pt);
        // Draw a rectangle preview
        Some(WireModel {
            taper_widths: Vec::new(),
            world_width: 0.0,
            fill_is_3d: false,
            pick_tris: Vec::new(),
            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
            name: "zoom_window_preview".into(),
            points: vec![
                [min.x, min.y, 0.0],
                [max.x, min.y, 0.0],
                [max.x, max.y, 0.0],
                [min.x, max.y, 0.0],
                [min.x, min.y, 0.0],
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
