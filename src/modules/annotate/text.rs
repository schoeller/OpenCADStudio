use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::Vec3;

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/text.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "TEXT",
        label: "Text",
        icon: ICON,
        event: ModuleEvent::Command("TEXT".to_string()),
    }
}

enum Step {
    InsertPoint,
}

pub struct TextCommand {
    step: Step,
}

impl TextCommand {
    pub fn new() -> Self {
        Self {
            step: Step::InsertPoint,
        }
    }
}

impl CadCommand for TextCommand {
    fn name(&self) -> &'static str {
        "TEXT"
    }

    fn prompt(&self) -> String {
        match &self.step {
            Step::InsertPoint => "TEXT  Specify insertion point:".into(),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        // Hand off to the in-place plain-text editor anchored at the click.
        CmdResult::OpenTextEditor {
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
inventory::submit!(crate::command::CommandRegistration { names: &["DT", "T", "TEXT"] });  // TextCommand
