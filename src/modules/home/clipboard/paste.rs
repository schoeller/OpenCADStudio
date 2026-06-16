// Paste tool — ribbon definition + CadCommand implementation.

use crate::modules::{IconKind, ModuleEvent, ToolDef};

pub fn tool() -> ToolDef {
    ToolDef {
        id: "PASTECLIP",
        label: "Paste",
        icon: IconKind::Svg(include_bytes!("../../../../assets/icons/paste.svg")),
        event: ModuleEvent::Command("PASTECLIP".to_string()),
    }
}

// ── CadCommand implementation ─────────────────────────────────────────────

use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::scene::model::wire_model::WireModel;

pub struct PasteCommand {
    /// Wire models of the clipboard entities (used for preview).
    wires: Vec<WireModel>,
    /// Centroid of the clipboard entities (offset origin for translation).
    centroid: Vec3,
}

impl PasteCommand {
    pub fn new(wires: Vec<WireModel>, centroid: Vec3) -> Self {
        Self { wires, centroid }
    }
}

impl CadCommand for PasteCommand {
    fn name(&self) -> &'static str {
        "PASTECLIP"
    }

    fn prompt(&self) -> String {
        "PASTECLIP  Pick insertion point:".into()
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        CmdResult::PasteClipboard { base_pt: pt }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_hover_entity(&mut self, _handle: Handle, _pt: Vec3) -> Vec<WireModel> {
        vec![]
    }

    fn on_preview_wires(&mut self, pt: Vec3) -> Vec<WireModel> {
        let delta = pt - self.centroid;
        self.wires.iter().map(|w| w.translated(delta)).collect()
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["PASTECLIP", "PC"] });  // PasteCommand
