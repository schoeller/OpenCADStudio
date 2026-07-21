// TOLERANCE command — place a GD&T (geometric dimensioning & tolerancing) frame.
//
// Workflow:
//   1. Text: Enter tolerance string  (e.g. "%%v0.05|A" or plain text)
//   2. Point: Click insertion point

use acadrust::entities::Tolerance;
use acadrust::types::Vector3;
use acadrust::EntityType;
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/tolerance.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "TOLERANCE",
        label: "Tolerance",
        icon: ICON,
        event: ModuleEvent::Command("TOLERANCE".to_string()),
    }
}

enum Step {
    Text,
    Insertion { text: String },
}

pub struct ToleranceCommand {
    step: Step,
}

impl ToleranceCommand {
    pub fn new() -> Self {
        Self { step: Step::Text }
    }
}

impl CadCommand for ToleranceCommand {
    fn name(&self) -> &'static str {
        "TOLERANCE"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Text => "TOLERANCE  Enter tolerance text:".into(),
            Step::Insertion { text } => format!("TOLERANCE  Specify insertion point  [{text}]:"),
        }
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.step, Step::Text)
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        let t = text.trim().to_string();
        if t.is_empty() {
            return Some(CmdResult::Cancel);
        }
        self.step = Step::Insertion { text: t };
        Some(CmdResult::NeedPoint)
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        if let Step::Insertion { text } = &self.step {
            let ins = Vector3::new(pt.x, pt.y, pt.z);
            let tol = Tolerance::with_text(ins, text.clone());
            CmdResult::CommitAndExit(EntityType::Tolerance(tol))
        } else {
            CmdResult::NeedPoint
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        if !matches!(self.step, Step::Insertion { .. }) {
            return None;
        }
        let d = 0.15_f32;
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
            name: "tolerance_preview".into(),
            points: vec![
                [pt.x - d, pt.y, pt.z],
                [pt.x + d, pt.y, pt.z],
                [f32::NAN, 0.0, 0.0],
                [pt.x, pt.y, pt.z - d],
                [pt.x, pt.y, pt.z + d],
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
inventory::submit!(crate::command::CommandRegistration { names: &["TOLERANCE"] });  // ToleranceCommand
