// Line tool — ribbon definition + interactive command.
//
// Command:  LINE — OpenCADStudio behaviour:
//   1. First click  → stores start point, prompts for next point
//   2. Each further click → immediately commits an acadrust::Line entity
//      (start→end) to the document; end becomes the new start point
//   3. Enter / Escape → ends the command

use acadrust::types::Vector3;
use acadrust::{EntityType, Line};

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

// ── Ribbon definition ─────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "LINE",
        label: "Line",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/line.svg")),
        event: ModuleEvent::Command("LINE".to_string()),
    }
}

// ── Command implementation ────────────────────────────────────────────────

pub struct LineCommand {
    /// Every point picked so far. `points[0]` is the start (needed by Close);
    /// `points.last()` is the start of the next segment. Each pick after the
    /// first commits one Line entity, so the count of committed segments is
    /// `points.len() - 1`.
    points: Vec<DVec3>,
}

impl LineCommand {
    pub fn new() -> Self {
        Self { points: Vec::new() }
    }

    fn line_between(a: DVec3, b: DVec3) -> EntityType {
        EntityType::Line(Line::from_points(
            Vector3::new(a.x, a.y, a.z),
            Vector3::new(b.x, b.y, b.z),
        ))
    }
}

impl CadCommand for LineCommand {
    fn name(&self) -> &'static str {
        "LINE"
    }

    fn prompt(&self) -> String {
        match self.points.len() {
            0 => "LINE  Specify first point:".to_string(),
            1 => "LINE  Specify next point  [Undo]:".to_string(),
            _ => "LINE  Specify next point  [Close/Undo]:".to_string(),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        if let Some(&last) = self.points.last() {
            let line = Self::line_between(last, pt);
            self.points.push(pt);
            CmdResult::CommitEntity(line)
        } else {
            self.points.push(pt);
            CmdResult::NeedPoint
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        // Accept Close / Undo once at least the first point is placed.
        !self.points.is_empty()
    }

    fn point_step_accepts_keywords(&self) -> bool {
        // The next-point pick also takes C / U, so the polar dynamic-input
        // distance/angle boxes stay visible while the keywords are available.
        !self.points.is_empty()
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        match text.trim().to_uppercase().as_str() {
            "C" | "CLOSE" => {
                // Need at least two points to draw a closing segment back to
                // the start; then finish the command.
                if self.points.len() >= 2 {
                    let close = Self::line_between(
                        *self.points.last().unwrap(),
                        self.points[0],
                    );
                    Some(CmdResult::CommitAndExit(close))
                } else {
                    Some(CmdResult::NeedPoint)
                }
            }
            "U" | "UNDO" => {
                if self.points.len() >= 2 {
                    // Drop the last vertex and revert its committed segment.
                    self.points.pop();
                    Some(CmdResult::UndoDocument)
                } else if self.points.len() == 1 {
                    // Only the start is placed (nothing committed yet) — clear
                    // it so the next pick restarts the line.
                    self.points.clear();
                    Some(CmdResult::NeedPoint)
                } else {
                    Some(CmdResult::NeedPoint)
                }
            }
            _ => None,
        }
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        let last = *self.points.last()?;
        Some(WireModel::solid_f64(
            "rubber_band".to_string(),
            vec![[last.x, last.y, last.z], [pt.x, pt.y, pt.z]],
            WireModel::CYAN,
            false,
        ))
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["L", "LINE"] });  // LineCommand
