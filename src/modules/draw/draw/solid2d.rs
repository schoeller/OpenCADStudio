// 2D solid tool — interactive command.
//
// Command: SOLID (reachable as SO / SOLID2D — the bare SOLID verb currently
// toggles the shaded display) — pick three or four corner points and commit a
// filled triangle or quadrilateral. Points are taken in the order picked: the
// third point is the corner diagonally opposite the second, so a four-point
// solid is entered in a Z pattern (entering the corners in ring order yields
// the classic bow-tie fill). Enter after the third point commits a triangle.

use acadrust::entities::Solid;
use acadrust::types::Vector3;
use acadrust::EntityType;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

// ── Ribbon definition ─────────────────────────────────────────────────────

#[allow(dead_code)] // ribbon definition ready for wiring; command works via the command line
pub fn tool() -> ToolDef {
    ToolDef {
        id: "SOLID2D",
        label: "2D Solid",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/line.svg")),
        event: ModuleEvent::Command("SOLID2D".to_string()),
    }
}

// ── Command implementation ────────────────────────────────────────────────

pub struct Solid2dCommand {
    /// Corner points picked so far (3 → triangle, 4 → quadrilateral).
    points: Vec<DVec3>,
}

impl Solid2dCommand {
    pub fn new() -> Self {
        Self { points: Vec::new() }
    }

    fn v3(p: DVec3) -> Vector3 {
        Vector3::new(p.x, p.y, p.z)
    }
}

impl CadCommand for Solid2dCommand {
    fn name(&self) -> &'static str {
        "SOLID"
    }

    fn prompt(&self) -> String {
        match self.points.len() {
            0 => "SOLID  Specify first point:".to_string(),
            1 => "SOLID  Specify second point:".to_string(),
            2 => "SOLID  Specify third point:".to_string(),
            _ => "SOLID  Specify fourth point  [Enter = triangle]:".to_string(),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        self.points.push(pt);
        if self.points.len() == 4 {
            let p = &self.points;
            let solid = Solid::new(
                Self::v3(p[0]),
                Self::v3(p[1]),
                Self::v3(p[2]),
                Self::v3(p[3]),
            );
            CmdResult::CommitAndExit(EntityType::Solid(solid))
        } else {
            CmdResult::NeedPoint
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        if self.points.len() == 3 {
            let p = &self.points;
            let solid = Solid::triangle(Self::v3(p[0]), Self::v3(p[1]), Self::v3(p[2]));
            CmdResult::CommitAndExit(EntityType::Solid(solid))
        } else {
            CmdResult::Cancel
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        if self.points.is_empty() {
            return None;
        }
        // Outline the corners picked so far plus the cursor, closed back to the
        // first point, as a rubber-band hint.
        let mut pts: Vec<[f64; 3]> = self.points.iter().map(|p| [p.x, p.y, p.z]).collect();
        pts.push([pt.x, pt.y, pt.z]);
        pts.push([self.points[0].x, self.points[0].y, self.points[0].z]);
        Some(WireModel::solid_f64(
            "rubber_band".to_string(),
            pts,
            WireModel::CYAN,
            false,
        ))
    }
}

// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["SO", "SOLID2D"] });  // Solid2dCommand
