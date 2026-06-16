use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::Vec3;

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/mtext.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "MTEXT",
        label: "MText",
        icon: ICON,
        event: ModuleEvent::Command("MTEXT".to_string()),
    }
}

enum Step {
    InsertPoint,
}

pub struct MTextCommand {
    step: Step,
}

impl MTextCommand {
    pub fn new() -> Self {
        Self {
            step: Step::InsertPoint,
        }
    }
}

impl CadCommand for MTextCommand {
    fn name(&self) -> &'static str {
        "MTEXT"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::InsertPoint => "MTEXT  Specify insertion point:".into(),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        // Hand off to the in-place editor (toolbar + text area + live preview).
        CmdResult::OpenMTextEditor {
            pos: pt,
            handle: None,
            initial: String::new(),
            height: 0.25,
        }
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
inventory::submit!(crate::command::CommandRegistration { names: &["MT", "MTEXT"] });  // MTextCommand
