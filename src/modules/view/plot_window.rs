// PLOTWINDOW command — pick two corners to define the plot window area.
//
// In paper space it writes the layout's PlotSettings (PlotType::Window); in
// model space the host stores the window for the plot dialog.

use crate::command::{CadCommand, CmdResult};
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

pub struct PlotWindowCommand {
    p1: Option<DVec3>,
}

impl PlotWindowCommand {
    pub fn new() -> Self {
        Self { p1: None }
    }
}

impl CadCommand for PlotWindowCommand {
    fn name(&self) -> &'static str {
        "PLOTWINDOW"
    }

    fn prompt(&self) -> String {
        if self.p1.is_none() {
            "PLOTWINDOW  Specify first corner of plot window:".into()
        } else {
            "PLOTWINDOW  Specify opposite corner:".into()
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match self.p1 {
            None => {
                self.p1 = Some(pt);
                CmdResult::NeedPoint
            }
            Some(p1) => CmdResult::SetPlotWindow { p1, p2: pt },
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        let p1 = self.p1?.as_vec3();
        // Draw the selection rectangle.
        Some(WireModel {
            taper_widths: Vec::new(),
            world_width: 0.0,
            fill_is_3d: false,
            pick_tris: Vec::new(),
            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
            name: "plotwindow_preview".into(),
            points: vec![
                [p1.x, p1.y, p1.z],
                [pt.x, p1.y, p1.z],
                [pt.x, p1.y, p1.z],
                [pt.x, pt.y, pt.z],
                [pt.x, pt.y, pt.z],
                [p1.x, pt.y, pt.z],
                [p1.x, pt.y, pt.z],
                [p1.x, p1.y, p1.z],
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
inventory::submit!(crate::command::CommandRegistration { names: &["PLOTWINDOW"] });  // PlotWindowCommand
