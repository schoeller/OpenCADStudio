// SelectObjectsCommand — generic "select objects then run command" gather phase.
//
// Used when a modify command is invoked with nothing pre-selected.
// The user may single-click, box-select, or polygon-select any number of objects.
// After the FIRST completed selection action (regardless of method) the command fires
// immediately — no Enter required.

use acadrust::Handle;
use glam::Vec3;

use crate::command::{CadCommand, CmdResult};
use crate::scene::model::wire_model::WireModel;

pub struct SelectObjectsCommand {
    pending_cmd: String,
}

impl SelectObjectsCommand {
    pub fn new(pending_cmd: &str) -> Self {
        Self {
            pending_cmd: pending_cmd.to_string(),
        }
    }
}

impl CadCommand for SelectObjectsCommand {
    fn name(&self) -> &'static str {
        "SELECT"
    }

    fn prompt(&self) -> String {
        format!("{}  Select objects:", self.pending_cmd)
    }

    // Opt into the selection-gathering path; host routes clicks through
    // the normal selection system and calls on_selection_complete after each action.
    fn is_selection_gathering(&self) -> bool {
        true
    }

    fn on_selection_complete(&mut self, handles: Vec<Handle>) -> CmdResult {
        if handles.is_empty() {
            return CmdResult::NeedPoint;
        }
        CmdResult::Relaunch(std::mem::take(&mut self.pending_cmd), handles)
    }

    // These are never called while is_selection_gathering is true, but the
    // trait requires implementations.
    fn on_point(&mut self, _pt: Vec3) -> CmdResult {
        CmdResult::NeedPoint
    }
    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_hover_entity(&mut self, _handle: Handle, _pt: Vec3) -> Vec<WireModel> {
        vec![]
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["AR", "ARRAY", "ARRAYPATH", "ARRAYPOLAR", "ARRAYRECT", "BLOCK", "CC", "CO", "COPY", "COPYCLIP", "CUTCLIP", "CX", "E", "ERASE", "EXPLODE", "G", "GROUP", "LAYFRZ", "LAYLCK", "LAYMCUR", "LAYOFF", "LAYULK", "M", "MI", "MIRROR", "MOVE", "RO", "ROTATE", "SC", "SCALE", "SS", "STRETCH", "X"] });  // SelectObjectsCommand
