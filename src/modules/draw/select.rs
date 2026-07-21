// SelectObjectsCommand — generic "select objects then run command" gather phase.
//
// Used when a modify command is invoked with nothing pre-selected.
// The user may single-click, box-select, or polygon-select any number of objects.
// Picks accumulate into the selection (Shift removes); the set is applied when
// the user presses Enter or right-clicks (standard "Select objects:" behaviour).
//
// Single-object commands (e.g. LAYMCUR) use `instant()` instead: the first
// completed selection action fires straight away, no Enter required.

use acadrust::Handle;
use glam::DVec3;

use crate::command::{CadCommand, CmdOption, CmdResult};
use crate::scene::model::wire_model::WireModel;

pub struct SelectObjectsCommand {
    pending_cmd: String,
    /// Selection accumulated so far (kept in sync with the scene selection by
    /// each `on_selection_complete` call). Applied on Enter when `commit_on_enter`.
    handles: Vec<Handle>,
    /// When true, gathering continues until Enter / right-click commits the set.
    /// When false, the first completed selection action fires immediately.
    commit_on_enter: bool,
}

impl SelectObjectsCommand {
    /// Standard selection set: accumulate picks, apply on Enter / right-click.
    pub fn new(pending_cmd: &str) -> Self {
        Self {
            pending_cmd: pending_cmd.to_string(),
            handles: Vec::new(),
            commit_on_enter: true,
        }
    }

    /// Single-object variant: the first completed selection action applies
    /// immediately, with no Enter (used by commands that act on one object).
    pub fn instant(pending_cmd: &str) -> Self {
        Self {
            pending_cmd: pending_cmd.to_string(),
            handles: Vec::new(),
            commit_on_enter: false,
        }
    }
}

impl CadCommand for SelectObjectsCommand {
    fn name(&self) -> &'static str {
        "SELECT"
    }

    fn prompt(&self) -> String {
        if self.commit_on_enter && !self.handles.is_empty() {
            format!(
                "{}  Select objects ({} selected, Enter to apply):",
                self.pending_cmd,
                self.handles.len()
            )
        } else {
            format!("{}  Select objects:", self.pending_cmd)
        }
    }

    // Opt into the selection-gathering path; host routes clicks through
    // the normal selection system and calls on_selection_complete after each action.
    fn is_selection_gathering(&self) -> bool {
        true
    }

    // Clickable selection keywords (#426): Previous re-selects the set the
    // last command worked on, Last the most recently created object. The
    // keywords are consumed centrally in `try_selection_keyword`, so they
    // also work typed — these buttons just surface them.
    fn options(&self) -> Vec<CmdOption> {
        if self.commit_on_enter {
            vec![CmdOption::new("Previous", "P"), CmdOption::new("Last", "L")]
        } else {
            Vec::new()
        }
    }

    fn on_selection_complete(&mut self, handles: Vec<Handle>) -> CmdResult {
        if self.commit_on_enter {
            // Keep gathering: remember the running set and update the prompt.
            // The command is applied when the user presses Enter / right-clicks.
            self.handles = handles;
            return CmdResult::NeedPoint;
        }
        // Single-object variant: apply on the first non-empty selection.
        if handles.is_empty() {
            return CmdResult::NeedPoint;
        }
        CmdResult::Relaunch(std::mem::take(&mut self.pending_cmd), handles)
    }

    fn on_point(&mut self, _pt: DVec3) -> CmdResult {
        CmdResult::NeedPoint
    }

    // Enter / right-click ends the gather phase and fires the pending command
    // with the accumulated selection. Nothing selected → cancel.
    fn on_enter(&mut self) -> CmdResult {
        if !self.commit_on_enter || self.handles.is_empty() {
            return CmdResult::Cancel;
        }
        CmdResult::Relaunch(
            std::mem::take(&mut self.pending_cmd),
            std::mem::take(&mut self.handles),
        )
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }
    fn on_hover_entity(&mut self, _handle: Handle, _pt: DVec3) -> Vec<WireModel> {
        vec![]
    }
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["ARRAY", "ARRAYPATH", "ARRAYPOLAR", "ARRAYRECT", "BLOCK", "COPY", "COPYCLIP", "CUTCLIP", "ERASE", "EXPLODE", "GROUP", "LAYFRZ", "LAYLCK", "LAYMCUR", "LAYOFF", "LAYULK", "MIRROR", "MOVE", "ROTATE", "SCALE", "STRETCH"] });  // SelectObjectsCommand
