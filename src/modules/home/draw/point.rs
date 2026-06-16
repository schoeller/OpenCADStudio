// Point tool — ribbon definition + interactive command.
//
// Command:  POINT (PO)
//   Single click → commits EntityType::Point.  Stays active for more points.

use acadrust::types::Vector3;
use acadrust::{EntityType, Point as CadPoint};

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::Vec3;

#[allow(dead_code)]
pub fn tool() -> ToolDef {
    ToolDef {
        id: "POINT",
        label: "Point",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/point.svg")),
        event: ModuleEvent::Command("POINT".to_string()),
    }
}

pub struct PointCommand;

impl PointCommand {
    pub fn new() -> Self {
        Self
    }
}

impl CadCommand for PointCommand {
    fn name(&self) -> &'static str {
        "POINT"
    }
    fn prompt(&self) -> String {
        "POINT  Specify point  [Enter=done]:".into()
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        let p = CadPoint {
            location: Vector3::new(pt.x as f64, pt.y as f64, pt.z as f64),
            ..Default::default()
        };
        CmdResult::CommitEntity(EntityType::Point(p))
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_mouse_move(&mut self, _pt: Vec3) -> Option<WireModel> {
        None
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["PO", "POINT"] });  // PointCommand
