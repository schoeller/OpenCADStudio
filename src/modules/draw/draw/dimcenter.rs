// Center mark tool — interactive command.
//
// Command: DIMCENTER (aliases DCE, CENTERMARK) — pick a Circle or Arc and draw
// a small cross of two Line entities through its center. Each arm of the cross
// extends a distance of `radius * 0.2` from the center in the four cardinal
// directions, so the result is a horizontal line from (cx - m, cy) to
// (cx + m, cy) and a vertical line from (cx, cy - m) to (cx, cy + m), with
// `m = radius * 0.2`. Both lines are committed at once and the command ends.

use acadrust::types::Vector3;
use acadrust::{EntityType, Handle, Line};
use glam::DVec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};

// ── Ribbon definition ─────────────────────────────────────────────────────

#[allow(dead_code)] // ribbon definition ready for wiring; command works via the command line
pub fn tool() -> ToolDef {
    ToolDef {
        id: "DIMCENTER",
        label: "Center Mark",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/line.svg")),
        event: ModuleEvent::Command("DIMCENTER".to_string()),
    }
}

// ── Command implementation ────────────────────────────────────────────────

pub struct DimCenterCommand {
    /// The picked entity, injected by the host before `on_entity_pick` runs.
    /// `None` until the host injects it.
    picked: Option<EntityType>,
}

impl DimCenterCommand {
    pub fn new() -> Self {
        Self { picked: None }
    }

    /// Extract (center, radius) from a Circle or Arc; `None` for anything else.
    fn center_radius(entity: &EntityType) -> Option<(Vector3, f64)> {
        match entity {
            EntityType::Circle(c) => Some((c.center, c.radius)),
            EntityType::Arc(a) => Some((a.center, a.radius)),
            _ => None,
        }
    }

    /// Build the two cross lines from a center and radius.
    fn build_cross(center: Vector3, radius: f64) -> Vec<EntityType> {
        let m = radius * 0.2;
        let cx = center.x;
        let cy = center.y;
        let cz = center.z;
        let horizontal = Line::from_points(
            Vector3::new(cx - m, cy, cz),
            Vector3::new(cx + m, cy, cz),
        );
        let vertical = Line::from_points(
            Vector3::new(cx, cy - m, cz),
            Vector3::new(cx, cy + m, cz),
        );
        vec![EntityType::Line(horizontal), EntityType::Line(vertical)]
    }
}

impl CadCommand for DimCenterCommand {
    fn name(&self) -> &'static str {
        "DIMCENTER"
    }

    fn prompt(&self) -> String {
        "DIMCENTER  Select arc or circle:".to_string()
    }

    fn needs_entity_pick(&self) -> bool {
        true
    }

    fn inject_before_entity_pick(&self) -> bool {
        true
    }

    fn inject_picked_entity(&mut self, entity: EntityType) {
        self.picked = Some(entity);
    }

    fn on_entity_pick(&mut self, handle: Handle, _pt: DVec3) -> CmdResult {
        if handle.is_null() {
            return CmdResult::NeedPoint;
        }
        match self.picked.as_ref().and_then(Self::center_radius) {
            Some((center, radius)) => {
                let lines = Self::build_cross(center, radius);
                CmdResult::ReplaceMany(vec![], lines)
            }
            // Picked something that is not a circle or arc — keep prompting.
            None => CmdResult::NeedPoint,
        }
    }

    fn on_point(&mut self, _pt: DVec3) -> CmdResult {
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
}

// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration {
    names: &["DIMCENTER", "DCE", "CENTERMARK"]
}); // DimCenterCommand
