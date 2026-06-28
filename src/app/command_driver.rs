use super::{Message, OpenCADStudio};
use crate::command::{CmdResult, StepInput};
use acadrust::Handle;
use iced::Task;

impl OpenCADStudio {
    /// Drive the active command's step machine with one [`StepInput`], then
    /// apply the result. This is the single entry point every input source
    /// (command line, headless, dynamic input, plugin API, viewport) funnels
    /// through, so the routing from input → `on_*` method → `apply_cmd_result`
    /// lives in exactly one place. No-op when no command is active.
    pub(super) fn feed_command(&mut self, input: StepInput) -> Task<Message> {
        let i = self.active_tab;
        let result: Option<CmdResult> = {
            let Some(cmd) = self.tabs[i].active_cmd.as_mut() else {
                return Task::none();
            };
            match input {
                StepInput::Point(p) => Some(cmd.on_point(p)),
                StepInput::Text(s) => cmd.on_text_input(&s),
                StepInput::EntityPick(h, p) => Some(cmd.on_entity_pick(h, p)),
                StepInput::StructurePick(h, p) => Some(cmd.on_structure_pick(h, p)),
                StepInput::SelectionComplete(hs) => Some(cmd.on_selection_complete(hs)),
                StepInput::Tangent(o, p) => Some(cmd.on_tangent_point(o, p)),
                StepInput::EditorClosed(c) => Some(cmd.on_editor_closed(c)),
                StepInput::Enter => Some(cmd.on_enter()),
                StepInput::Escape => Some(cmd.on_escape()),
            }
        };
        match result {
            Some(r) => self.apply_cmd_result(r),
            None => Task::none(),
        }
    }

    /// Run one whole command-line string. A single word or an inline-argument
    /// command (`PDMODE 3`, `LAYER Walls`, `UCS Z 90` pasted as one line)
    /// dispatches as-is; for a multi-token line whose first word starts an
    /// interactive tool (`LINE 0,0 10,10`) the first word starts the tool and the
    /// remaining tokens are fed as points / option keywords, then the command is
    /// terminated as if Enter were pressed. Shared by the GUI command line and
    /// the headless automation feeder so both behave identically.
    pub(super) fn run_command_line(&mut self, cmd: &str) -> Task<Message> {
        let i = self.active_tab;
        let tokens: Vec<&str> = cmd.split_whitespace().collect();
        if tokens.len() <= 1 {
            return self.dispatch_command(cmd);
        }
        // Plugin commands parse their own inline arguments from the whole line
        // (e.g. `HC_PIPE 2B 2C 1.25 0.013`), so offer the full command to plugin
        // dispatch first. A built-in interactive tool matches only its bare name
        // (`LINE`), so the full line is not a plugin command and falls through to
        // the first-word + fed-tokens path below. (#162)
        if crate::plugin::try_dispatch(self, i, cmd) {
            let toks: Vec<String> = tokens.iter().map(|s| s.to_string()).collect();
            self.finish_active_command(&toks);
            return Task::none();
        }
        let _ = self.dispatch_command(tokens[0]);
        if self.tabs[i].active_cmd.is_none() {
            // Not an interactive tool — an inline-argument command (`PDMODE 3`).
            return self.dispatch_command(cmd);
        }
        let toks: Vec<String> = tokens.iter().map(|s| s.to_string()).collect();
        self.finish_active_command(&toks);
        Task::none()
    }

    /// Feed `tokens[1..]` to the active interactive command as points / option
    /// keywords, then terminate it as if Enter were pressed. No-op when no
    /// command is active.
    pub(super) fn finish_active_command(&mut self, tokens: &[String]) {
        let i = self.active_tab;
        if self.tabs[i].active_cmd.is_none() {
            return;
        }
        self.last_point = None;
        for tok in &tokens[1..] {
            if self.tabs[i].active_cmd.is_none() {
                break;
            }
            self.feed_active_cmd(tok);
        }
        let _ = self.feed_command(StepInput::Enter);
    }

    /// Classify one typed token into a [`StepInput`] and route it through the
    /// shared [`Self::feed_command`]. An object-pick step takes a hex handle; a
    /// coordinate is parsed (and, like the GUI command line, interpreted in the
    /// active UCS); anything else is an option keyword / value. Used by both the
    /// GUI command line and headless automation.
    pub(super) fn feed_active_cmd(&mut self, token: &str) {
        let i = self.active_tab;
        // Object-pick step: the token is a handle (as returned by `query`).
        if self.tabs[i]
            .active_cmd
            .as_ref()
            .is_some_and(|c| c.needs_entity_pick())
        {
            if let Ok(v) = u64::from_str_radix(token.trim_start_matches("0x"), 16) {
                let handle = Handle::new(v);
                let pt = self.tabs[i]
                    .scene
                    .document
                    .get_entity(handle)
                    .map(|e| {
                        let bb = e.as_entity().bounding_box();
                        glam::Vec3::new(
                            ((bb.min.x + bb.max.x) * 0.5) as f32,
                            ((bb.min.y + bb.max.y) * 0.5) as f32,
                            0.0,
                        )
                    })
                    .unwrap_or(glam::Vec3::ZERO);
                let _ = self.feed_command(StepInput::EntityPick(handle, pt.as_dvec3()));
            }
            return;
        }
        if let Some((coord, kind)) = super::helpers::parse_coord(token) {
            // Match the GUI command line: typed coordinates are in the active
            // UCS (relative offsets are rotated by the UCS axes), so a multi-
            // token `LINE 0,0 10,10` under a rotated UCS lands correctly.
            let ucs = self.tabs[i].active_ucs.clone();
            let wcs = match (matches!(kind, super::helpers::CoordKind::Relative), self.last_point) {
                (true, Some(base)) => {
                    base + match &ucs {
                        Some(u) => super::helpers::ucs_rotate_vec(coord, u),
                        None => coord,
                    }
                }
                _ => match &ucs {
                    Some(u) => super::helpers::ucs_to_wcs(coord, u),
                    None => coord,
                },
            };
            self.last_point = Some(wcs);
            self.push_ucs_to_cmd(i);
            let _ = self.feed_command(StepInput::Point(wcs.as_dvec3()));
        } else {
            let _ = self.feed_command(StepInput::Text(token.to_string()));
        }
    }

    /// Applies one command result, then — when that result ended the active
    /// command — drops the selection. Editing tools (MOVE, COPY, ROTATE, …) and
    /// every other interactive command leave nothing selected once they finish,
    /// so a follow-up edit doesn't silently reuse the previous working set.
    ///
    /// `Relaunch`/`Dispatch` are excepted: they end the front-end command only
    /// to immediately start another and hand it a deliberate selection (the
    /// pick-first selector relaunching MOVE on the picked set works this way).
    /// Pure-selection commands (SELECTALL, QSELECT, …) run without an active
    /// command, so `was_active` is false and their selection is preserved.
    pub(super) fn apply_cmd_result(&mut self, result: CmdResult) -> Task<Message> {
        let was_active = self.tabs[self.active_tab].active_cmd.is_some();
        let preserve_selection =
            matches!(result, CmdResult::Relaunch(..) | CmdResult::Dispatch(..));
        let task = self.apply_cmd_result_inner(result);
        let i = self.active_tab;
        if was_active
            && !preserve_selection
            && self.tabs[i].active_cmd.is_none()
            && !self.tabs[i].scene.selected.is_empty()
        {
            self.tabs[i].scene.deselect_all();
            self.refresh_properties();
        }
        task
    }

    fn apply_cmd_result_inner(&mut self, result: CmdResult) -> Task<Message> {
        let i = self.active_tab;
        let mut task = Task::none();
        match result {
            CmdResult::NeedPoint => {
                // If ATTEDIT just completed entity pick, inject attribute data.
                let attedit_handle = self.tabs[i]
                    .active_cmd
                    .as_ref()
                    .and_then(|c| c.attedit_pending_handle());
                if let Some(ins_handle) = attedit_handle {
                    if let Some(acadrust::EntityType::Insert(ins)) =
                        self.tabs[i].scene.document.get_entity(ins_handle)
                    {
                        let attrs: Vec<(String, String)> = ins
                            .attributes
                            .iter()
                            .map(|a| (a.tag.clone(), a.get_value().to_string()))
                            .collect();
                        if attrs.is_empty() {
                            self.command_line
                                .push_error("ATTEDIT  This INSERT has no attributes.");
                            self.tabs[i].active_cmd = None;
                            return Task::none();
                        }
                        if let Some(cmd) = &mut self.tabs[i].active_cmd {
                            cmd.attedit_set_attrs(attrs);
                        }
                    } else {
                        self.command_line
                            .push_error("ATTEDIT  Please select an INSERT entity with attributes.");
                        self.tabs[i].active_cmd = None;
                        return Task::none();
                    }
                }
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
                // The command may have advanced to a step with a different
                // dynamic-input shape (e.g. FILLET object-pick → radius entry).
                // Rebuild the fields now so the matching box appears immediately
                // and typed digits land in it rather than the command line,
                // instead of waiting for the next cursor move to resync.
                self.sync_dyn_fields();
            }
            CmdResult::Preview(wire) => {
                self.tabs[i].scene.set_preview_wires(vec![wire]);
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
            }
            CmdResult::InterimWire(wire) => {
                self.tabs[i].scene.set_interim_wire(wire);
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
            }
            CmdResult::CommitEntity(entity) => {
                let label = self.history_label_from_active_cmd(i, "ENTITY");
                self.push_undo_snapshot(i, label);
                self.commit_entity(entity);
                self.tabs[i].dirty = true;
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
            }
            CmdResult::TransformSelected(handles, transform) => {
                let label = self.history_label_from_active_cmd(i, "MOVE");
                self.push_undo_snapshot(i, label);
                self.tabs[i].scene.transform_entities(&handles, &transform);
                // ACIS solids render from a cached mesh, so a move/rotate/
                // scale/mirror needs the mesh re-tessellated from the now-moved
                // body — wire re-tessellation alone leaves the solid drawn at
                // its old spot. (#135)
                if self.tabs[i].scene.any_solid(&handles) {
                    self.tabs[i].scene.populate_meshes_from_document();
                }
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
                self.refresh_properties();
            }
            CmdResult::CopySelected(handles, transform) => {
                let label = self.history_label_from_active_cmd(i, "COPY");
                self.push_undo_snapshot(i, label);
                let new_handles = self.tabs[i].scene.copy_entities(&handles, &transform);
                if self.tabs[i].scene.any_solid(&new_handles) {
                    self.tabs[i].scene.populate_meshes_from_document();
                }
                self.tabs[i].dirty = true;
                self.tabs[i].scene.deselect_all();
                for h in new_handles {
                    self.tabs[i].scene.select_entity(h, false);
                }
                self.tabs[i].scene.clear_preview_wire();
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
                self.refresh_properties();
            }
            CmdResult::CommitAndExit(entity) => {
                // For XATTACH: ensure the xref block definition exists before
                // committing the INSERT entity that references it.
                // Extract path early to avoid borrow conflicts.
                let xattach_path: Option<String> = {
                    let tab = &self.tabs[i];
                    if let Some(cmd) = tab.active_cmd.as_ref() {
                        if cmd.name() == "XATTACH" {
                            cmd.xattach_path()
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                if let Some(path) = xattach_path {
                    crate::modules::insert::xattach::prepare_xref_block(
                        &mut self.tabs[i].scene,
                        &path,
                    );
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        // Resolve the referenced file on a worker thread so the
                        // UI stays responsive while large xrefs parse.
                        let doc = self.tabs[i].scene.document.clone();
                        let base_dir = std::path::PathBuf::from(&path)
                            .parent()
                            .map(|p| p.to_path_buf())
                            .unwrap_or_default();
                        let tab_idx = self.active_tab;
                        task = Task::perform(
                            async move {
                                let (doc, infos, dropped) =
                                    crate::io::xref::resolve_xrefs_on_thread(doc, base_dir);
                                (tab_idx, doc, infos, dropped)
                            },
                            |(tab_idx, doc, infos, dropped)| {
                                Message::XrefsResolved(tab_idx, doc, infos, dropped)
                            },
                        );
                    }
                }
                let label = self.history_label_from_active_cmd(i, "ENTITY");
                self.push_undo_snapshot(i, label);
                self.commit_entity(entity);
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
            }
            CmdResult::CommitSolid { entity, solid } => {
                let label = self.history_label_from_active_cmd(i, "SOLID");
                self.push_undo_snapshot(i, label);
                self.add_solid_model(entity, *solid);
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
            }
            CmdResult::CommitAndEditText(entity) => {
                let label = self.history_label_from_active_cmd(i, "ENTITY");
                self.push_undo_snapshot(i, label);
                let handle = self.commit_entity_handle(entity);
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
                self.ribbon.deactivate_tool();
                if let Some(h) = handle {
                    return self.begin_text_edit(h);
                }
            }
            CmdResult::CommitManyAndEditText {
                entities,
                edit_index,
            } => {
                let label = self.history_label_from_active_cmd(i, "ENTITY");
                self.push_undo_snapshot(i, label);
                let mut edit_handle = None;
                let mut leader_handle = None;
                for (idx, entity) in entities.into_iter().enumerate() {
                    let is_leader = matches!(entity, acadrust::EntityType::Leader(_));
                    let h = self.commit_entity_handle(entity);
                    if idx == edit_index {
                        edit_handle = h;
                    }
                    if is_leader {
                        leader_handle = h;
                    }
                }
                // Link the leader to its annotation so the pair edits as a unit
                // (double-click on the leader resolves to the text entity).
                if let (Some(lh), Some(ah)) = (leader_handle, edit_handle) {
                    if let Some(acadrust::EntityType::Leader(l)) =
                        self.tabs[i].scene.document.get_entity_mut(lh)
                    {
                        l.annotation_handle = ah;
                    }
                }
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
                self.ribbon.deactivate_tool();
                if let Some(h) = edit_handle {
                    return self.begin_text_edit(h);
                }
            }
            CmdResult::CreateBlock {
                handles,
                name,
                base,
            } => {
                self.push_undo_snapshot(i, "BLOCK");
                match self.tabs[i]
                    .scene
                    .create_block_from_entities(&handles, &name, base.as_vec3())
                {
                    Ok(insert_handle) => {
                        self.tabs[i].dirty = true;
                        self.tabs[i].scene.deselect_all();
                        if !insert_handle.is_null() {
                            self.tabs[i].scene.select_entity(insert_handle, false);
                        }
                        self.tabs[i].scene.clear_preview_wire();
                        self.tabs[i].active_cmd = None;
                        self.tabs[i].snap_result = None;
                        self.command_line
                            .push_output(&format!("Block \"{name}\" created."));
                        self.refresh_properties();
                    }
                    Err(err) => {
                        let _ = self.tabs[i].history.undo_stack.pop();
                        self.command_line.push_error(&err);
                        let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                        if let Some(p) = prompt {
                            self.command_line.push_info(&p);
                        }
                    }
                }
            }
            CmdResult::CommitHatch(hatch) => {
                let label = self.history_label_from_active_cmd(i, "HATCH");
                self.push_undo_snapshot(i, label);
                let new_handle = self.tabs[i].scene.add_hatch(hatch);
                if !new_handle.is_null() {
                    self.tabs[i].scene.select_entity(new_handle, true);
                }
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
                self.refresh_properties();
            }
            CmdResult::BatchCopy(handles, transforms) => {
                let label = self.history_label_from_active_cmd(i, "ARRAY");
                self.push_undo_snapshot(i, label.clone());
                let count = transforms.len();
                for t in &transforms {
                    self.tabs[i].scene.copy_entities(&handles, t);
                }
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
                let noun = if count == 1 { "copy" } else { "copies" };
                self.command_line
                    .push_output(&format!("{label}: {count} {noun} created."));
                self.refresh_properties();
            }
            CmdResult::ReplaceMany(replacements, additions) => {
                let label = self.history_label_from_active_cmd(i, "FILLET");
                let was_catchment = self
                    .tabs[i]
                    .active_cmd
                    .as_ref()
                    .is_some_and(|c| c.name() == "SS_CATCHMENT");
                self.push_undo_snapshot(i, label);
                for (handle, entities) in replacements {
                    self.tabs[i].scene.erase_entities(&[handle]);
                    for entity in entities {
                        self.tabs[i].scene.add_entity(entity);
                    }
                }
                for entity in additions {
                    self.tabs[i].scene.add_entity(entity);
                }
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                if was_catchment {
                    self.command_line
                        .push_info("Catchment tagged successfully.");
                }
                self.refresh_properties();
            }
            CmdResult::ReplaceEntity(handle, new_entities) => {
                // Detect ATTEDIT sentinel.
                if new_entities.len() == 1 {
                    if let acadrust::EntityType::XLine(ref xl) = new_entities[0] {
                        let layer = xl.common.layer.clone();
                        if let Some(encoded) = layer.strip_prefix("__ATTEDIT__") {
                            let label = self.history_label_from_active_cmd(i, "ATTEDIT");
                            self.push_undo_snapshot(i, label);
                            crate::modules::draw::modify::attedit::apply_attedit(
                                &mut self.tabs[i].scene.document,
                                handle,
                                encoded,
                            );
                            self.tabs[i].dirty = true;
                            self.tabs[i].active_cmd = None;
                            self.tabs[i].snap_result = None;
                            self.command_line
                                .push_output("ATTEDIT  Attribute values updated.");
                            return Task::none();
                        }
                    }
                }

                // Detect SPLINEDIT sentinel: a single XLine with a magic layer name.
                if new_entities.len() == 1 {
                    if let acadrust::EntityType::XLine(ref xl) = new_entities[0] {
                        let op = xl.common.layer.clone();
                        if op.starts_with("__SPLINEDIT_") {
                            let label = self.history_label_from_active_cmd(i, "SPLINEDIT");
                            self.push_undo_snapshot(i, label);
                            crate::modules::draw::modify::splinedit::apply_spline_op(
                                &mut self.tabs[i].scene.document,
                                handle,
                                &op,
                            );
                            self.tabs[i].dirty = true;
                            let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                            if let Some(p) = prompt {
                                self.command_line.push_info(&p);
                            }
                            return Task::none();
                        }
                    }
                }
                // Detect DIMBREAK sentinel.
                if new_entities.len() == 1 {
                    if let acadrust::EntityType::XLine(ref xl) = new_entities[0] {
                        let layer = xl.common.layer.clone();
                        if layer.starts_with("__DIMBREAK__")
                            || layer.starts_with("__DIMBREAK_AUTO__")
                        {
                            // DIMBREAK needs a break-gap field on the dimension
                            // model (not yet present) to store and render the gap.
                            // Report honestly rather than claiming success while
                            // changing nothing. (#181 / DIM-020)
                            self.command_line
                                .push_info("DIMBREAK: not yet implemented — nothing changed.");
                            self.tabs[i].active_cmd = None;
                            self.tabs[i].snap_result = None;
                            return Task::none();
                        }
                        if layer.starts_with("__DIMSPACE__") {
                            if let Some(encoded) = layer.strip_prefix("__DIMSPACE__") {
                                apply_dimspace(&mut self.tabs[i].scene, encoded);
                            }
                            self.push_undo_snapshot(i, "DIMSPACE");
                            self.command_line.push_output("DIMSPACE  Spacing adjusted.");
                            self.tabs[i].dirty = true;
                            self.tabs[i].active_cmd = None;
                            self.tabs[i].snap_result = None;
                            return Task::none();
                        }
                        if layer.starts_with("__DIMJOG__") {
                            // DIMJOGLINE needs a jog-point field on the dimension
                            // model (not yet present) to store and render the jog.
                            // Report honestly rather than faking success. (DIM-019)
                            self.command_line
                                .push_info("DIMJOGLINE: not yet implemented — nothing changed.");
                            self.tabs[i].active_cmd = None;
                            self.tabs[i].snap_result = None;
                            return Task::none();
                        }
                        if layer.starts_with("__MLEADERALIGN__") {
                            if let Some(encoded) = layer.strip_prefix("__MLEADERALIGN__") {
                                apply_mleader_align(&mut self.tabs[i].scene, encoded);
                            }
                            self.push_undo_snapshot(i, "MLEADERALIGN");
                            self.command_line
                                .push_output("MLEADERALIGN  Leaders aligned.");
                            self.tabs[i].dirty = true;
                            self.tabs[i].active_cmd = None;
                            self.tabs[i].snap_result = None;
                            return Task::none();
                        }
                        if layer.starts_with("__MLEADERCOLLECT__") {
                            if let Some(encoded) = layer.strip_prefix("__MLEADERCOLLECT__") {
                                apply_mleader_collect(&mut self.tabs[i].scene, encoded);
                            }
                            self.push_undo_snapshot(i, "MLEADERCOLLECT");
                            self.command_line
                                .push_output("MLEADERCOLLECT  Leaders collected.");
                            self.tabs[i].dirty = true;
                            self.tabs[i].active_cmd = None;
                            self.tabs[i].snap_result = None;
                            return Task::none();
                        }
                    }
                }

                let label = self.history_label_from_active_cmd(i, "TRIM");
                self.push_undo_snapshot(i, label);
                self.tabs[i].scene.erase_entities(&[handle]);
                let new_handles: Vec<acadrust::Handle> = new_entities
                    .into_iter()
                    .map(|e| self.tabs[i].scene.add_entity(e))
                    .collect();
                if let Some(cmd) = &mut self.tabs[i].active_cmd {
                    cmd.on_entity_replaced(handle, &new_handles);
                }
                self.tabs[i].dirty = true;
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
            }
            CmdResult::AttreqNeeded { block_name } => {
                // Collect AttributeDefinitions owned by this block record.
                let attdefs: Vec<(String, String, String)> = {
                    let doc = &self.tabs[i].scene.document;
                    if let Some(br) = doc.block_records.get(&block_name) {
                        br.entity_handles
                            .iter()
                            .filter_map(|&h| {
                                if let Some(acadrust::EntityType::AttributeDefinition(ad)) =
                                    doc.get_entity(h)
                                {
                                    Some((
                                        ad.tag.clone(),
                                        ad.prompt.clone(),
                                        ad.default_value.clone(),
                                    ))
                                } else {
                                    None
                                }
                            })
                            .collect()
                    } else {
                        vec![]
                    }
                };

                if attdefs.is_empty() {
                    // No attribute definitions — commit the INSERT directly.
                    let entity = self.tabs[i]
                        .active_cmd
                        .as_mut()
                        .and_then(|c| c.attreq_take_insert());
                    if let Some(entity) = entity {
                        let label = self.history_label_from_active_cmd(i, "INSERT");
                        self.push_undo_snapshot(i, label);
                        self.commit_entity(entity);
                        self.tabs[i].dirty = true;
                        self.tabs[i].scene.clear_preview_wire();
                        self.tabs[i].active_cmd = None;
                        self.tabs[i].snap_result = None;
                        self.restore_pre_cmd_tangent();
                    }
                } else {
                    // Inject attdefs so the command enters attr-filling mode.
                    if let Some(cmd) = &mut self.tabs[i].active_cmd {
                        cmd.attreq_set_attdefs(attdefs);
                    }
                    let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                    if let Some(p) = prompt {
                        self.command_line.push_info(&p);
                    }
                }
            }
            CmdResult::CommitLiveEntity(entity) => {
                let label = self.history_label_from_active_cmd(i, "ENTITY");
                self.push_undo_snapshot(i, label);
                let handle = self.commit_entity_handle(entity);
                self.tabs[i].dirty = true;
                self.tabs[i].scene.clear_preview_wire();
                if let Some(h) = handle {
                    if let Some(cmd) = self.tabs[i].active_cmd.as_mut() {
                        cmd.set_live_handle(h);
                    }
                }
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
            }
            CmdResult::UpdateLiveEntity {
                handle,
                entity,
                finish,
            } => {
                // Replace the live entity's geometry in place, preserving its
                // handle and layer (the fresh entity from the command carries
                // defaults — a NULL handle would desync it from the document
                // map key and drop it from rendering / hit-test). No undo
                // snapshot — the create already pushed one, so the whole object
                // reverts as a unit.
                if let Some(old) = self.tabs[i].scene.document.get_entity_mut(handle) {
                    let old_handle = old.as_entity().handle();
                    let layer = old.as_entity().layer().to_string();
                    let mut new = entity;
                    new.as_entity_mut().set_handle(old_handle);
                    new.as_entity_mut().set_layer(layer);
                    *old = new;
                    self.tabs[i].scene.mark_entity_dirty(handle);
                    self.tabs[i].scene.bump_geometry_no_blocks();
                    self.tabs[i].dirty = true;
                }
                if finish {
                    self.tabs[i].scene.clear_preview_wire();
                    self.tabs[i].active_cmd = None;
                    self.tabs[i].snap_result = None;
                    self.restore_pre_cmd_tangent();
                } else {
                    let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                    if let Some(p) = prompt {
                        self.command_line.push_info(&p);
                    }
                }
            }
            CmdResult::Cancel => {
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.restore_pre_cmd_tangent();
                self.command_line.push_info("Command cancelled.");
            }
            CmdResult::Relaunch(cmd, handles) => {
                self.tabs[i].scene.deselect_all();
                for h in &handles {
                    self.tabs[i].scene.select_entity(*h, false);
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
                let _ = self.dispatch_command(&cmd);
            }
            CmdResult::Dispatch(cmd) => {
                // End this interactive front-end, then run the assembled command
                // through the normal dispatcher. Selection is left untouched.
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
                let _ = self.dispatch_command(&cmd);
            }
            CmdResult::MatchEntityLayer { dest, src } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                let src_layer = self.tabs[i]
                    .scene
                    .document
                    .get_entity(src)
                    .map(|e| e.common().layer.clone());
                if let Some(layer) = src_layer {
                    self.push_undo_snapshot(i, "LAYMATCH");
                    for h in &dest {
                        if let Some(e) = self.tabs[i].scene.document.get_entity_mut(*h) {
                            e.as_entity_mut().set_layer(layer.clone());
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.command_line
                        .push_info(&format!("Layer matched to \"{layer}\"."));
                    self.sync_ribbon_layers();
                } else {
                    self.command_line.push_error("Source object not found.");
                }
            }
            CmdResult::MatchProperties { dest, src } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();

                let props = self.tabs[i].scene.document.get_entity(src).map(|e| {
                    let c = e.common();
                    (
                        c.layer.clone(),
                        c.color,
                        c.linetype.clone(),
                        c.linetype_scale,
                        c.line_weight,
                    )
                });

                if let Some((layer, color, linetype, lt_scale, lw)) = props {
                    self.push_undo_snapshot(i, "MATCHPROP");
                    for h in &dest {
                        if let Some(e) = self.tabs[i].scene.document.get_entity_mut(*h) {
                            e.as_entity_mut().set_layer(layer.clone());
                            crate::scene::view::dispatch::apply_color(e, color);
                            crate::scene::view::dispatch::apply_line_weight(e, lw);
                            e.common_mut().linetype = linetype.clone();
                            e.common_mut().linetype_scale = lt_scale;
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                    self.command_line
                        .push_info(&format!("Properties matched to {} object(s).", dest.len()));
                } else {
                    self.command_line.push_error("Source object not found.");
                }
            }
            CmdResult::PasteClipboard { base_pt } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                if self.clipboard.is_empty() {
                    self.command_line.push_error("Clipboard is empty.");
                } else {
                    let delta = base_pt - self.clipboard_centroid;
                    let translate = crate::command::EntityTransform::Translate(delta);
                    self.push_undo_snapshot(i, "PASTECLIP");
                    let count = self.clipboard.len();
                    let by_index = self.finalize_paste(i, Some(translate));
                    self.tabs[i].scene.deselect_all();
                    for h in by_index.iter().copied().filter(|h| !h.is_null()) {
                        self.tabs[i].scene.select_entity(h, false);
                    }
                    self.tabs[i].dirty = true;
                    // Surface any layers the paste brought in (cross-drawing)
                    // in the layer manager and the layer dropdown.
                    self.refresh_layer_panel();
                    self.refresh_properties();
                    self.command_line
                        .push_info(&format!("{count} object(s) pasted."));
                }
            }
            CmdResult::CreateGroup { handles, name } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.push_undo_snapshot(i, "GROUP");
                self.tabs[i].scene.create_group(name.clone(), handles);
                self.tabs[i].dirty = true;
                self.command_line
                    .push_info(&format!("Group \"{}\" created.", name));
            }
            CmdResult::DeleteGroups { handles } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.push_undo_snapshot(i, "UNGROUP");
                let count = self.tabs[i].scene.delete_groups_containing(&handles);
                self.tabs[i].dirty = true;
                if count > 0 {
                    self.command_line
                        .push_info(&format!("{} group(s) dissolved.", count));
                } else {
                    self.command_line
                        .push_info("No groups found for selected objects.");
                }
            }
            CmdResult::VpLayerUpdate {
                vp_handle,
                freeze,
                thaw,
            } => {
                // Resolve layer names → handles, then update frozen_layers on the viewport(s).
                // vp_handle == Handle::NULL means "apply to all viewports in current layout".
                let freeze_handles: Vec<Handle> = freeze
                    .iter()
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
                let thaw_handles: Vec<Handle> = thaw
                    .iter()
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

                let mut frozen_count = 0usize;
                let mut thawed_count = 0usize;

                // Collect target viewport handles
                let target_handles: Vec<Handle> = if vp_handle == acadrust::Handle::NULL {
                    // All viewports in current layout block
                    let block_handle = self.tabs[i].scene.current_layout_block_handle_pub();
                    self.tabs[i]
                        .scene
                        .document
                        .entities()
                        .filter(|e| {
                            e.common().owner_handle == block_handle
                                && matches!(e, acadrust::EntityType::Viewport(_))
                        })
                        .map(|e| e.common().handle)
                        .collect()
                } else {
                    vec![vp_handle]
                };

                for &target_handle in &target_handles {
                    if let Some(acadrust::EntityType::Viewport(vp)) =
                        self.tabs[i].scene.document.get_entity_mut(target_handle)
                    {
                        for h in &freeze_handles {
                            if !vp.frozen_layers.contains(h) {
                                vp.frozen_layers.push(*h);
                                frozen_count += 1;
                            }
                        }
                        for h in &thaw_handles {
                            let before = vp.frozen_layers.len();
                            vp.frozen_layers.retain(|fh| fh != h);
                            if vp.frozen_layers.len() < before {
                                thawed_count += 1;
                            }
                        }
                    }
                }

                if frozen_count > 0 || thawed_count > 0 {
                    self.push_undo_snapshot(i, "VPLAYER");
                    self.tabs[i].dirty = true;
                    if frozen_count > 0 {
                        self.command_line.push_info(&format!(
                            "VPLAYER: {frozen_count} layer(s) frozen in viewport."
                        ));
                    }
                    if thawed_count > 0 {
                        self.command_line.push_info(&format!(
                            "VPLAYER: {thawed_count} layer(s) thawed in viewport."
                        ));
                    }
                    // Sync layer panel so VP freeze columns update immediately.
                    let doc_layers = self.tabs[i].scene.document.layers.clone();
                    let vp_info = self.tabs[i].scene.viewport_list();
                    self.tabs[i]
                        .layers
                        .sync_with_viewports(&doc_layers, vp_info);
                }

                // Show updated prompt (command stays active for more operations).
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
            }

            CmdResult::ZoomToWindow { p1, p2 } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.tabs[i].scene.zoom_to_window(p1.as_vec3(), p2.as_vec3());
                self.command_line.push_output("Zoom Window");
            }
            CmdResult::Measurement(msg) => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
                self.command_line.push_output(&msg);
            }
            CmdResult::AlignSelected {
                handles,
                src1,
                dst1,
                angle_rad,
                scale,
            } => {
                if handles.is_empty() {
                    self.tabs[i].active_cmd = None;
                    self.tabs[i].snap_result = None;
                    self.tabs[i].scene.clear_preview_wire();
                    self.restore_pre_cmd_tangent();
                } else {
                    let label = self.history_label_from_active_cmd(i, "ALIGN");
                    self.push_undo_snapshot(i, label);
                    // Step 1: translate so src1 is at origin
                    self.tabs[i].scene.transform_entities(
                        &handles,
                        &crate::command::EntityTransform::Translate(-src1),
                    );
                    // Step 2: uniform scale (only when != 1)
                    if (scale - 1.0).abs() > 1e-4 {
                        self.tabs[i].scene.transform_entities(
                            &handles,
                            &crate::command::EntityTransform::Scale {
                                center: glam::DVec3::ZERO,
                                factor: scale,
                            },
                        );
                    }
                    // Step 3: rotate in the XY plane by angle_rad
                    if angle_rad.abs() > 1e-4 {
                        self.tabs[i].scene.transform_entities(
                            &handles,
                            &crate::command::EntityTransform::Rotate {
                                center: glam::DVec3::ZERO,
                                angle_rad,
                            },
                        );
                    }
                    // Step 4: translate to dst1
                    self.tabs[i].scene.transform_entities(
                        &handles,
                        &crate::command::EntityTransform::Translate(dst1),
                    );
                    self.tabs[i].dirty = true;
                    self.tabs[i].scene.deselect_all();
                    for h in &handles {
                        self.tabs[i].scene.select_entity(*h, false);
                    }
                    self.tabs[i].scene.clear_preview_wire();
                    self.tabs[i].active_cmd = None;
                    self.tabs[i].snap_result = None;
                    self.restore_pre_cmd_tangent();
                    self.command_line.push_output("ALIGN: applied.");
                    self.refresh_properties();
                }
            }
            CmdResult::LengthenEntity {
                handle,
                pick_pt,
                mode,
            } => {
                use crate::modules::draw::modify::lengthen::lengthen_entity;
                let result = self.tabs[i]
                    .scene
                    .document
                    .get_entity(handle)
                    .and_then(|e| lengthen_entity(e, pick_pt.as_vec3(), &mode));
                match result {
                    Some(new_entity) => {
                        let label = self.history_label_from_active_cmd(i, "LENGTHEN");
                        self.push_undo_snapshot(i, label);
                        self.tabs[i].scene.erase_entities(&[handle]);
                        self.tabs[i].scene.add_entity(new_entity);
                        self.tabs[i].dirty = true;
                        self.command_line.push_output("LENGTHEN: applied.");
                        self.refresh_properties();
                    }
                    None => {
                        self.command_line
                            .push_error("LENGTHEN: entity type not supported.");
                    }
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }
            CmdResult::DivideEntity { handle, n } => {
                use crate::modules::draw::inquiry::divide::divide_entity;
                let pts = self.tabs[i]
                    .scene
                    .document
                    .get_entity(handle)
                    .map(|e| divide_entity(e, n))
                    .unwrap_or_default();
                let count = pts.len();
                if count > 0 {
                    self.push_undo_snapshot(i, "DIVIDE");
                    for p in pts {
                        self.tabs[i].scene.add_entity(p);
                    }
                    self.tabs[i].dirty = true;
                    self.command_line
                        .push_output(&format!("DIVIDE: {count} point(s) placed."));
                } else {
                    self.command_line
                        .push_error("DIVIDE: entity type not supported or N < 2.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }
            CmdResult::MeasureEntity {
                handle,
                segment_length,
            } => {
                use crate::modules::draw::inquiry::divide::measure_entity;
                let pts = self.tabs[i]
                    .scene
                    .document
                    .get_entity(handle)
                    .map(|e| measure_entity(e, segment_length))
                    .unwrap_or_default();
                let count = pts.len();
                if count > 0 {
                    self.push_undo_snapshot(i, "MEASURE");
                    for p in pts {
                        self.tabs[i].scene.add_entity(p);
                    }
                    self.tabs[i].dirty = true;
                    self.command_line
                        .push_output(&format!("MEASURE: {count} point(s) placed."));
                } else {
                    self.command_line
                        .push_error("MEASURE: entity type not supported or distance too large.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }
            CmdResult::PeditOp { handle, op } => {
                use crate::modules::draw::modify::pedit::apply_pedit;
                let changed = self.tabs[i]
                    .scene
                    .document
                    .get_entity_mut(handle)
                    .map(|e| apply_pedit(e, &op))
                    .unwrap_or(false);
                if changed {
                    self.push_undo_snapshot(i, "PEDIT");
                    self.tabs[i].dirty = true;
                    self.command_line.push_output("PEDIT: applied.");
                    self.refresh_properties();
                } else {
                    self.command_line
                        .push_error("PEDIT: operation not applicable to this entity.");
                }
                // Keep command active — user may apply more ops
                self.command_line
                    .push_info("PEDIT  Enter option [C=Close O=Open W=Width X=Exit]:");
            }
            CmdResult::JoinEntities(handles) => {
                use crate::modules::draw::modify::join::join_entities;
                let pairs: Vec<_> = handles
                    .iter()
                    .filter_map(|&h| self.tabs[i].scene.document.get_entity(h).map(|e| (h, e)))
                    .collect();
                match join_entities(&pairs) {
                    Some((to_remove, merged)) => {
                        let label = self.history_label_from_active_cmd(i, "JOIN");
                        self.push_undo_snapshot(i, label);
                        self.tabs[i].scene.erase_entities(&to_remove);
                        let count_in = to_remove.len();
                        let count_out = merged.len();
                        for e in merged {
                            self.tabs[i].scene.add_entity(e);
                        }
                        self.tabs[i].dirty = true;
                        self.tabs[i].scene.clear_preview_wire();
                        self.tabs[i].active_cmd = None;
                        self.tabs[i].snap_result = None;
                        self.restore_pre_cmd_tangent();
                        self.command_line.push_output(&format!(
                            "JOIN: {count_in} object(s) joined into {count_out}."
                        ));
                        self.refresh_properties();
                    }
                    None => {
                        self.tabs[i].active_cmd = None;
                        self.tabs[i].snap_result = None;
                        self.tabs[i].scene.clear_preview_wire();
                        self.restore_pre_cmd_tangent();
                        self.command_line.push_error(
                            "JOIN: objects don't form a single connected chain, or contain an unsupported type / tilted arc.",
                        );
                    }
                }
            }
            CmdResult::BreakEntity { handle, p1, p2 } => {
                use crate::modules::draw::modify::break_cmd::break_entity;
                let replacement = self.tabs[i]
                    .scene
                    .document
                    .get_entity(handle)
                    .and_then(|e| break_entity(e, p1.as_vec3(), p2.as_vec3()));
                match replacement {
                    Some(frags) => {
                        let label = self.history_label_from_active_cmd(i, "BREAK");
                        self.push_undo_snapshot(i, label);
                        self.tabs[i].scene.erase_entities(&[handle]);
                        let count = frags.len();
                        for e in frags {
                            self.tabs[i].scene.add_entity(e);
                        }
                        self.tabs[i].dirty = true;
                        self.tabs[i].scene.clear_preview_wire();
                        self.tabs[i].active_cmd = None;
                        self.tabs[i].snap_result = None;
                        self.restore_pre_cmd_tangent();
                        self.command_line
                            .push_output(&format!("BREAK: {} fragment(s).", count));
                        self.refresh_properties();
                    }
                    None => {
                        self.tabs[i].active_cmd = None;
                        self.tabs[i].snap_result = None;
                        self.tabs[i].scene.clear_preview_wire();
                        self.restore_pre_cmd_tangent();
                        self.command_line
                            .push_error("BREAK: entity type not supported.");
                    }
                }
            }
            CmdResult::SetPlotWindow { p1, p2 } => {
                use acadrust::objects::{ObjectType, PlotSettings};
                let layout_name = self.tabs[i].scene.current_layout.clone();
                if layout_name == "Model" {
                    self.command_line
                        .push_error("PLOTWINDOW: switch to a paper space layout first.");
                } else {
                    let block_handle = self.tabs[i].scene.current_layout_block_handle_pub();
                    let doc = &mut self.tabs[i].scene.document;
                    let ps_handle = doc.objects.iter().find_map(|(h, obj)| {
                        if let ObjectType::PlotSettings(ps) = obj {
                            if ps.page_name == layout_name {
                                Some(*h)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    });
                    let ps_entry = match ps_handle {
                        Some(h) => doc.objects.get_mut(&h),
                        None => {
                            let nh = acadrust::Handle::new(doc.next_handle());
                            let ps = PlotSettings::new(layout_name.clone());
                            doc.objects.insert(nh, ObjectType::PlotSettings(ps));
                            doc.objects.get_mut(&nh)
                        }
                    };
                    let _ = block_handle;
                    if let Some(ObjectType::PlotSettings(ps)) = ps_entry {
                        // Convert world-space points to DXF coordinates (X, Z plane → DXF X, Y).
                        let x1 = p1.x.min(p2.x) as f64;
                        let y1 = p1.z.min(p2.z) as f64;
                        let x2 = p1.x.max(p2.x) as f64;
                        let y2 = p1.z.max(p2.z) as f64;
                        ps.set_plot_window(x1, y1, x2, y2);
                        self.push_undo_snapshot(i, "PLOTWINDOW");
                        self.tabs[i].dirty = true;
                        self.command_line.push_output(&format!(
                            "PLOTWINDOW: ({x1:.3},{y1:.3}) → ({x2:.3},{y2:.3})"
                        ));
                    }
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }
            CmdResult::StretchEntities {
                handles,
                win_min,
                win_max,
                delta,
            } => {
                self.push_undo_snapshot(i, "STRETCH");
                let mut count = 0usize;

                // Helper: is DXF point (x, y) inside the world-space window?
                // Drawing plane is world XY (= DXF XY).
                let in_win = |x: f64, y: f64| -> bool {
                    x >= win_min.x && x <= win_max.x && y >= win_min.y && y <= win_max.y
                };

                let dx = delta.x as f64;
                let dy = delta.y as f64; // drawing plane is world XY
                let dz = delta.z as f64;

                for handle in &handles {
                    let Some(entity) = self.tabs[i].scene.document.get_entity_mut(*handle) else {
                        continue;
                    };
                    let mut stretched = false;
                    match entity {
                        acadrust::EntityType::Line(l) => {
                            let s_in = in_win(l.start.x, l.start.y);
                            let e_in = in_win(l.end.x, l.end.y);
                            if s_in {
                                l.start.x += dx;
                                l.start.y += dy;
                                l.start.z += dz;
                                stretched = true;
                            }
                            if e_in {
                                l.end.x += dx;
                                l.end.y += dy;
                                l.end.z += dz;
                                stretched = true;
                            }
                        }
                        acadrust::EntityType::LwPolyline(p) => {
                            for v in &mut p.vertices {
                                if in_win(v.location.x, v.location.y) {
                                    v.location.x += dx;
                                    v.location.y += dy;
                                    stretched = true;
                                }
                            }
                        }
                        acadrust::EntityType::Polyline2D(p) => {
                            for v in &mut p.vertices {
                                if in_win(v.location.x, v.location.y) {
                                    v.location.x += dx;
                                    v.location.y += dy;
                                    stretched = true;
                                }
                            }
                        }
                        acadrust::EntityType::Polyline(p) => {
                            for v in &mut p.vertices {
                                if in_win(v.location.x, v.location.z) {
                                    v.location.x += dx;
                                    v.location.z += dy;
                                    stretched = true;
                                }
                            }
                        }
                        acadrust::EntityType::Arc(a) => {
                            if in_win(a.center.x, a.center.y) {
                                a.center.x += dx;
                                a.center.y += dy;
                                a.center.z += dz;
                                stretched = true;
                            }
                        }
                        acadrust::EntityType::Circle(c) => {
                            if in_win(c.center.x, c.center.y) {
                                c.center.x += dx;
                                c.center.y += dy;
                                c.center.z += dz;
                                stretched = true;
                            }
                        }
                        acadrust::EntityType::Ellipse(e) => {
                            if in_win(e.center.x, e.center.y) {
                                e.center.x += dx;
                                e.center.y += dy;
                                e.center.z += dz;
                                stretched = true;
                            }
                        }
                        acadrust::EntityType::Insert(ins) => {
                            if in_win(ins.insert_point.x, ins.insert_point.y) {
                                ins.insert_point.x += dx;
                                ins.insert_point.y += dy;
                                ins.insert_point.z += dz;
                                stretched = true;
                            }
                        }
                        acadrust::EntityType::Text(t) => {
                            if in_win(t.insertion_point.x, t.insertion_point.y) {
                                t.insertion_point.x += dx;
                                t.insertion_point.y += dy;
                                t.insertion_point.z += dz;
                                stretched = true;
                            }
                        }
                        acadrust::EntityType::MText(t) => {
                            if in_win(t.insertion_point.x, t.insertion_point.y) {
                                t.insertion_point.x += dx;
                                t.insertion_point.y += dy;
                                t.insertion_point.z += dz;
                                stretched = true;
                            }
                        }
                        _ => {
                            // Generic: move entire entity (treat as block-level)
                            stretched = false; // skip generic types
                        }
                    }
                    if stretched {
                        self.tabs[i].scene.mark_entity_dirty(*handle);
                        count += 1;
                    }
                }

                // Geometry was edited in place via get_entity_mut, which the
                // scene's tessellation cache doesn't observe. Re-tessellate the
                // moved entities so the viewport reflects the stretch right away
                // instead of only on the next unrelated redraw. See #95.
                if count > 0 {
                    self.tabs[i].scene.bump_geometry_no_blocks();
                }
                self.tabs[i].dirty = true;
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
                self.command_line
                    .push_output(&format!("STRETCH: {count} entity(ies) stretched."));
                self.refresh_properties();
            }
            // ── Solid3D creation (BOX / SPHERE / CYLINDER) ────────────────
            CmdResult::CommitSolid3D { mesh_fn } => {
                use crate::modules::insert::solid3d_cmds::empty_solid3d;
                self.push_undo_snapshot(i, "SOLID3D");
                let entity = empty_solid3d();
                let handle = self.tabs[i].scene.add_entity(entity);
                if !handle.is_null() {
                    let name = format!("{}", handle.value());
                    let color = [0.6f32, 0.6, 0.8, 1.0]; // default colour; command embedded it
                    let _ = color; // color is captured inside mesh_fn
                    if let Some(mesh) = mesh_fn(name) {
                        self.tabs[i].scene.meshes.insert(handle, crate::scene::MeshLodSet::from_single(mesh));
                    }
                    self.tabs[i].dirty = true;
                    self.command_line.push_output("Solid created.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }

            // ── EXTRUDE ────────────────────────────────────────────────────
            CmdResult::ExtrudeEntity {
                handle,
                height,
                color,
            } => {
                use crate::entities::traits::EntityTypeOps;
                use crate::modules::insert::solid3d_cmds::empty_solid3d;
                use crate::scene::convert::acad_to_truck::TruckObject;
                use crate::scene::convert::truck_tess;
                use truck_modeling::builder;
                use truck_modeling::Vector3 as TruckVec3;

                let entity_opt = self.tabs[i].scene.document.get_entity(handle).cloned();
                if let Some(entity) = entity_opt {
                    let truck_entity = entity.to_truck_entity(&self.tabs[i].scene.document);
                    let result = truck_entity.and_then(|te| {
                        match te.object {
                            TruckObject::Contour(wire) => {
                                // Attach a planar face to the wire profile, then sweep.
                                let face = builder::try_attach_plane(&[wire]).ok()?;
                                // tsweep(Face) → Solid
                                let solid =
                                    builder::tsweep(&face, TruckVec3::new(0.0, 0.0, height as f64));
                                match truck_tess::tessellate_solid(&solid) {
                                    truck_tess::TruckTessResult::Mesh {
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                    } => Some(crate::scene::model::mesh_model::MeshModel {
                                        name: String::new(),
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                        color,
                                        selected: false,
                                    }),
                                    _ => None,
                                }
                            }
                            _ => None,
                        }
                    });
                    if let Some(mut mesh) = result {
                        self.push_undo_snapshot(i, "EXTRUDE");
                        let new_entity = empty_solid3d();
                        let new_handle = self.tabs[i].scene.add_entity(new_entity);
                        mesh.name = format!("{}", new_handle.value());
                        self.tabs[i].scene.meshes.insert(new_handle, crate::scene::MeshLodSet::from_single(mesh));
                        self.tabs[i].dirty = true;
                        self.command_line.push_output("EXTRUDE: solid created.");
                    } else {
                        self.command_line.push_error("EXTRUDE: could not build profile. Select a closed 2D entity (Circle, LwPolyline, etc.).");
                    }
                } else {
                    self.command_line.push_error("EXTRUDE: entity not found.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }

            // ── REVOLVE ────────────────────────────────────────────────────
            CmdResult::RevolveEntity {
                handle,
                axis_start,
                axis_end,
                angle_deg,
                color,
            } => {
                use crate::entities::traits::EntityTypeOps;
                use crate::modules::insert::solid3d_cmds::empty_solid3d;
                use crate::scene::convert::acad_to_truck::TruckObject;
                use crate::scene::convert::truck_tess;
                use truck_modeling::builder;
                use truck_modeling::{Point3, Rad, Vector3 as TruckVec3};

                let entity_opt = self.tabs[i].scene.document.get_entity(handle).cloned();
                if let Some(entity) = entity_opt {
                    let truck_entity = entity.to_truck_entity(&self.tabs[i].scene.document);
                    let result = truck_entity.and_then(|te| {
                        let wire: Option<truck_modeling::Wire> = match te.object {
                            TruckObject::Contour(w) => Some(w),
                            TruckObject::Curve(e) => Some(std::iter::once(e).collect()),
                            _ => None,
                        };
                        let wire = wire?;
                        let origin = Point3::new(
                            axis_start.x as f64,
                            axis_start.y as f64,
                            axis_start.z as f64,
                        );
                        let dir = (axis_end - axis_start).normalize();
                        let axis = TruckVec3::new(dir.x as f64, dir.y as f64, dir.z as f64);
                        let shell = builder::rsweep(
                            &wire,
                            origin,
                            axis,
                            Rad(angle_deg.to_radians() as f64),
                        );
                        match truck_tess::tessellate_shell(&shell) {
                            truck_tess::TruckTessResult::Mesh {
                                verts,
                                verts_low,
                                normals,
                                indices,
                            } => Some(crate::scene::model::mesh_model::MeshModel {
                                name: String::new(),
                                verts,
                                verts_low,
                                normals,
                                indices,
                                color,
                                selected: false,
                            }),
                            _ => None,
                        }
                    });
                    if let Some(mut mesh) = result {
                        self.push_undo_snapshot(i, "REVOLVE");
                        let new_entity = empty_solid3d();
                        let new_handle = self.tabs[i].scene.add_entity(new_entity);
                        mesh.name = format!("{}", new_handle.value());
                        self.tabs[i].scene.meshes.insert(new_handle, crate::scene::MeshLodSet::from_single(mesh));
                        self.tabs[i].dirty = true;
                        self.command_line
                            .push_output(&format!("REVOLVE: solid created ({:.0}°).", angle_deg));
                    } else {
                        self.command_line
                            .push_error("REVOLVE: could not revolve profile.");
                    }
                } else {
                    self.command_line.push_error("REVOLVE: entity not found.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }

            // ── SWEEP ──────────────────────────────────────────────────────
            CmdResult::SweepEntity {
                profile_handle,
                path_handle,
                color,
            } => {
                use crate::entities::traits::EntityTypeOps;
                use crate::modules::insert::solid3d_cmds::empty_solid3d;
                use crate::scene::convert::acad_to_truck::TruckObject;
                use crate::scene::convert::truck_tess;
                use truck_modeling::builder;
                use truck_modeling::Vector3 as TruckVec3;

                let profile_ent = self.tabs[i]
                    .scene
                    .document
                    .get_entity(profile_handle)
                    .cloned();
                let path_ent = self.tabs[i].scene.document.get_entity(path_handle).cloned();

                let result = profile_ent.zip(path_ent).and_then(|(prof_e, path_e)| {
                    let prof_truck = prof_e.to_truck_entity(&self.tabs[i].scene.document)?;
                    let path_truck = path_e.to_truck_entity(&self.tabs[i].scene.document)?;

                    // Profile must be a wire (closed or open).
                    let profile_wire: truck_modeling::Wire = match prof_truck.object {
                        TruckObject::Contour(w) => w,
                        TruckObject::Curve(e) => std::iter::once(e).collect(),
                        _ => return None,
                    };

                    // Path determines the sweep operation.
                    let mesh = match path_truck.object {
                        // Linear path: translate profile along the line direction.
                        TruckObject::Curve(edge) => {
                            let p_start = edge.front().point();
                            let p_end = edge.back().point();
                            let dir = TruckVec3::new(
                                p_end.x - p_start.x,
                                p_end.y - p_start.y,
                                p_end.z - p_start.z,
                            );
                            // Try to build a face from the profile; if it's a closed
                            // wire we get a Solid, otherwise a Shell.
                            if let Ok(face) = builder::try_attach_plane(&[profile_wire.clone()]) {
                                let solid = builder::tsweep(&face, dir);
                                match truck_tess::tessellate_solid(&solid) {
                                    truck_tess::TruckTessResult::Mesh {
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                    } => Some(crate::scene::model::mesh_model::MeshModel {
                                        name: String::new(),
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                        color,
                                        selected: false,
                                    }),
                                    _ => None,
                                }
                            } else {
                                let shell = builder::tsweep(&profile_wire, dir);
                                match truck_tess::tessellate_shell(&shell) {
                                    truck_tess::TruckTessResult::Mesh {
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                    } => Some(crate::scene::model::mesh_model::MeshModel {
                                        name: String::new(),
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                        color,
                                        selected: false,
                                    }),
                                    _ => None,
                                }
                            }
                        }

                        // Contour path (polyline): sweep along the polyline using the
                        // first edge's direction as approximation (multi-segment sweep
                        // requires NURBS deformation — not supported here).
                        TruckObject::Contour(path_wire) => {
                            // Use start→end of the whole wire as translation vector.
                            let p_start = path_wire.front_vertex()?.point();
                            let p_end = path_wire.back_vertex()?.point();
                            let dir = TruckVec3::new(
                                p_end.x - p_start.x,
                                p_end.y - p_start.y,
                                p_end.z - p_start.z,
                            );
                            if let Ok(face) = builder::try_attach_plane(&[profile_wire.clone()]) {
                                let solid = builder::tsweep(&face, dir);
                                match truck_tess::tessellate_solid(&solid) {
                                    truck_tess::TruckTessResult::Mesh {
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                    } => Some(crate::scene::model::mesh_model::MeshModel {
                                        name: String::new(),
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                        color,
                                        selected: false,
                                    }),
                                    _ => None,
                                }
                            } else {
                                let shell = builder::tsweep(&profile_wire, dir);
                                match truck_tess::tessellate_shell(&shell) {
                                    truck_tess::TruckTessResult::Mesh {
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                    } => Some(crate::scene::model::mesh_model::MeshModel {
                                        name: String::new(),
                                        verts,
                                        verts_low,
                                        normals,
                                        indices,
                                        color,
                                        selected: false,
                                    }),
                                    _ => None,
                                }
                            }
                        }

                        _ => None,
                    };
                    mesh
                });

                if let Some(mut mesh) = result {
                    self.push_undo_snapshot(i, "SWEEP");
                    let new_entity = empty_solid3d();
                    let new_handle = self.tabs[i].scene.add_entity(new_entity);
                    mesh.name = format!("{}", new_handle.value());
                    self.tabs[i].scene.meshes.insert(new_handle, crate::scene::MeshLodSet::from_single(mesh));
                    self.tabs[i].dirty = true;
                    self.command_line.push_output("SWEEP: solid created.");
                } else {
                    self.command_line.push_error("SWEEP: could not sweep profile along path. Use a closed 2D profile and a Line or Polyline path.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }

            // ── LOFT ───────────────────────────────────────────────────────
            CmdResult::LoftEntities { handles, color } => {
                use crate::entities::traits::EntityTypeOps;
                use crate::modules::insert::solid3d_cmds::empty_solid3d;
                use crate::scene::convert::acad_to_truck::TruckObject;
                use crate::scene::convert::truck_tess;
                use truck_modeling::builder;

                // Collect wires from each profile.
                let mut wires: Vec<truck_modeling::Wire> = Vec::new();
                for h in &handles {
                    if let Some(ent) = self.tabs[i].scene.document.get_entity(*h).cloned() {
                        if let Some(te) = ent.to_truck_entity(&self.tabs[i].scene.document) {
                            let wire = match te.object {
                                TruckObject::Contour(w) => Some(w),
                                TruckObject::Curve(e) => Some(std::iter::once(e).collect()),
                                _ => None,
                            };
                            if let Some(w) = wire {
                                wires.push(w);
                            }
                        }
                    }
                }

                let result: Option<crate::scene::model::mesh_model::MeshModel> = (|| {
                    if wires.len() < 2 {
                        return None;
                    }

                    // Build ruled shells between consecutive profile pairs.
                    let mut all_faces: Vec<truck_modeling::Face> = Vec::new();

                    for pair in wires.windows(2) {
                        let shell = builder::try_wire_homotopy(&pair[0], &pair[1]).ok()?;
                        for face in shell.into_iter() {
                            all_faces.push(face);
                        }
                    }

                    // Cap the first and last profiles if they are closed.
                    if let Ok(cap) = builder::try_attach_plane(&[wires.first()?.clone()]) {
                        all_faces.push(cap);
                    }
                    if let Ok(cap) = builder::try_attach_plane(&[wires.last()?.clone()]) {
                        all_faces.push(cap);
                    }

                    let shell = truck_modeling::Shell::from(all_faces);
                    match truck_tess::tessellate_shell(&shell) {
                        truck_tess::TruckTessResult::Mesh {
                            verts,
                            verts_low,
                            normals,
                            indices,
                        } => Some(crate::scene::model::mesh_model::MeshModel {
                            name: String::new(),
                            verts,
                            verts_low,
                            normals,
                            indices,
                            color,
                            selected: false,
                        }),
                        _ => None,
                    }
                })();

                if let Some(mut mesh) = result {
                    self.push_undo_snapshot(i, "LOFT");
                    let new_entity = empty_solid3d();
                    let new_handle = self.tabs[i].scene.add_entity(new_entity);
                    mesh.name = format!("{}", new_handle.value());
                    self.tabs[i].scene.meshes.insert(new_handle, crate::scene::MeshLodSet::from_single(mesh));
                    self.tabs[i].dirty = true;
                    self.command_line.push_output(&format!(
                        "LOFT: solid created from {} profiles.",
                        handles.len()
                    ));
                } else {
                    self.command_line.push_error("LOFT: could not loft profiles. Ensure sections have the same edge count and are compatible.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }

            CmdResult::HatcheditApply {
                handle,
                name,
                scale,
                angle,
            } => {
                if let Some(mut model) = self.tabs[i].scene.hatches.get(&handle).cloned() {
                    // Update model fields
                    if !name.is_empty() {
                        use crate::scene::model::hatch_model::HatchPattern;
                        use crate::scene::model::hatch_patterns;
                        model.name = name.clone();
                        if name.to_uppercase() == "SOLID" {
                            model.pattern = HatchPattern::Solid;
                        } else if let Some(entry) = hatch_patterns::find(&name) {
                            model.pattern = entry.gpu.clone();
                        }
                        // If not found in catalog, keep existing pattern type
                    }
                    model.scale = scale;
                    model.angle_offset = angle;

                    self.push_undo_snapshot(i, "HATCHEDIT");
                    // Remove old hatch (entity + GPU model)
                    self.tabs[i].scene.erase_entities(&[handle]);
                    // Re-add with updated model
                    self.tabs[i].scene.add_hatch(model);
                    self.tabs[i].dirty = true;
                    self.command_line.push_output("HATCHEDIT: hatch updated.");
                } else {
                    self.command_line
                        .push_error("HATCHEDIT: hatch entity not found.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }
            CmdResult::OpenMTextEditor {
                pos,
                handle,
                initial,
                height,
            } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.open_mtext_editor(pos.as_vec3(), handle, &initial, height);
            }
            CmdResult::OpenTextEditor {
                pos,
                handle,
                initial,
                height,
            } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.open_text_inline(
                    pos.as_vec3(),
                    handle,
                    &initial,
                    height,
                    super::text_inline::TextEntityField::Text,
                );
            }
            CmdResult::EditTextEntity { handle } => {
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
                self.ribbon.deactivate_tool();
                return self.begin_text_edit(handle);
            }
            CmdResult::SuspendForTextEdit { handle } => {
                let is_editable = crate::app::text_inline::can_edit_text(handle, &self.tabs[i].scene.document);
                if !is_editable {
                    self.command_line.push_error("TEXTEDIT: selected entity is not text.");
                    let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                    if let Some(p) = prompt {
                        self.command_line.push_info(&p);
                    }
                    return Task::none();
                }
                let cmd = self.tabs[i].active_cmd.take();
                self.tabs[i].suspended_cmd = cmd;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
                self.ribbon.deactivate_tool();
                return self.begin_text_edit(handle);
            }
            CmdResult::UndoDocument => {
                let active = self.tabs[i].active_cmd.take();
                self.undo_active_tab();
                self.tabs[i].active_cmd = active;
                let prompt = self.tabs[i].active_cmd.as_ref().map(|c| c.prompt());
                if let Some(p) = prompt {
                    self.command_line.push_info(&p);
                }
            }
            CmdResult::SetTexteditMode(val) => {
                self.texteditmode = val;
                let display_val = if val { 1 } else { 0 };
                self.command_line.push_output(&format!("TEXTEDITMODE set to {display_val}"));
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
            }
            CmdResult::DdeditEntity { handle, new_text } => {
                let mut updated = false;
                if let Some(entity) = self.tabs[i].scene.document.get_entity_mut(handle) {
                    match entity {
                        acadrust::EntityType::Text(t) => {
                            t.value = new_text;
                            updated = true;
                        }
                        acadrust::EntityType::MText(t) => {
                            t.value = new_text;
                            updated = true;
                        }
                        acadrust::EntityType::AttributeDefinition(a) => {
                            a.default_value = new_text;
                            updated = true;
                        }
                        acadrust::EntityType::AttributeEntity(a) => {
                            a.set_value(new_text);
                            updated = true;
                        }
                        acadrust::EntityType::Dimension(d) => {
                            // Empty string resets to auto-measured value; otherwise set override.
                            let base = d.base_mut();
                            base.text = new_text;
                            updated = true;
                        }
                        _ => {}
                    }
                }
                if updated {
                    self.push_undo_snapshot(i, "DDEDIT");
                    self.tabs[i].dirty = true;
                    self.command_line.push_output("DDEDIT: text updated.");
                } else {
                    self.command_line
                        .push_error("DDEDIT: entity type not supported.");
                }
                self.tabs[i].active_cmd = None;
                self.tabs[i].snap_result = None;
                self.tabs[i].scene.clear_preview_wire();
                self.restore_pre_cmd_tangent();
            }
        }
        // Keep the command-line input focused at all times — every typed
        // character is meant to route there (the command processor reads
        // its keystroke stream from this widget). When no command is
        // running the ribbon tool button still has to visually deactivate.
        if self.tabs[i].active_cmd.is_none() {
            self.ribbon.deactivate_tool();
        }
        // The in-place TEXT editor needs keyboard focus on its own field.
        if self.text_inline.is_some() {
            return Task::batch([
                task,
                iced::widget::operation::focus(iced::widget::Id::new(
                    super::view::TEXT_INLINE_ID,
                )),
            ]);
        }
        Task::batch([task, self.focus_cmd_input()])
    }

    /// Restore the tangent-snap / ortho state that was in effect before the command started.
    /// Recreate clipboard-dependency records (layer / linetype / text + dim
    /// style) in tab `i`'s document for any the copied entities reference but
    /// this drawing doesn't already have. Each recreated record gets a fresh
    /// handle from the target document so it can't collide with an existing
    /// one. No-op for same-document pastes (the records already exist). (#129)
    pub(super) fn merge_clipboard_deps(&mut self, i: usize) {
        use acadrust::TableEntry;
        if self.clipboard_deps.is_empty() {
            return;
        }
        let doc = &mut self.tabs[i].scene.document;
        for rec in &self.clipboard_deps.layers {
            if !doc.layers.contains(rec.name()) {
                let mut r = rec.clone();
                r.set_handle(doc.allocate_handle());
                let _ = doc.layers.add(r);
            }
        }
        for rec in &self.clipboard_deps.linetypes {
            if !doc.line_types.contains(rec.name()) {
                let mut r = rec.clone();
                r.set_handle(doc.allocate_handle());
                let _ = doc.line_types.add(r);
            }
        }
        for rec in &self.clipboard_deps.text_styles {
            if !doc.text_styles.contains(rec.name()) {
                let mut r = rec.clone();
                r.set_handle(doc.allocate_handle());
                let _ = doc.text_styles.add(r);
            }
        }
        for rec in &self.clipboard_deps.dim_styles {
            if !doc.dim_styles.contains(rec.name()) {
                let mut r = rec.clone();
                r.set_handle(doc.allocate_handle());
                let _ = doc.dim_styles.add(r);
            }
        }
    }

    /// Recreate any block definition the pasted INSERTs reference but tab
    /// `i`'s document lacks (cross-drawing paste), so the block reference
    /// renders its geometry instead of nothing. No-op for same-document
    /// pastes. (#135)
    pub(super) fn merge_clipboard_blocks(&mut self, i: usize) {
        if self.clipboard_deps.blocks.is_empty() {
            return;
        }
        let blocks = self.clipboard_deps.blocks.clone();
        for def in blocks {
            if self.tabs[i].scene.document.block_records.get(&def.name).is_some() {
                continue;
            }
            self.tabs[i]
                .scene
                .define_block_raw(&def.name, def.base_point, def.entities);
        }
    }

    /// Shared paste finalize for every paste path (PASTECLIP, PASTEORIG):
    /// recreate the clipboard's dependency records and block definitions, add
    /// each entity with fresh handles (optionally transformed), recreate each
    /// entity's xdictionary graph (XCLIP filters etc.), and tessellate pasted
    /// solids. Returns the new handles, index-aligned with the clipboard
    /// (NULL where an add failed). Keeping this in one place means a new
    /// cross-drawing concern is wired once, not re-implemented per command.
    pub(super) fn finalize_paste(
        &mut self,
        i: usize,
        translate: Option<crate::command::EntityTransform>,
    ) -> Vec<Handle> {
        self.merge_clipboard_deps(i);
        self.merge_clipboard_blocks(i);
        let by_index: Vec<Handle> = self
            .clipboard
            .clone()
            .into_iter()
            .map(|mut entity| {
                if let Some(t) = &translate {
                    crate::scene::view::dispatch::apply_transform(&mut entity, t);
                }
                self.tabs[i].scene.add_entity_clone(entity)
            })
            .collect();
        self.merge_clipboard_ext_objects(i, &by_index);
        self.tabs[i].scene.populate_meshes_from_document();
        by_index
    }

    /// Recreate the extension-dictionary object graph (XCLIP spatial filters,
    /// attached XRecords, …) captured for each copied entity, cloning every
    /// object into this document with fresh handles, remapping all internal
    /// references, and re-pointing the pasted entity's `xdictionary_handle` at
    /// the new root. `by_index` is the paste's new entity handles, aligned with
    /// the clipboard order (NULL where the add failed). No-op without captures.
    pub(super) fn merge_clipboard_ext_objects(&mut self, i: usize, by_index: &[Handle]) {
        if self.clipboard_deps.ext_objects.is_empty() {
            return;
        }
        let captures = self.clipboard_deps.ext_objects.clone();
        let doc = &mut self.tabs[i].scene.document;
        for cap in &captures {
            let Some(&new_entity) = by_index.get(cap.entity_index) else {
                continue;
            };
            if new_entity.is_null() {
                continue;
            }
            if let Some(new_root) = recreate_ext_subtree(doc, cap, Some(new_entity)) {
                if let Some(e) = doc.get_entity_mut(new_entity) {
                    e.common_mut().xdictionary_handle = Some(new_root);
                }
            }
        }
        // The wires were tessellated before the filters existed; force a rebuild
        // so the clip is applied to the freshly-pasted, now-filtered inserts.
        self.tabs[i].scene.bump_geometry();
    }

    /// Recreate the captured xdictionary subtrees in this document (fresh
    /// handles, remapped references) WITHOUT an added host entity, returning
    /// `entity_index → new xdictionary root`. Used by PASTEBLOCK, which folds
    /// the clipboard into a new block definition: the caller stamps each new
    /// root onto the matching entity's `xdictionary_handle` before defining the
    /// block, so the block's nested insert keeps its XCLIP filter.
    pub(super) fn recreate_clipboard_ext_roots(
        &mut self,
        i: usize,
    ) -> std::collections::HashMap<usize, Handle> {
        let mut out = std::collections::HashMap::new();
        if self.clipboard_deps.ext_objects.is_empty() {
            return out;
        }
        let captures = self.clipboard_deps.ext_objects.clone();
        let doc = &mut self.tabs[i].scene.document;
        for cap in &captures {
            if let Some(new_root) = recreate_ext_subtree(doc, cap, None) {
                out.insert(cap.entity_index, new_root);
            }
        }
        out
    }

    fn restore_pre_cmd_tangent(&mut self) {
        if let Some(was_on) = self.pre_cmd_tangent.take() {
            if !was_on {
                self.snapper.enabled.remove(&crate::snap::SnapType::Tangent);
            }
        }
        if self.rect_suppressed_ortho {
            self.rect_suppressed_ortho = false;
            self.ortho_mode = true;
            self.polar_mode = false;
        }
    }
}

/// Clone one captured xdictionary subtree into `doc` with fresh handles,
/// remapping every internal reference (and the owning entity, when known),
/// returning the new root handle. `allocate_handle` advances the document's
/// handle counter — `next_handle()` only peeks, so reusing it would hand every
/// object the same handle and collapse the dictionary chain.
fn recreate_ext_subtree(
    doc: &mut acadrust::CadDocument,
    cap: &crate::app::ClipExtObjects,
    entity_handle: Option<Handle>,
) -> Option<Handle> {
    use std::collections::HashMap;
    let mut remap: HashMap<Handle, Handle> = HashMap::new();
    if let Some(eh) = entity_handle {
        remap.insert(cap.src_entity_handle, eh);
    }
    for (old, _) in &cap.objects {
        remap.insert(*old, doc.allocate_handle());
    }
    for (old, obj) in &cap.objects {
        let mut obj = obj.clone();
        let new_h = remap[old];
        remap_object(&mut obj, new_h, &remap);
        doc.objects.insert(new_h, obj);
    }
    remap.get(&cap.root).copied()
}

/// Rewrite a cloned extension-dictionary object onto fresh handles: set its own
/// handle to `new_handle` and remap its owner and any handle references it holds
/// through `remap` (a handle still in the source space stays unchanged, which is
/// correct for cross-references that point outside the captured subtree).
fn remap_object(
    obj: &mut acadrust::objects::ObjectType,
    new_handle: acadrust::Handle,
    remap: &std::collections::HashMap<acadrust::Handle, acadrust::Handle>,
) {
    use acadrust::objects::ObjectType;
    let map = |h: acadrust::Handle| remap.get(&h).copied().unwrap_or(h);
    match obj {
        ObjectType::Dictionary(d) => {
            d.handle = new_handle;
            d.owner = map(d.owner);
            for (_, h) in d.entries.iter_mut() {
                *h = map(*h);
            }
            if let Some(x) = d.xdictionary_handle.as_mut() {
                *x = map(*x);
            }
            for r in d.reactors.iter_mut() {
                *r = map(*r);
            }
        }
        ObjectType::DictionaryWithDefault(d) => {
            d.handle = new_handle;
            d.owner = map(d.owner);
            for (_, h) in d.entries.iter_mut() {
                *h = map(*h);
            }
            d.default_handle = map(d.default_handle);
        }
        ObjectType::DictionaryVariable(v) => {
            v.handle = new_handle;
            v.owner_handle = map(v.owner_handle);
        }
        ObjectType::SpatialFilter(s) => {
            s.handle = new_handle;
            s.owner = map(s.owner);
        }
        ObjectType::XRecord(x) => {
            x.handle = new_handle;
            x.owner = map(x.owner);
        }
        ObjectType::Group(g) => {
            g.handle = new_handle;
            g.owner = map(g.owner);
            for h in g.entities.iter_mut() {
                *h = map(*h);
            }
        }
        // Other leaf object kinds don't appear in an entity xdictionary; if one
        // does, it's inserted with the fresh handle below via the caller's key,
        // but its internal owner is left as-is (best effort).
        _ => {}
    }
}

// ── DIMSPACE helper ───────────────────────────────────────────────────────────

/// Parse `base_val,h1;h2;...;hN,spacing` and adjust parallel dimension positions.
fn apply_dimspace(scene: &mut crate::scene::Scene, encoded: &str) {
    // Format: "<base_handle>,<h1>;<h2>;...;<hN>,<spacing>"
    let parts: Vec<&str> = encoded.splitn(3, ',').collect();
    if parts.len() < 3 {
        return;
    }
    let base_val: u64 = parts[0].parse().unwrap_or(0);
    let other_vals: Vec<u64> = parts[1]
        .split(';')
        .filter_map(|s| s.parse::<u64>().ok())
        .collect();
    let spacing: f64 = parts[2].parse().unwrap_or(0.0);

    use acadrust::entities::Dimension;
    let base_h = acadrust::Handle::from(base_val);
    // Base dim: the perpendicular direction (from its rotation / axis) and the
    // dim line's perpendicular coordinate. Spacing steps each parallel dim along
    // this perp IN THE DRAWING PLANE — offsetting Z had no effect on the dim
    // line, which is computed from def·perp with perp.z = 0. (#181 / DIM-021)
    let (perp, base_coord) = match scene.document.get_entity(base_h) {
        Some(acadrust::EntityType::Dimension(Dimension::Linear(d))) => {
            let (s, c) = d.rotation.sin_cos();
            let perp = (-s, c);
            let dp = d.definition_point;
            (perp, dp.x * perp.0 + dp.y * perp.1)
        }
        Some(acadrust::EntityType::Dimension(Dimension::Aligned(d))) => {
            let dx = d.second_point.x - d.first_point.x;
            let dy = d.second_point.y - d.first_point.y;
            let len = (dx * dx + dy * dy).sqrt().max(1e-12);
            let perp = (-dy / len, dx / len);
            let dp = d.definition_point;
            (perp, dp.x * perp.0 + dp.y * perp.1)
        }
        _ => return,
    };

    let effective_spacing = if spacing <= 0.0 { 10.0 } else { spacing };
    for (idx, &hv) in other_vals.iter().enumerate() {
        let h = acadrust::Handle::from(hv);
        let target = base_coord + effective_spacing * (idx + 1) as f64;
        if let Some(acadrust::EntityType::Dimension(dim)) = scene.document.get_entity_mut(h) {
            // Slide this dim's definition point along perp so its perpendicular
            // coordinate equals `target`; update both the struct field (render)
            // and base (save).
            let slide = |p: &mut acadrust::types::Vector3| {
                let cur = p.x * perp.0 + p.y * perp.1;
                let delta = target - cur;
                p.x += perp.0 * delta;
                p.y += perp.1 * delta;
            };
            match dim {
                Dimension::Linear(d) => {
                    slide(&mut d.definition_point);
                    d.base.definition_point = d.definition_point;
                }
                Dimension::Aligned(d) => {
                    slide(&mut d.definition_point);
                    d.base.definition_point = d.definition_point;
                }
                _ => {}
            }
        }
    }
    scene.bump_geometry();
}

// ── MLEADERALIGN helper ───────────────────────────────────────────────────────

/// Parse `h1,h2,...;fx,fz;tx,tz` and align multileader content points along the direction.
fn apply_mleader_align(scene: &mut crate::scene::Scene, encoded: &str) {
    // Format: "<h1>,<h2>,...;<fx>,<fz>;<tx>,<tz>"
    let parts: Vec<&str> = encoded.splitn(3, ';').collect();
    if parts.len() < 3 {
        return;
    }
    let handles: Vec<acadrust::Handle> = parts[0]
        .split(',')
        .filter_map(|s| s.parse::<u64>().ok().map(acadrust::Handle::from))
        .collect();
    let from_parts: Vec<f64> = parts[1].split(',').filter_map(|s| s.parse().ok()).collect();
    let to_parts: Vec<f64> = parts[2].split(',').filter_map(|s| s.parse().ok()).collect();
    if from_parts.len() < 2 || to_parts.len() < 2 || handles.is_empty() {
        return;
    }

    let fx = from_parts[0];
    let fz = from_parts[1];
    let tx = to_parts[0];
    let tz = to_parts[1];
    let dx = tx - fx;
    let dz = tz - fz;
    let len = (dx * dx + dz * dz).sqrt();
    if len < 1e-9 {
        return;
    }

    // Project each multileader's content point onto the alignment line, then
    // snap it to the line (preserve perpendicular offset from line is discarded;
    // align along direction through `from`).
    for h in handles {
        if let Some(acadrust::EntityType::MultiLeader(ml)) = scene.document.get_entity_mut(h) {
            let cp = &mut ml.context.content_base_point;
            // Project onto line from_pt + t * dir: keep t component, set perpendicular = 0
            let rel_x = cp.x - fx;
            let rel_z = cp.z - fz;
            let t = (rel_x * (dx / len) + rel_z * (dz / len)) / len;
            let t = t.clamp(0.0, 1.0);
            cp.x = fx + t * dx;
            cp.z = fz + t * dz;
        }
    }
}

// ── MLEADERCOLLECT helper ─────────────────────────────────────────────────────

/// Parse `h1,h2,...;px,pz` — merge all selected multileaders into the first one at position.
fn apply_mleader_collect(scene: &mut crate::scene::Scene, encoded: &str) {
    let parts: Vec<&str> = encoded.splitn(2, ';').collect();
    if parts.len() < 2 {
        return;
    }
    let handles: Vec<acadrust::Handle> = parts[0]
        .split(',')
        .filter_map(|s| s.parse::<u64>().ok().map(acadrust::Handle::from))
        .collect();
    let pos_parts: Vec<f64> = parts[1].split(',').filter_map(|s| s.parse().ok()).collect();
    if handles.len() < 2 || pos_parts.len() < 2 {
        return;
    }

    let px = pos_parts[0];
    let pz = pos_parts[1];

    // Collect all leader roots from the secondary multileaders.
    let mut extra_roots: Vec<acadrust::entities::LeaderRoot> = Vec::new();
    for &h in &handles[1..] {
        if let Some(acadrust::EntityType::MultiLeader(ml)) = scene.document.get_entity(h) {
            extra_roots.extend(ml.context.leader_roots.iter().cloned());
        }
    }

    // Add collected roots to the first multileader and move its content point.
    if let Some(acadrust::EntityType::MultiLeader(ml)) = scene.document.get_entity_mut(handles[0]) {
        ml.context.content_base_point.x = px;
        ml.context.content_base_point.z = pz;
        for root in extra_roots {
            ml.context.leader_roots.push(root);
        }
    }

    // Erase the secondary multileaders.
    scene.erase_entities(&handles[1..]);
}
