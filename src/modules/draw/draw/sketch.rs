// Freehand sketch tool — interactive command.
//
// Command: SKETCH — freehand sketching. A click toggles the pen down/up. While
// the pen is DOWN, moving the cursor records its position into the current
// stroke whenever it has travelled more than a small fixed threshold, and a
// cyan preview wire of the current stroke is shown. A click while the pen is
// down lifts it, ending the current stroke; the next click starts a fresh one.
// Enter commits every recorded stroke (each stroke of two or more points
// becomes one lightweight polyline of straight segments); Esc cancels.
//
// Geometry is built like the wide-line / ring tools: each stroke is one
// LwPolyline whose vertices carry the sampled XY positions and whose elevation
// is the first sample's Z.

use acadrust::entities::{LwPolyline, LwVertex};
use acadrust::types::{Vector2, Vector3};
use acadrust::EntityType;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::DVec3;

// Minimum cursor travel (drawing units) before a new sample is recorded.
const SAMPLE_EPSILON: f64 = 0.5;

// ── Ribbon definition ─────────────────────────────────────────────────────

#[allow(dead_code)] // ribbon definition ready for wiring; command works via the command line
pub fn tool() -> ToolDef {
    ToolDef {
        id: "SKETCH",
        label: "Sketch",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/line.svg")),
        event: ModuleEvent::Command("SKETCH".to_string()),
    }
}

// ── Command implementation ────────────────────────────────────────────────

pub struct SketchCommand {
    /// All completed strokes plus, while the pen is down, the one being drawn
    /// as the last element. Each stroke is its own list of sampled points.
    strokes: Vec<Vec<DVec3>>,
    /// True while the pen is down and samples accumulate into the current stroke.
    pen_down: bool,
}

impl SketchCommand {
    pub fn new() -> Self {
        Self {
            strokes: Vec::new(),
            pen_down: false,
        }
    }

    /// Build one lightweight polyline from a stroke's sampled points, or `None`
    /// when the stroke has too few points to form a segment.
    fn build_stroke(points: &[DVec3]) -> Option<EntityType> {
        if points.len() < 2 {
            return None;
        }
        let elevation = points[0].z;
        let mut pl = LwPolyline::new();
        pl.is_closed = false;
        pl.elevation = elevation;
        pl.vertices = points
            .iter()
            .map(|p| LwVertex::new(Vector2::new(p.x, p.y)))
            .collect();
        pl.normal = Vector3::UNIT_Z;
        Some(EntityType::LwPolyline(pl))
    }

    /// All strokes that have enough points, as committable entities.
    fn build_all(&self) -> Vec<EntityType> {
        self.strokes
            .iter()
            .filter_map(|s| Self::build_stroke(s))
            .collect()
    }
}

impl CadCommand for SketchCommand {
    fn name(&self) -> &'static str {
        "SKETCH"
    }

    fn prompt(&self) -> String {
        if self.pen_down {
            "SKETCH  Pen down — move to sketch, click to lift, Enter to record:".to_string()
        } else {
            "SKETCH  Pen up — click to lower the pen, Enter to record:".to_string()
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        if self.pen_down {
            // Lift the pen: the current stroke is finished. A new click later
            // begins a fresh stroke.
            self.pen_down = false;
        } else {
            // Lower the pen: start a new stroke seeded with this point.
            self.strokes.push(vec![pt]);
            self.pen_down = true;
        }
        CmdResult::NeedPoint
    }

    fn on_enter(&mut self) -> CmdResult {
        let entities = self.build_all();
        match entities.len() {
            0 => CmdResult::Cancel,
            1 => CmdResult::CommitAndExit(entities.into_iter().next().unwrap()),
            _ => CmdResult::ReplaceMany(vec![], entities),
        }
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        if !self.pen_down {
            return None;
        }
        // The current stroke is the last one pushed while the pen is down.
        let stroke = self.strokes.last_mut()?;
        // Record the sample only once it has moved past the threshold from the
        // last recorded point, so the polyline isn't flooded with near-duplicate
        // vertices.
        let record = match stroke.last() {
            Some(last) => last.distance(pt) > SAMPLE_EPSILON,
            None => true,
        };
        if record {
            stroke.push(pt);
        }
        // Preview the current stroke, including the (possibly unrecorded) cursor
        // position so the line tracks the pointer smoothly.
        let mut pts: Vec<[f64; 3]> = stroke.iter().map(|p| [p.x, p.y, p.z]).collect();
        if !record {
            pts.push([pt.x, pt.y, pt.z]);
        }
        if pts.len() < 2 {
            return None;
        }
        Some(WireModel::solid_f64(
            "rubber_band".to_string(),
            pts,
            WireModel::CYAN,
            false,
        ))
    }
}

// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["SKETCH"] });  // SketchCommand
