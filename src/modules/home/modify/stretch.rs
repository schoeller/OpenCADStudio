// Stretch tool — ribbon definition + interactive command.
//
// Command:  STRETCH (SS)
//   Workflow:
//     1. Pick first corner of the crossing window (right-to-left = crossing).
//     2. Pick second corner.
//     3. Pick base point.
//     4. Pick new point → stretches only vertices inside the crossing window.
//
//   Entity behaviour:
//     Line        : move start if inside, move end if inside, move both if both inside.
//     LwPolyline  : move each vertex independently.
//     Polyline/P2D: move each vertex independently.
//     Arc / Circle: move the whole entity if its center is inside the window.
//     Insert      : move the whole entity if its insertion point is inside.
//     All others  : move the whole entity if any point is inside.

use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::wire_model::WireModel;

// ── Ribbon definition ──────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "STRETCH",
        label: "Stretch",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/stretch.svg")),
        event: ModuleEvent::Command("STRETCH".to_string()),
    }
}

// ── Command implementation ─────────────────────────────────────────────────

enum Step {
    /// Waiting for the first crossing-window corner.
    WindowCorner1,
    /// Waiting for the second corner; `c1` is the first corner.
    WindowCorner2(Vec3),
    /// Crossing window defined; waiting for base point.
    Base { win_min: Vec3, win_max: Vec3 },
    /// Waiting for target point.
    Target {
        win_min: Vec3,
        win_max: Vec3,
        base: Vec3,
    },
}

pub struct StretchCommand {
    handles: Vec<Handle>,
    wire_models: Vec<WireModel>,
    step: Step,
}

impl StretchCommand {
    pub fn new(handles: Vec<Handle>, wire_models: Vec<WireModel>) -> Self {
        Self {
            handles,
            wire_models,
            step: Step::WindowCorner1,
        }
    }
}

impl CadCommand for StretchCommand {
    fn name(&self) -> &'static str {
        "STRETCH"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::WindowCorner1 => format!(
                "STRETCH  Specify first corner of crossing window  [{} objects]:",
                self.handles.len()
            ),
            Step::WindowCorner2(_) => "STRETCH  Specify opposite corner:".into(),
            Step::Base { .. } => "STRETCH  Specify base point:".into(),
            Step::Target { base, .. } => format!(
                "STRETCH  Specify new point  [base {:.3},{:.3}]:",
                base.x, base.z
            ),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match &self.step {
            Step::WindowCorner1 => {
                self.step = Step::WindowCorner2(pt);
                CmdResult::NeedPoint
            }
            Step::WindowCorner2(c1) => {
                let win_min = c1.min(pt);
                let win_max = c1.max(pt);
                self.step = Step::Base { win_min, win_max };
                CmdResult::NeedPoint
            }
            Step::Base { win_min, win_max } => {
                let (wmin, wmax) = (*win_min, *win_max);
                self.step = Step::Target {
                    win_min: wmin,
                    win_max: wmax,
                    base: pt,
                };
                CmdResult::NeedPoint
            }
            Step::Target {
                win_min,
                win_max,
                base,
            } => {
                let delta = pt - *base;
                CmdResult::StretchEntities {
                    handles: self.handles.clone(),
                    win_min: *win_min,
                    win_max: *win_max,
                    delta,
                }
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_preview_wires(&mut self, pt: Vec3) -> Vec<WireModel> {
        match &self.step {
            Step::WindowCorner2(c1) => {
                // Show crossing-window rectangle preview (dashed green)
                let c1 = *c1;
                let pts = vec![
                    [c1.x, c1.y, c1.z],
                    [pt.x, c1.y, c1.z],
                    [pt.x, pt.y, pt.z],
                    [c1.x, pt.y, pt.z],
                    [c1.x, c1.y, c1.z],
                ];
                vec![WireModel::solid(
                    "stretch_window".into(),
                    pts,
                    [0.3, 1.0, 0.3, 0.7],
                    false,
                )]
            }
            Step::Target {
                win_min,
                win_max,
                base,
            } => {
                let delta = pt - *base;
                // Live ghost: vertices inside the crossing window follow the
                // cursor, the rest stay anchored.
                let mut out: Vec<WireModel> = self
                    .wire_models
                    .iter()
                    .map(|w| w.stretched(*win_min, *win_max, delta))
                    .collect();
                out.push(WireModel::solid(
                    "rubber_band".into(),
                    vec![[base.x, base.y, base.z], [pt.x, pt.y, pt.z]],
                    WireModel::CYAN,
                    false,
                ));
                out
            }
            _ => vec![],
        }
    }
}
