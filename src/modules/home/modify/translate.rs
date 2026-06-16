// Move tool — ribbon definition + interactive command.
//
// Command:  MOVE (M)
//   Requires at least one entity selected before starting.
//   Step 1: pick base point
//   Step 2: pick destination → translates all selected entities by (dest - base)

use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult, EntityTransform};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

// ── Ribbon definition ──────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "MOVE",
        label: "Move",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/move.svg")),
        event: ModuleEvent::Command("MOVE".to_string()),
    }
}

// ── Command implementation ─────────────────────────────────────────────────

enum Step {
    Base,
    Target(Vec3),
}

pub struct MoveCommand {
    handles: Vec<Handle>,
    wire_models: Vec<WireModel>,
    step: Step,
}

impl MoveCommand {
    pub fn new(handles: Vec<Handle>, wire_models: Vec<WireModel>) -> Self {
        Self {
            handles,
            wire_models,
            step: Step::Base,
        }
    }
}

impl CadCommand for MoveCommand {
    fn name(&self) -> &'static str {
        "MOVE"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Base => format!(
                "MOVE  Specify base point  [{} objects]:",
                self.handles.len()
            ),
            Step::Target(base) => format!(
                "MOVE  Specify destination  [base {:.3},{:.3}]:",
                base.x, base.y
            ),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match &self.step {
            Step::Base => {
                self.step = Step::Target(pt);
                CmdResult::NeedPoint
            }
            Step::Target(base) => {
                let delta = pt - *base;
                CmdResult::TransformSelected(
                    self.handles.clone(),
                    EntityTransform::Translate(delta),
                )
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
        let Step::Target(base) = &self.step else {
            return vec![];
        };
        let delta = pt - *base;
        // Translated ghost of each selected object + rubber-band line.
        let mut out: Vec<WireModel> = self
            .wire_models
            .iter()
            .map(|w| w.translated(delta))
            .collect();
        out.push(WireModel::solid(
            "rubber_band".into(),
            vec![[base.x, base.y, base.z], [pt.x, pt.y, pt.z]],
            WireModel::CYAN,
            false,
        ));
        out
    }
}
