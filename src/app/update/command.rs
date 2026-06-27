//! `command` arms and helpers, split out of the original `update.rs` (#mechanical decomposition).

#![allow(unused_imports)]
use super::util::*;
use super::{format_size, VIEWCUBE_HIT_SIZE};
use crate::app::helpers::{
    ortho_constrain, parse_coord, polar_constrain_near, ucs_rotate_vec, ucs_to_wcs, ucs_z_axis,
    CoordKind,
};
use crate::app::{Message, OpenCADStudio, POLY_START_DELAY_MS};
use crate::modules::ModuleEvent;
use crate::scene::pick::grip::{find_hit_grip, find_hit_grip_paper, find_hit_grip_rte, GripEdit};
use crate::scene::model::object::GripApply;
use crate::scene::{
    self, hover_id, CubeRegion, Scene, VIEWCUBE_DRAW_PX, VIEWCUBE_PAD, VIEWCUBE_PX,
};
use crate::ui::PropertiesPanel;
use acadrust::types::Color as AcadColor;
use acadrust::{EntityType as AcadEntityType, Handle};
use iced::time::Instant;
use iced::{mouse, Point, Task};


impl OpenCADStudio {
pub(super) fn on_tab_close(&mut self, idx: usize) -> Task<Message> {
                // Start tab is fixed — close requests on it are no-ops.
                if self.tabs.get(idx).map_or(false, |t| t.is_start) {
                    return Task::none();
                }
                if self.tabs.get(idx).map_or(false, |t| t.dirty) {
                    self.pending_close = Some(crate::app::PendingClose::Tab(idx));
                    return self.open_unsaved_dialog_window();
                }
                // Only-tab case: when the lone non-start tab closes, fall
                // back to the Start tab if it exists; otherwise spawn a
                // fresh blank drawing (legacy behaviour).
                if self.tabs.len() == 1 {
                    self.tab_counter += 1;
                    self.tabs[0] = crate::app::document::DocumentTab::new_drawing(self.tab_counter);
                    self.active_tab = 0;
                    self.apply_bg_default(0);
                } else {
                    self.tabs.remove(idx);
                    if self.active_tab >= self.tabs.len() {
                        self.active_tab = self.tabs.len() - 1;
                    }
                }
                // The active tab is now either a brand-new blank or a
                // different existing tab; in both cases the ribbon needs
                // to track that doc's defaults / selection. #21.
                self.sync_ribbon_layers();
                self.sync_ribbon_styles();
                self.sync_ribbon_from_selection();
                Task::none()
    }

    pub(super) fn on_command_append_char(&mut self, s: String) -> Task<Message> {
                // While the MText preview is up, typed glyphs edit it directly.
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    if s.chars().all(|c| !c.is_control()) {
                        self.mtext_type(&s);
                    }
                    return Task::none();
                }
                // Filter out control characters — only push the typed
                // glyph(s). `Tab`, etc. arrive as Named keys, not here.
                if s.chars().all(|c| !c.is_control()) {
                    let i = self.active_tab;
                    // `,` is the coordinate separator in dynamic input,
                    // not a decimal point: typing it locks the current
                    // field's buffer and advances to the next coordinate,
                    // reshaping the field set when going polar → cartesian
                    // (Distance → X, Y) or 2-D → 3-D (X, Y → X, Y, Z).
                    // See #35.
                    if s == "," && self.dyn_input && !self.tabs[i].dyn_fields.is_empty() {
                        self.dyn_comma_advance();
                        self.command_line.autocomplete_cursor = None;
                        return self.focus_cmd_input();
                    }
                    // While dynamic input is showing fields, numeric and
                    // expression glyphs edit the focused field instead of
                    // the command line. Letters still go to the command line
                    // so command-option keywords keep working.
                    let dyn_field_char = !s.is_empty()
                        && s.chars().all(|c| {
                            c.is_ascii_digit()
                                || matches!(c, '.' | '-' | '+' | '*' | '/' | '^' | '%' | '(' | ')')
                        });
                    if dyn_field_char && self.dyn_input && !self.tabs[i].dyn_fields.is_empty() {
                        let a = self.tabs[i]
                            .dyn_active
                            .min(self.tabs[i].dyn_fields.len() - 1);
                        self.tabs[i].dyn_fields[a]
                            .buffer
                            .get_or_insert_with(String::new)
                            .push_str(&s);
                    } else {
                        self.command_line.input.push_str(&s);
                    }
                }
                self.command_line.autocomplete_cursor = None;
                self.focus_cmd_input()
    }

    pub(super) fn on_command_backspace(&mut self) -> Task<Message> {
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    self.mtext_backspace();
                    return Task::none();
                }
                let i = self.active_tab;
                // Backspace edits the focused dynamic-input field first;
                // emptying it unlocks the field (back to cursor tracking).
                if self.dyn_input && !self.tabs[i].dyn_fields.is_empty() {
                    let a = self.tabs[i]
                        .dyn_active
                        .min(self.tabs[i].dyn_fields.len() - 1);
                    if let Some(buf) = self.tabs[i].dyn_fields[a].buffer.as_mut() {
                        buf.pop();
                        if buf.is_empty() {
                            self.tabs[i].dyn_fields[a].buffer = None;
                        }
                        return self.focus_cmd_input();
                    }
                }
                self.command_line.input.pop();
                self.command_line.autocomplete_cursor = None;
                self.focus_cmd_input()
    }

    pub(super) fn on_command_submit(&mut self) -> Task<Message> {
                // Submitting a command implicitly dismisses the history
                // dropdown so the dispatched command's new prompt is
                // immediately visible on the overlay.
                self.command_line.close_history();
                // Grip-menu value prompt — consume the typed number and
                // route it through `apply_grip_menu_value`.
                if let Some(pending) = self.grip_pending.take() {
                    let raw = crate::app::expr_eval::eval_to_string(self.command_line.input.trim());
                    self.command_line.input.clear();
                    let Ok(v) = raw.parse::<f64>() else {
                        self.command_line.push_error(&format!(
                            "{}: expected a number, got \"{raw}\"",
                            pending.label
                        ));
                        return Task::none();
                    };
                    let i = self.active_tab;
                    use crate::entities::traits::EntityTypeOps;
                    self.push_undo_snapshot(i, pending.label);
                    if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(pending.handle)
                    {
                        entity.apply_grip_menu_value(pending.grip_id, pending.action, v);
                    }
                    self.tabs[i].scene.bump_geometry();
                    self.tabs[i].dirty = true;
                    self.refresh_selected_grips();
                    self.refresh_properties();
                    return Task::none();
                }
                // Interactive VPORTS: the entry after a bare `VPORTS` is the
                // tiled configuration. Empty input defaults to SINGLE.
                if self.awaiting_vports {
                    self.awaiting_vports = false;
                    let cfg = self.command_line.input.trim().to_string();
                    self.command_line.input.clear();
                    let cfg = if cfg.is_empty() {
                        "SINGLE".to_string()
                    } else {
                        cfg
                    };
                    return self.dispatch_command(&format!("VPORTS {cfg}"));
                }
                // If the user navigated the autocomplete list with the
                // arrow keys, Enter dispatches the highlighted command
                // rather than the partial text actually in the buffer.
                let i_tab = self.active_tab;
                if self.tabs[i_tab].active_cmd.is_none() {
                    if let Some(picked) = self.command_line.selected_suggestion() {
                        let cmd = picked.to_string();
                        self.command_line.input.clear();
                        self.command_line.autocomplete_cursor = None;
                        return self.dispatch_command(&cmd);
                    }
                }
                let i = self.active_tab;
                // A whole multi-token command line (`UCS Z 90`, `LINE 0,0
                // 10,10`, `PDMODE 3`) — typable now that Space is literal — is
                // processed as one unit: feed the tokens to a running command,
                // or start a new one through the shared runner that the headless
                // automation feeder uses too.
                {
                    // Skip token-splitting when the active command collects
                    // free-form text with spaces (TEXT / MTEXT / a name) — it
                    // wants the whole line as one input.
                    let wants_spaces = self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.wants_text_input() && c.wants_text_with_spaces())
                        .unwrap_or(false);
                    let raw = self.command_line.input.clone();
                    let toks: Vec<String> = raw.split_whitespace().map(String::from).collect();
                    if toks.len() > 1 && !wants_spaces {
                        self.command_line.input.clear();
                        if self.tabs[i].active_cmd.is_some() {
                            for tok in &toks {
                                if self.tabs[i].active_cmd.is_none() {
                                    break;
                                }
                                self.feed_active_cmd(tok);
                            }
                            return Task::none();
                        }
                        return self.run_command_line(&raw);
                    }
                }
                // With the command line empty, a typed dynamic-input value
                // commits as a point pick instead of an empty submit.
                if self.tabs[i].active_cmd.is_some() && self.command_line.input.trim().is_empty() {
                    if let Some(task) = self.try_dyn_commit() {
                        return task;
                    }
                }
                if self.tabs[i].active_cmd.is_some() {
                    let text = crate::app::expr_eval::eval_to_string(self.command_line.input.trim());
                    self.command_line.input.clear();

                    // Offer the typed text to the command's option handler
                    // first (keywords like PLINE's A/L/C, a radius, …). If it
                    // consumes the text we're done; if it returns None the
                    // text falls through to the Enter / coordinate handling
                    // below, so a bare Enter still terminates and typed points
                    // still work when dynamic input is off. See #97.
                    if self.tabs[i]
                        .active_cmd
                        .as_ref()
                        .map(|c| c.wants_text_input())
                        .unwrap_or(false)
                    {
                        self.push_ucs_to_cmd(i);
                        if let Some(result) = self.tabs[i]
                            .active_cmd
                            .as_mut()
                            .and_then(|c| c.on_text_input(&text))
                        {
                            return self.apply_cmd_result(result);
                        }
                    }

                    if text.is_empty() {
                        let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_enter());
                        if let Some(r) = result {
                            return self.apply_cmd_result(r);
                        }
                        return Task::none();
                    }

                    // OTRACK: while aligned to a tracking ray, a bare distance
                    // places the point along the ray from the tracking point
                    // (issue #69).
                    if let Some((base, dir)) = self.otrack_active {
                        if let Some(dist) = crate::app::expr_eval::eval_number(text.trim()) {
                            let pt = base + dir * dist as f32;
                            self.last_point = Some(pt);
                            self.dyn_user_reshaped = false;
                            self.sync_dyn_fields();
                            self.reset_tracking_after_point();
                            self.push_ucs_to_cmd(i);
                            let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_point(pt.as_dvec3()));
                            if let Some(r) = result {
                                let task = self.apply_cmd_result(r);
                                self.refresh_active_cmd_preview(i);
                                return task;
                            }
                            return Task::none();
                        }
                    }

                    if let Some((coord, kind)) = parse_coord(&text) {
                        // Command-line coordinates are absolute by default,
                        // independent of DYN: `@` forces relative, `#` forces
                        // absolute, and a bare value is absolute. (Relative-by-
                        // default lives in the DYN tooltip path — see
                        // `dyn_resolve_point` — matching AutoCAD, where the
                        // command line stays absolute regardless of DYN.)
                        let want_relative = matches!(kind, CoordKind::Relative);
                        let ucs = self.tabs[i].active_ucs.clone();
                        let wcs_pt = match (want_relative, self.last_point) {
                            (true, Some(base)) => {
                                // Offset from the last point, rotated by the
                                // UCS axes (no origin translation).
                                let offset = match &ucs {
                                    Some(u) => ucs_rotate_vec(coord, u),
                                    None => coord,
                                };
                                base + offset
                            }
                            _ => {
                                // Absolute: typed coordinates are in active UCS.
                                match &ucs {
                                    Some(u) => ucs_to_wcs(coord, u),
                                    None => coord,
                                }
                            }
                        };
                        self.last_point = Some(wcs_pt);
                        self.dyn_user_reshaped = false;
                        self.sync_dyn_fields();
                        self.reset_tracking_after_point();
                        self.push_ucs_to_cmd(i);
                        let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_point(wcs_pt.as_dvec3()));
                        if let Some(r) = result {
                            let task = self.apply_cmd_result(r);
                            // The rubber-band preview that the command
                            // last published reflects the *previous*
                            // last_point — a typed coordinate doesn't
                            // fire a mouse-move, so re-run the preview
                            // hook now using the current cursor world
                            // pos so the next segment immediately starts
                            // from the just-committed point. See #32.
                            self.refresh_active_cmd_preview(i);
                            return task;
                        }
                        return Task::none();
                    }

                    self.push_ucs_to_cmd(i);
                    if let Some(result) = self.tabs[i]
                        .active_cmd
                        .as_mut()
                        .and_then(|c| c.on_text_input(&text))
                    {
                        return self.apply_cmd_result(result);
                    }

                    self.command_line.push_error(&format!(
                        "Expected coordinates (x,y) or a number, got: \"{text}\""
                    ));
                    return self.focus_cmd_input();
                }
                if let Some(cmd) = self.command_line.submit() {
                    return self.dispatch_command_or_suggest(&cmd);
                }
                // Empty Enter / Space with no active command repeats the
                // last dispatched command — same shortcut `CommandFinalize`
                // already implements, mirrored here so the trailing-space
                // submit path goes through it too.
                if let Some(cmd) = self.tabs[i].last_cmd.clone() {
                    return self.dispatch_command(&cmd);
                }
                Task::none()
    }

    pub(super) fn on_command_finalize(&mut self) -> Task<Message> {
                // In the MText preview, Enter inserts a line break.
                if self.mtext_editor.as_ref().is_some_and(|e| e.show_preview) {
                    self.mtext_type("\n");
                    return Task::none();
                }
                // Grip popup open → Enter commits the highlighted item.
                if self.grip_popup.is_some() {
                    let idx = self.grip_popup.as_ref().map(|p| p.selected).unwrap_or(0);
                    return Task::done(Message::GripMenuPick(idx));
                }
                // Any typed command-line text must be submitted rather than
                // finalising. The focused-input Enter routes through
                // CommandSubmit, but when the field isn't focused — e.g. at
                // startup before the window grabs focus, or a command started
                // from the ribbon — its Enter arrives here. Forward a non-empty
                // buffer to the same submit path so typing a command name (or
                // an option keyword like "R") and pressing Enter works without
                // first clicking into the command line (issue #99).
                if !self.command_line.input.trim().is_empty() {
                    return self.update(Message::CommandSubmit);
                }
                // A typed dynamic-input value commits as a point pick
                // before the plain-Enter (on_enter) path runs.
                if let Some(task) = self.try_dyn_commit() {
                    return task;
                }
                let i = self.active_tab;
                if self.tabs[i].active_cmd.is_some() {
                    let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_enter());
                    if let Some(r) = result {
                        return self.apply_cmd_result(r);
                    }
                    Task::none()
                } else if let Some(cmd) = self.tabs[i].last_cmd.clone() {
                    self.dispatch_command(&cmd)
                } else {
                    Task::none()
                }
    }

    pub(super) fn on_command_escape(&mut self) -> Task<Message> {
                // Open MText editor swallows Escape (cancel without committing).
                if self.mtext_editor.is_some() {
                    self.mtext_cancel();
                    return self.post_editor_closed(false);
                }
                // The in-place TEXT editor likewise cancels on Escape.
                if self.text_inline.is_some() {
                    self.text_inline_cancel();
                    return self.post_editor_closed(false);
                }
                // Esc cancels an armed pane move.
                if self.pane_move_from.take().is_some() {
                    return Task::none();
                }
                // UCS icon: Esc ends any grip drag and clears the selection
                // (only when no command owns Escape).
                if self.tabs[self.active_tab].active_cmd.is_none()
                    && (self.ucs_grip_drag.is_some() || self.ucs_icon_selected)
                {
                    let i = self.active_tab;
                    self.ucs_grip_drag = None;
                    self.ucs_icon_selected = false;
                    self.ucs_icon_hover = false;
                    self.tabs[i].snap_result = None;
                    return Task::none();
                }
                // Leave interactive PAN mode (and end any in-flight pan drag).
                if self.tabs[self.active_tab].pan_mode {
                    let i = self.active_tab;
                    self.tabs[i].pan_mode = false;
                    {
                        let mut sel = self.tabs[i].scene.selection.borrow_mut();
                        sel.middle_down = false;
                        sel.middle_last_pos = None;
                    }
                    self.command_line.push_output("PAN ended.");
                    return Task::none();
                }
                // Grip popup intercepts Escape — dismisses the menu
                // without doing anything else.
                if self.grip_popup.take().is_some() {
                    self.grip_hover = None;
                    return Task::none();
                }
                if self.visibility_popup.take().is_some() {
                    return Task::none();
                }
                if self.grip_pending.take().is_some() {
                    self.command_line.input.clear();
                    return Task::none();
                }
                // A hot grip (click-move-click placement in progress) ends on
                // Escape, leaving the entity at its last previewed position.
                if self.tabs[self.active_tab].active_grip.take().is_some() {
                    // An Add-Leader arrow being placed: Esc removes it again.
                    if let Some((h, gid)) = self.grip_add_provisional.take() {
                        let i = self.active_tab;
                        use crate::entities::traits::EntityTypeOps;
                        if let Some(e) = self.tabs[i].scene.document.get_entity_mut(h) {
                            e.apply_grip_menu(
                                gid,
                                crate::scene::model::object::GripMenuAction::RemoveLeader,
                            );
                        }
                        self.tabs[i].scene.bump_geometry();
                        self.refresh_selected_grips();
                    }
                    // Cancel an in-progress grip drag: restore the edited
                    // entity from its pre-drag backup, un-hide it, re-tessellate
                    // once, and drop the preview.
                    if let Some(h) = self.grip_preview_handle.take() {
                        let i = self.active_tab;
                        if let Some(orig) = self.grip_original.take() {
                            if let Some(e) = self.tabs[i].scene.document.get_entity_mut(h) {
                                *e = orig;
                            }
                        }
                        self.tabs[i].scene.hidden.remove(&h);
                        self.tabs[i].scene.clear_preview_wire();
                        // Geometry restored to the backup — re-tessellate just it.
                        self.tabs[i].scene.mark_entity_dirty(h);
                        self.tabs[i].scene.bump_geometry_no_blocks();
                        self.refresh_selected_grips();
                    }
                    self.tabs[self.active_tab].snap_result = None;
                    self.refresh_properties();
                    return Task::none();
                }
                // Cancel layout rename / context menus first, then fall through.
                let i_e = self.active_tab;
                if self.qselect.take().is_some() {
                    return Task::none();
                }
                {
                    let mut sel = self.tabs[i_e].scene.selection.borrow_mut();
                    if sel.context_menu.is_some() {
                        sel.context_menu = None;
                        return Task::none();
                    }
                }
                if self.layout_rename_state.take().is_some()
                    || self.layout_context_menu.take().is_some()
                {
                    return Task::none();
                }
                // Typed text on the command line cancels first — one
                // Esc empties the buffer, a second Esc then escalates
                // to whatever the current mode would otherwise do
                // (cancel command / exit viewport / deselect).
                if !self.command_line.input.is_empty() {
                    self.command_line.input.clear();
                    self.command_line.autocomplete_cursor = None;
                    self.command_line.close_history();
                    return Task::none();
                }
                let i = self.active_tab;
                if self.tabs[i].active_cmd.is_some() {
                    let result = self.tabs[i].active_cmd.as_mut().map(|c| c.on_escape());
                    if let Some(r) = result {
                        return self.apply_cmd_result(r);
                    }
                } else if self.tabs[i].scene.active_viewport.is_some() {
                    // ESC while in MSPACE → exit back to paper space.
                    return Task::done(Message::ExitViewport);
                } else {
                    self.tabs[i].scene.deselect_all();
                    self.refresh_properties();
                    let mut sel = self.tabs[i].scene.selection.borrow_mut();
                    sel.box_anchor = None;
                    sel.box_current = None;
                    sel.box_crossing = false;
                }
                Task::none()
    }

    pub(super) fn on_layer_toggle_vp_freeze(&mut self, layer_idx: usize, vp_col_idx: usize) -> Task<Message> {
                let i = self.active_tab;
                let vp_handle = self.tabs[i]
                    .layers
                    .vp_cols
                    .get(vp_col_idx)
                    .map(|c| c.handle);
                let layer_name = self.tabs[i]
                    .layers
                    .layers
                    .get(layer_idx)
                    .map(|l| l.name.clone());

                if let (Some(vp_handle), Some(layer_name)) = (vp_handle, layer_name) {
                    // Get the layer handle from the document
                    if let Some(doc_layer) = self.tabs[i].scene.document.layers.get(&layer_name) {
                        let layer_handle = doc_layer.handle;
                        self.push_undo_snapshot(i, "VPLAYER");

                        // Toggle frozen_layers on the viewport entity
                        for e in self.tabs[i].scene.document.entities_mut() {
                            if let acadrust::EntityType::Viewport(vp) = e {
                                if vp.common.handle == vp_handle {
                                    if vp.frozen_layers.contains(&layer_handle) {
                                        vp.frozen_layers.retain(|h| h != &layer_handle);
                                    } else {
                                        vp.frozen_layers.push(layer_handle);
                                    }
                                    break;
                                }
                            }
                        }

                        // Re-sync layer panel with updated VP info
                        let vp_info = self.tabs[i].scene.viewport_list();
                        let doc_layers = self.tabs[i].scene.document.layers.clone();
                        self.tabs[i]
                            .layers
                            .sync_with_viewports(&doc_layers, vp_info);
                        self.tabs[i].scene.bump_geometry();
                        self.tabs[i].dirty = true;
                    }
                }
                Task::none()
    }

    pub(super) fn on_layer_new(&mut self) -> Task<Message> {
                let i = self.active_tab;
                let mut n = 1;
                let new_name = loop {
                    let candidate = format!("Layer{}", n);
                    if !self.tabs[i].scene.document.layers.contains(&candidate) {
                        break candidate;
                    }
                    n += 1;
                };
                self.push_undo_snapshot(i, "LAYER NEW");
                use acadrust::tables::layer::Layer as DocLayer;
                // A layer needs a real handle or it is dropped on a DWG save
                // (the format is handle-based; issue #67).
                let mut dl = DocLayer::new(&new_name);
                // `allocate_handle` advances the seed so the layer gets a
                // unique handle; the non-advancing `next_handle` getter hands
                // out the same value twice and the later object overwrites it.
                dl.handle = self.tabs[i].scene.document.allocate_handle();
                let _ = self.tabs[i].scene.document.layers.add(dl);
                self.tabs[i].dirty = true;
                let doc_layers = self.tabs[i].scene.document.layers.clone();
                let vp_info = self.tabs[i].scene.viewport_list();
                self.tabs[i]
                    .layers
                    .sync_with_viewports(&doc_layers, vp_info);
                let new_idx = self.tabs[i]
                    .layers
                    .layers
                    .iter()
                    .position(|l| l.name == new_name);
                if let Some(idx) = new_idx {
                    self.tabs[i].layers.selected = Some(idx);
                    self.tabs[i].layers.editing = Some(idx);
                    self.tabs[i].layers.edit_buf = new_name.clone();
                }
                self.sync_ribbon_layers();
                Task::none()
    }

    pub(super) fn on_layer_delete(&mut self) -> Task<Message> {
                let i = self.active_tab;
                if let Some(idx) = self.tabs[i].layers.selected {
                    let name = self.tabs[i]
                        .layers
                        .layers
                        .get(idx)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    if name == "0" {
                        return Task::none();
                    }
                    self.push_undo_snapshot(i, "LAYER DELETE");
                    self.tabs[i].scene.document.layers.remove(&name);
                    self.tabs[i].dirty = true;
                    let doc_layers = self.tabs[i].scene.document.layers.clone();
                    let vp_info = self.tabs[i].scene.viewport_list();
                    self.tabs[i]
                        .layers
                        .sync_with_viewports(&doc_layers, vp_info);
                    self.tabs[i].layers.selected = None;
                    self.sync_ribbon_layers();
                }
                Task::none()
    }

    pub(super) fn on_layer_set_current(&mut self) -> Task<Message> {
                let i = self.active_tab;
                if let Some(idx) = self.tabs[i].layers.selected {
                    if let Some(layer) = self.tabs[i].layers.layers.get(idx) {
                        let name = layer.name.clone();
                        // Mirror the change into the document header (CLAYER) too,
                        // not just the per-tab default. Otherwise the no-selection
                        // ribbon refresh (e.g. after Esc) re-reads the stale header
                        // layer and the dropdown snaps back to it. See #93.
                        let handle = self.tabs[i]
                            .scene
                            .document
                            .layers
                            .get(&name)
                            .map(|l| l.handle)
                            .unwrap_or(acadrust::types::Handle::NULL);
                        self.tabs[i].scene.document.header.current_layer_name = name.clone();
                        self.tabs[i].scene.document.header.current_layer_handle = handle;
                        self.tabs[i].active_layer = name.clone();
                        self.tabs[i].layers.current_layer = name.clone();
                        self.tabs[i].dirty = true;
                        self.ribbon.active_layer = name;
                    }
                }
                Task::none()
    }

    pub(super) fn on_layer_rename_commit(&mut self) -> Task<Message> {
                let i = self.active_tab;
                let editing_idx = self.tabs[i].layers.editing.take();
                if let Some(idx) = editing_idx {
                    let new_name = self.tabs[i].layers.edit_buf.trim().to_string();
                    let old_name = self.tabs[i]
                        .layers
                        .layers
                        .get(idx)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    if !new_name.is_empty()
                        && new_name != old_name
                        && !self.tabs[i].scene.document.layers.contains(&new_name)
                    {
                        self.push_undo_snapshot(i, "LAYER RENAME");
                        // Keep the whole record (handle, color, linetype,
                        // lineweight, flags) and only change the name, so the
                        // renamed layer still has a valid handle and survives a
                        // DWG save (issue #67).
                        if let Some(mut nl) =
                            self.tabs[i].scene.document.layers.get(&old_name).cloned()
                        {
                            nl.name = new_name.clone();
                            if !nl.handle.is_valid() {
                                nl.handle = self.tabs[i].scene.document.allocate_handle();
                            }
                            let _ = self.tabs[i].scene.document.layers.add(nl);
                        }
                        self.tabs[i].scene.document.layers.remove(&old_name);
                        for e in self.tabs[i].scene.document.entities_mut() {
                            if e.as_entity().layer() == old_name {
                                e.as_entity_mut().set_layer(new_name.clone());
                            }
                        }
                        self.tabs[i].dirty = true;
                    }
                    let doc_layers = self.tabs[i].scene.document.layers.clone();
                    let vp_info = self.tabs[i].scene.viewport_list();
                    self.tabs[i]
                        .layers
                        .sync_with_viewports(&doc_layers, vp_info);
                    self.tabs[i].layers.edit_buf.clear();
                    self.sync_ribbon_layers();
                }
                Task::none()
    }

    pub(super) fn on_grip_menu_pick(&mut self, idx: usize) -> Task<Message> {
                let i = self.active_tab;
                let Some(popup) = self.grip_popup.take() else {
                    return Task::none();
                };
                self.grip_hover = None;
                let Some(item) = popup.items.get(idx).cloned() else {
                    return Task::none();
                };
                use crate::entities::traits::EntityTypeOps;
                use crate::scene::model::object::GripMenuAction;
                if matches!(
                    item.action,
                    GripMenuAction::Stretch
                        | GripMenuAction::MoveWithLeader
                        | GripMenuAction::MoveIndependent
                ) {
                    // Stretch / Move = grab this grip. Engage it so the next
                    // click places it (click-move-click) — same as picking the
                    // grip directly in the viewport. Without this the menu just
                    // closed and the grip never became hot (issue #48).
                    if let Some(g) = self.tabs[i]
                        .selected_grips
                        .iter()
                        .find(|g| g.id == popup.grip_id)
                    {
                        // "Move with Leader" drags the whole multileader; the
                        // others move just the picked grip.
                        let (grip_id, is_translate) =
                            if matches!(item.action, GripMenuAction::MoveWithLeader) {
                                (crate::entities::multileader::MOVE_ALL_GRIP, true)
                            } else {
                                (popup.grip_id, g.is_midpoint)
                            };
                        self.tabs[i].active_grip = Some(GripEdit {
                            handle: popup.handle,
                            grip_id,
                            is_translate,
                            origin_world: g.world,
                            last_world: g.world,
                        });
                    }
                    return Task::none();
                }
                // Actions that need a follow-up number stash a pending
                // state + prompt; the next typed value drives
                // `apply_grip_menu_value`.
                let prompt = self.tabs[i]
                    .scene
                    .document
                    .get_entity(popup.handle)
                    .and_then(|e| e.grip_menu_value_prompt(popup.grip_id, item.action));
                if let Some(label) = prompt {
                    self.grip_pending = Some(crate::app::GripPendingValue {
                        handle: popup.handle,
                        grip_id: popup.grip_id,
                        action: item.action,
                        label,
                    });
                    self.command_line.push_info(&format!("{label}:"));
                    return self.focus_cmd_input();
                }
                // One-shot action — apply immediately.
                self.push_undo_snapshot(i, item.label);
                // For Add Leader, the new arrow becomes the last grip; remember
                // its id so we can grab it for placement right after.
                let add_leader_gid = if matches!(item.action, GripMenuAction::AddLeader) {
                    self.tabs[i]
                        .scene
                        .document
                        .get_entity(popup.handle)
                        .and_then(|e| match e {
                            acadrust::EntityType::MultiLeader(ml) => Some(
                                ml.context
                                    .leader_roots
                                    .iter()
                                    .flat_map(|r| r.lines.iter())
                                    .map(|l| l.points.len())
                                    .sum::<usize>(),
                            ),
                            _ => None,
                        })
                } else {
                    None
                };
                if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(popup.handle) {
                    entity.apply_grip_menu(popup.grip_id, item.action);
                }
                self.tabs[i].scene.bump_geometry();
                self.tabs[i].dirty = true;
                self.refresh_selected_grips();
                self.refresh_properties();
                // Grab the new arrow so it follows the cursor (click places it,
                // Esc removes it).
                if let Some(new_gid) = add_leader_gid {
                    if let Some(g) = self.tabs[i].selected_grips.iter().find(|g| g.id == new_gid) {
                        self.tabs[i].active_grip = Some(GripEdit {
                            handle: popup.handle,
                            grip_id: new_gid,
                            is_translate: false,
                            origin_world: g.world,
                            last_world: g.world,
                        });
                        self.grip_add_provisional = Some((popup.handle, new_gid));
                    }
                }
                Task::none()
    }

    pub(super) fn on_paste_shortcut(&mut self) -> Task<Message> {
                if self.mtext_editor.is_some() {
                    // Web reads via the browser's async Clipboard API (iced's
                    // sync clipboard read returns nothing there); native uses
                    // iced's clipboard.
                    #[cfg(target_arch = "wasm32")]
                    return Task::perform(
                        crate::sys::read_clipboard_text(),
                        Message::MTextPasteClip,
                    );
                    #[cfg(not(target_arch = "wasm32"))]
                    return iced::clipboard::read().map(Message::MTextPasteClip);
                }
                if self.text_inline.is_some() {
                    // Web: the iced text_input can't reach the async clipboard,
                    // so paste it ourselves. Native: the focused text_input
                    // already handled Ctrl+V — doing it here would duplicate.
                    #[cfg(target_arch = "wasm32")]
                    return Task::perform(
                        crate::sys::read_clipboard_text(),
                        Message::TextInlinePasteClip,
                    );
                    #[cfg(not(target_arch = "wasm32"))]
                    return Task::none();
                }
                Task::done(Message::Command("PASTECLIP".to_string()))
    }

    pub(super) fn on_qselect_open(&mut self) -> Task<Message> {
                let i = self.active_tab;
                self.tabs[i].scene.selection.borrow_mut().context_menu = None;
                // Seed the type filter from the first selected entity so a
                // right-click → Quick Select on a known object opens the
                // panel pre-tuned to that entity's type. Property defaults
                // to "(Any property)" so the user immediately picks what
                // they want to compare.
                let mut type_filter: Option<String> = None;
                if let Some(&h) = self.tabs[i].scene.selected.iter().next() {
                    if let Some(e) = self.tabs[i].scene.document.get_entity(h) {
                        use crate::entities::traits::entity_type_name;
                        type_filter = Some(entity_type_name(e).to_string());
                    }
                }
                self.qselect = Some(crate::app::QSelectState {
                    type_filter,
                    property: None,
                    operator: crate::app::QSelectOp::Eq,
                    value: String::new(),
                    append: false,
                });
                Task::none()
    }

    pub(super) fn on_ribbon_layer_changed(&mut self, layer: String) -> Task<Message> {
                let i = self.active_tab;
                self.ribbon.close_dropdown();
                let handles = self.property_target_handles(i);
                if handles.is_empty() {
                    // No selection — change the creation default. Persist
                    // into the tab's header (CLAYER) so it survives a tab
                    // switch and rides the next save. #21.
                    let handle = self.tabs[i]
                        .scene
                        .document
                        .layers
                        .get(&layer)
                        .map(|l| l.handle)
                        .unwrap_or(acadrust::types::Handle::NULL);
                    self.tabs[i].scene.document.header.current_layer_name = layer.clone();
                    self.tabs[i].scene.document.header.current_layer_handle = handle;
                    self.tabs[i].active_layer = layer.clone();
                    self.tabs[i].layers.current_layer = layer.clone();
                    self.tabs[i].dirty = true;
                    self.ribbon.active_layer = layer;
                } else {
                    // Apply to selection; leave the creation default alone
                    // (matches AutoCAD; "Make current" is a separate action).
                    self.push_undo_snapshot(i, "CHPROP");
                    for handle in handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_common_prop(entity, "layer", &layer);
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.ribbon.active_layer = layer;
                    self.refresh_properties();
                }
                Task::none()
    }

    pub(super) fn on_ribbon_color_changed(&mut self, color: AcadColor) -> Task<Message> {
                let i = self.active_tab;
                self.ribbon.prop_color_palette_open = false;
                self.ribbon.close_dropdown();
                let handles = self.property_target_handles(i);
                if handles.is_empty() {
                    // Persist the new default into the tab's header so it
                    // round-trips through tab switches and writes back on
                    // save (CECOLOR). #21.
                    self.tabs[i].scene.document.header.current_entity_color = color;
                    self.tabs[i].dirty = true;
                    self.ribbon.active_color = color;
                } else {
                    self.push_undo_snapshot(i, "CHPROP");
                    for &handle in &handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_color(entity, color);
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].dirty = true;
                    self.ribbon.active_color = color;
                    self.refresh_properties();
                }
                Task::none()
    }

    pub(super) fn on_ribbon_linetype_changed(&mut self, lt: String) -> Task<Message> {
                let i = self.active_tab;
                self.ribbon.close_dropdown();
                let handles = self.property_target_handles(i);
                if handles.is_empty() {
                    // Persist into the tab's header (CELTYPE). Resolve to a
                    // handle when the name matches a line_types entry so the
                    // handle-based lookup stays in sync. #21.
                    let handle = self.tabs[i]
                        .scene
                        .document
                        .line_types
                        .iter()
                        .find(|x| x.name.eq_ignore_ascii_case(&lt))
                        .map(|x| x.handle)
                        .unwrap_or(acadrust::types::Handle::NULL);
                    self.tabs[i].scene.document.header.current_linetype_name = lt.clone();
                    self.tabs[i].scene.document.header.current_linetype_handle = handle;
                    self.tabs[i].dirty = true;
                    self.ribbon.active_linetype = lt;
                } else {
                    self.push_undo_snapshot(i, "CHPROP");
                    for handle in handles {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                            crate::scene::view::dispatch::apply_common_prop(entity, "linetype", &lt);
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.ribbon.active_linetype = lt;
                    self.refresh_properties();
                }
                Task::none()
    }

    pub(super) fn on_ribbon_style_changed(&mut self, key: crate::modules::StyleKey, name: String) -> Task<Message> {
                use crate::modules::StyleKey;
                self.ribbon.close_dropdown();
                match key {
                    StyleKey::TextStyle => {
                        self.ribbon.active_text_style = name.clone();
                        let i = self.active_tab;
                        let found = self.tabs[i]
                            .scene
                            .document
                            .text_styles
                            .iter()
                            .find(|s| s.name == name)
                            .map(|ts| ts.handle);
                        if let Some(h) = found {
                            self.tabs[i].scene.document.header.current_text_style_handle = h;
                            self.tabs[i].scene.document.header.current_text_style_name = name;
                        }
                    }
                    StyleKey::DimStyle => {
                        self.ribbon.active_dim_style = name.clone();
                        let i = self.active_tab;
                        let found = self.tabs[i]
                            .scene
                            .document
                            .dim_styles
                            .get(&name)
                            .map(|ds| ds.handle);
                        if let Some(h) = found {
                            self.tabs[i].scene.document.header.current_dimstyle_handle = h;
                            self.tabs[i].scene.document.header.current_dimstyle_name = name;
                        }
                    }
                    StyleKey::MLeaderStyle => {
                        self.ribbon.active_mleader_style = name.clone();
                        let i = self.active_tab;
                        self.tabs[i].active_mleader_style = name;
                    }
                    StyleKey::TableStyle => {
                        self.ribbon.active_table_style = name;
                    }
                }
                Task::none()
    }

    pub(super) fn on_prop_hatch_pattern_changed(&mut self, name: String) -> Task<Message> {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    use crate::scene::model::hatch_patterns;
                    if let Some(entry) = hatch_patterns::find(&name) {
                        self.push_undo_snapshot(i, "HATCHEDIT");
                        for &handle in &handles {
                            if let Some(acadrust::EntityType::Hatch(dxf)) =
                                self.tabs[i].scene.document.get_entity_mut(handle)
                            {
                                dxf.pattern = hatch_patterns::build_dxf_pattern(entry);
                                dxf.is_solid = matches!(
                                    entry.gpu,
                                    crate::scene::model::hatch_model::HatchPattern::Solid
                                );
                            }
                            if let Some(model) = self.tabs[i].scene.hatches.get_mut(&handle) {
                                model.pattern = entry.gpu.clone();
                                model.name = name.clone();
                            }
                        }
                        self.invalidate_property_targets(i, &handles);
                        self.tabs[i].dirty = true;
                        self.refresh_properties();
                    }
                }
                Task::none()
    }

    pub(super) fn on_prop_geom_choice_changed(&mut self, field: &'static str, value: String) -> Task<Message> {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    self.push_undo_snapshot(i, "CHPROP");
                    if field == "vp_ucs_name" {
                        // Resolve UCS name → cloned data, then mutate viewports.
                        let ucs_data = self.tabs[i]
                            .scene
                            .document
                            .ucss
                            .iter()
                            .find(|u| u.name == value)
                            .cloned();
                        if let Some(ucs) = ucs_data {
                            for handle in &handles {
                                if let Some(acadrust::EntityType::Viewport(vp)) =
                                    self.tabs[i].scene.document.get_entity_mut(*handle)
                                {
                                    vp.ucs_handle = ucs.handle;
                                    vp.ucs_origin = ucs.origin.clone();
                                    vp.ucs_x_axis = ucs.x_axis.clone();
                                    vp.ucs_y_axis = ucs.y_axis.clone();
                                }
                            }
                        }
                    } else if field == "vp_named_view" {
                        // Assign a named view to viewport(s): copy camera parameters.
                        let view_data = self.tabs[i]
                            .scene
                            .document
                            .views
                            .iter()
                            .find(|v| v.name == value)
                            .cloned();
                        if let Some(view) = view_data {
                            for handle in &handles {
                                if let Some(acadrust::EntityType::Viewport(vp)) =
                                    self.tabs[i].scene.document.get_entity_mut(*handle)
                                {
                                    vp.view_target = view.target.clone();
                                    vp.view_direction = view.direction.clone();
                                    if view.height > 0.0 {
                                        vp.view_height = view.height;
                                    }
                                }
                            }
                            self.tabs[i].scene.camera_generation += 1;
                        }
                    } else {
                        for &handle in &handles {
                            if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle)
                            {
                                crate::scene::view::dispatch::apply_geom_prop(entity, field, &value);
                            }
                        }
                    }
                    self.invalidate_property_targets(i, &handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                }
                Task::none()
    }

    pub(super) fn on_prop_geom_commit(&mut self, field: &'static str) -> Task<Message> {
                let i = self.active_tab;
                let handles = self.property_target_handles(i);
                if !handles.is_empty() {
                    if let Some(raw_val) = self.tabs[i].properties.edit_buf.remove(field) {
                        let val = crate::app::expr_eval::eval_to_string(&raw_val);
                        self.push_undo_snapshot(i, "CHPROP");
                        if field == "frozen_layers" {
                            // Resolve layer names → handles, then apply to viewports.
                            let layer_handles: Vec<acadrust::Handle> = val
                                .split(',')
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty())
                                .filter_map(|name| {
                                    self.tabs[i]
                                        .scene
                                        .document
                                        .layers
                                        .iter()
                                        .find(|l| l.name.eq_ignore_ascii_case(name))
                                        .map(|l| l.handle)
                                })
                                .collect();
                            for &handle in &handles {
                                if let Some(acadrust::EntityType::Viewport(vp)) =
                                    self.tabs[i].scene.document.get_entity_mut(handle)
                                {
                                    vp.frozen_layers = layer_handles.clone();
                                }
                            }
                        } else {
                            for &handle in &handles {
                                if let Some(entity) =
                                    self.tabs[i].scene.document.get_entity_mut(handle)
                                {
                                    match field {
                                        "linetype_scale" | "transparency" => {
                                            crate::scene::view::dispatch::apply_common_prop(
                                                entity, field, &val,
                                            );
                                        }
                                        _ => {
                                            crate::scene::view::dispatch::apply_geom_prop(
                                                entity, field, &val,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        self.invalidate_property_targets(i, &handles);
                        self.tabs[i].dirty = true;
                        self.refresh_properties();
                    }
                }
                Task::none()
    }
}
