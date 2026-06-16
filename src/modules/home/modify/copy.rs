// Copy tool — ribbon definition + interactive command.
//
// Command:  COPY (CO)
//   Requires at least one entity selected before starting.
//   Step 1: pick base point
//   Step 2+: each click makes another copy at (click - base); Enter to finish.

use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult, EntityTransform};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

// ── Ribbon definition ──────────────────────────────────────────────────────

pub fn tool() -> ToolDef {
    ToolDef {
        id: "COPY",
        label: "Copy",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/copy.svg")),
        event: ModuleEvent::Command("COPY".to_string()),
    }
}

// ── Command implementation ─────────────────────────────────────────────────

enum Step {
    Base,
    Placing(Vec3),
}

pub struct CopyCommand {
    handles: Vec<Handle>,
    wire_models: Vec<WireModel>,
    step: Step,
    count: usize,
}

impl CopyCommand {
    pub fn new(handles: Vec<Handle>, wire_models: Vec<WireModel>) -> Self {
        Self {
            handles,
            wire_models,
            step: Step::Base,
            count: 0,
        }
    }
}

impl CadCommand for CopyCommand {
    fn name(&self) -> &'static str {
        "COPY"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Base => format!(
                "COPY  Specify base point  [{} objects]:",
                self.handles.len()
            ),
            Step::Placing(base) => format!(
                "COPY  Specify destination  [{} copies so far | Enter=done | base {:.3},{:.3}]:",
                self.count, base.x, base.y
            ),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match &self.step {
            Step::Base => {
                self.step = Step::Placing(pt);
                CmdResult::NeedPoint
            }
            Step::Placing(base) => {
                let delta = pt - *base;
                self.count += 1;
                CmdResult::CopySelected(self.handles.clone(), EntityTransform::Translate(delta))
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
        let Step::Placing(base) = &self.step else {
            return vec![];
        };
        let delta = pt - *base;
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
