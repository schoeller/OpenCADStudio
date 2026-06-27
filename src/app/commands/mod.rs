use super::{Message, OpenCADStudio};
use crate::command::CadCommand;
use crate::scene::Scene;
use iced::Task;
use std::path::PathBuf;

mod blocks;
mod dim;
mod display;
mod draw;
mod fileops;
mod inquiry;
mod layerprops;
mod layers;
mod styleprops;
mod view;

// `DrawOrderRefCommand` lives in the `view` family file but is referenced by
// path (`commands::DrawOrderRefCommand`) from `update.rs`, so re-export it at
// the module root to keep that path valid.
pub(crate) use view::DrawOrderRefCommand;

impl OpenCADStudio {
    /// First `"{prefix}{n}"` (n ≥ 1) not already used by a block record in the
    /// active drawing. Used to auto-name a paste-as-block definition.
    fn unique_block_name(&self, prefix: &str) -> String {
        let i = self.active_tab;
        let mut n = 1;
        loop {
            let name = format!("{prefix}{n}");
            if self.tabs[i].scene.document.block_records.get(&name).is_none() {
                return name;
            }
            n += 1;
        }
    }

    pub(super) fn dispatch_command(&mut self, cmd: &str) -> Task<Message> {
        self.dispatch_command_inner(cmd, false)
    }

    /// Dispatch a verb typed at the interactive command line, falling back to
    /// the closest autocomplete suggestion when the verb matches no command
    /// family. Lets a partial command run on Enter (`BAC` → `BACKGROUND`),
    /// the standard DWG command-line behavior. The fallback only fires for
    /// genuinely unknown verbs, so complete aliases that resolve through a
    /// dispatch family (`LT`, `ZO`, …) still run as typed. Programmatic
    /// callers (ribbon, plugins, headless automation) use `dispatch_command`
    /// and never get silent substitution.
    pub(super) fn dispatch_command_or_suggest(&mut self, cmd: &str) -> Task<Message> {
        self.dispatch_command_inner(cmd, true)
    }

    fn dispatch_command_inner(&mut self, cmd: &str, allow_suggest: bool) -> Task<Message> {
        let i = self.active_tab;
        // Starting a command closes any open ribbon dropdown (e.g. a style
        // combo left open) so it does not stay stuck behind the new tool.
        self.ribbon.close_dropdown();
        // Cancel any running command before starting a new one.
        if self.tabs[i].active_cmd.is_some() {
            self.tabs[i].scene.clear_preview_wire();
            self.tabs[i].active_cmd = None;
        }
        // Starting any command leaves interactive PAN mode (the PAN arm below
        // re-enables it).
        self.tabs[i].pan_mode = false;
        // Reset the last committed point so the first click of the new command
        // is not constrained by ortho/polar relative to a previous command's endpoint.
        self.last_point = None;
        // A fresh command starts at the polar/cartesian default — clear
        // any `,`-driven reshape from a previous command (#35).
        self.dyn_user_reshaped = false;

        if let Some(path_str) = cmd.strip_prefix("OPEN_RECENT:") {
            let path = PathBuf::from(path_str);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            return Task::done(Message::OpenPathPicked(Some((path, size))));
        }

        // The Start (welcome) tab has no drawing to act on, so a drawing
        // command would silently do nothing. Allow only the commands that
        // make sense there (create / open a document, or quit) and tell the
        // user otherwise instead of running a no-op. See #96.
        if self.tabs[i].is_start
            && !matches!(
                cmd,
                "NEW" | "OPEN" | "EXIT" | "QUIT" | "REPORT" | "CHANGELOG" | "ABOUT"
                    | "PLUGINS" | "PLUGINMANAGER" | "DONATE" | "WEBVERSION"
            )
        {
            self.command_line
                .push_info("No drawing open. Use NEW or OPEN to start a drawing.");
            return Task::none();
        }

        if crate::plugin::try_dispatch(self, i, cmd) {
            // try_dispatch returns true for both finished commands and interactive
            // commands that it just installed. If no command is now active, the
            // tool was a one-shot and we must turn the ribbon highlight off here —
            // normally apply_cmd_result does that, but plugin dispatch can return
            // without producing a CmdResult.
            self.command_line.record_recent(cmd);
            if self.tabs[i].active_cmd.is_none() {
                self.ribbon.deactivate_tool();
            }
            return Task::none();
        }

        // Command families are dispatched in source order (see
        // `dispatch_families`); the first whose `match` arm matches handles it.
        if let Some(t) = self.dispatch_families(cmd, i) {
            // A command resolved — record it for the right-click Repeat menu so
            // commands from every source (command line, ribbon, context menu,
            // shortcuts) appear there, not only typed ones. Recorded after
            // resolution so a partial verb completed via the suggestion
            // fallback (`BAC`) stores the real command (`BACKGROUND`).
            self.command_line.record_recent(cmd);
            return t;
        }

        // No family matched. From the interactive command line, run the
        // closest autocomplete suggestion instead of erroring, so a partial
        // command completes on Enter (`BAC` → `BACKGROUND`). The verb's own
        // input was already cleared, so rank against `cmd` directly.
        if allow_suggest {
            if let Some(top) = crate::ui::command_line::ranked_matches(cmd).first().copied() {
                if !top.eq_ignore_ascii_case(cmd) {
                    return self.dispatch_command_inner(top, false);
                }
            }
        }
        self.command_line
            .push_error(&format!("Unknown command: {cmd}"));
        self.finish_dispatch(cmd)
    }

    /// Try each command family in source order, returning the first that
    /// handles `cmd`, or `None` when none match. Each family returns
    /// `Some(task)` for an arm it owns (early-returning or falling through to
    /// `finish_dispatch`), or `None` to defer to the next — equivalent to one
    /// sequential `match` over all arms.
    fn dispatch_families(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        if let Some(t) = self.dispatch_fileops(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_layers(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_blocks(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_draw(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_dim(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_inquiry(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_view(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_layerprops(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_styleprops(cmd, i) {
            return Some(t);
        }
        if let Some(t) = self.dispatch_display(cmd, i) {
            return Some(t);
        }
        None
    }

    /// Shared tail run after a `dispatch_*` family handler whose matched arm
    /// did not early-return. Focuses the command line whenever a command just
    /// became active.
    fn finish_dispatch(&mut self, cmd: &str) -> Task<Message> {
        let i = self.active_tab;
        if self.tabs[i].active_cmd.is_some() {
            self.tabs[i].last_cmd = Some(cmd.to_string());
            self.focus_cmd_input()
        } else {
            Task::none()
        }
    }
}


// ── Autocomplete registry — one-shot commands ──────────────────────────────
// These commands dispatch a single action (file ops, view, layer/style
// managers, undo/redo, …) rather than installing an interactive `CadCommand`,
// so they have no module of their own to register from. They are surfaced for
// command-line autocomplete here. Internal dispatch tokens that the user never
// types (REFEDIT_BEGIN, REFCLOSE_SAVE, REFCLOSE_DISCARD) are intentionally
// excluded.
inventory::submit!(crate::command::CommandRegistration {
    names: &[
        "3DORBIT", "3O", "ABOUT", "ATTDISP", "ATTEXT", "BACKGROUND", "CDIMSTY", "CELTSCALE",
        "CHANGELOG", "CHPROP", "CLAYER", "CLEAR", "CLR", "COLORSCHEME", "COUNT", "DATAEXTRACTION",
        "DE", "DESELALL", "DESELECT", "DIMSTYLE", "DONATE", "DRAWORDER", "DWGPROP", "DWGPROPS",
        "EATTEXT", "EXIT", "EXPORT", "EXPORTSTEP", "EXPORTSTL", "FILETAB", "FIND", "FLATTEN",
        "HELP", "HIDEOBJECTS", "IM", "IMAGE", "IMAGEATTACH", "IMPORTOBJ", "ISOLATEOBJECTS", "LA",
        "LAYER", "LAYERS", "LAYISO", "LAYON", "LAYOUTMANAGER", "LAYOUTPANEL", "LAYOUTTAB", "LAYTHW",
        "LAYUNISO", "LI", "LINETYPE", "LIST", "LTSCALE", "LWDISPLAY", "MASSPROP", "MLEADERSTYLE",
        "MLSTYLE", "MS", "MSPACE", "NAVVCUBE", "NEW", "OBJIMPORT", "OPEN", "ORTHO",
        "P", "PAN", "PAGESETUP", "PERF", "PERSP", "PLOT", "PLOTSTYLE", "PLOTSTYLEEDITOR",
        "PLOTSTYLEPANEL", "PR",
        "PRINT", "PROPERTIES", "PROPS", "PSPACE", "PURGE", "QS", "QSAVE", "QSELECT",
        "QUIT", "REDO", "REDRAW", "REDRWALL", "REGEN", "REGENALL", "RENAME", "REPORT", "RMBENTER",
        "SA", "SAVE", "SAVEAS", "SCALETEXT", "SELECTALL", "SELECTSIMILAR", "SELSIM", "SHEETSET",
        "SHORTCUTS", "SOLID", "SSM", "STEPOUT", "STLOUT", "STPOUT", "STYLE", "STYLESMANAGER",
        "TABLESTYLE", "TOOLPALETTES", "TP", "TS", "U", "UCS", "UCSICON", "UNDERLAY",
        "UNDO", "UNISOLATEOBJECTS", "USERI", "USERR", "VIEW", "VPORTS", "VS", "VW",
        "WB", "WBLOCK", "WEBVERSION", "WIREFRAME", "XA", "XATTACH", "XDATA", "XR",
        "XREF", "XRELOAD", "ZOOM", "ZS",
    ]
});
