use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;

pub fn tool() -> ToolDef {
    ToolDef {
        id: "BLOCK",
        label: "Create Block",
        icon: IconKind::Svg(include_bytes!("../../../assets/icons/blocks/block.svg")),
        event: ModuleEvent::Command("BLOCK".to_string()),
    }
}

enum Step {
    Name,
    Base { name: String },
}

pub struct CreateBlockCommand {
    handles: Vec<Handle>,
    step: Step,
}

impl CreateBlockCommand {
    pub fn new(handles: Vec<Handle>) -> Self {
        Self {
            handles,
            step: Step::Name,
        }
    }
}

impl CadCommand for CreateBlockCommand {
    fn name(&self) -> &'static str {
        "BLOCK"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::Name => format!(
                "BLOCK  Enter block name  [{} objects selected]:",
                self.handles.len()
            ),
            Step::Base { name } => format!(
                "BLOCK  Specify base point for \"{}\"  [{} objects]:",
                name,
                self.handles.len()
            ),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        match &self.step {
            Step::Name => CmdResult::NeedPoint,
            Step::Base { name } => CmdResult::CreateBlock {
                handles: self.handles.clone(),
                name: name.clone(),
                base: pt,
            },
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        matches!(self.step, Step::Name)
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if !matches!(self.step, Step::Name) {
            return None;
        }
        let name = text.trim();
        if name.is_empty() {
            return None;
        }
        self.step = Step::Base {
            name: name.to_string(),
        };
        Some(CmdResult::NeedPoint)
    }

    fn on_preview_wires(&mut self, _pt: Vec3) -> Vec<WireModel> {
        vec![]
    }
}
