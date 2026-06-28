use super::*;

impl OpenCADStudio {
    pub(super) fn dispatch_inquiry(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            "3DORBIT" | "3O" => {
                self.command_line
                    .push_info("3D Orbit: drag with right mouse button.");
            }

            // ── Selection utilities ───────────────────────────────────────
            "SELECTALL" | "SA" => {
                use crate::scene::Scene;
                let handles: Vec<acadrust::Handle> = self.tabs[i]
                    .scene
                    .entity_wires()
                    .iter()
                    .filter_map(|w| Scene::handle_from_wire_name(&w.name))
                    .collect();
                let count = handles.len();
                for h in handles {
                    self.tabs[i].scene.select_entity(h, false);
                }
                self.command_line
                    .push_output(&format!("SELECTALL: {} object(s) selected.", count));
                self.refresh_properties();
            }

            "DESELECT" | "DE" | "DESELALL" => {
                self.tabs[i].scene.deselect_all();
                self.command_line.push_output("Deselected.");
                self.refresh_properties();
            }

            "SELECTSIMILAR" | "SELSIM" => {
                let added = self.tabs[i].scene.select_similar();
                self.command_line
                    .push_output(&format!("Select Similar: {} added.", added));
                self.refresh_properties();
            }

            "QSELECT" | "QS" => {
                return Some(Task::done(Message::QSelectOpen));
            }

            // ── LIST — entity info ────────────────────────────────────────
            "LIST" | "LI" => {
                let selected: Vec<_> = self.tabs[i].scene.selected_entities();
                if selected.is_empty() {
                    self.command_line
                        .push_error("LIST: no entities selected. Select entities first.");
                } else {
                    for (handle, _) in &selected {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity(*handle) {
                            let type_name = crate::entities::names::dxf_name(entity);
                            let common = entity.common();
                            let color_str = common
                                .color
                                .index()
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "ByLayer".to_string());
                            let linetype =
                                if common.linetype.is_empty() || common.linetype == "ByLayer" {
                                    "ByLayer".to_string()
                                } else {
                                    common.linetype.clone()
                                };
                            // Entity-specific details
                            let details = entity_list_details(entity);
                            self.command_line.push_output(&format!(
                                "{type_name}  Handle:{:X}  Layer:{}  Color:{}  LT:{}{}",
                                handle.value(),
                                common.layer,
                                color_str,
                                linetype,
                                if details.is_empty() {
                                    String::new()
                                } else {
                                    format!("\n    {details}")
                                }
                            ));
                        }
                    }
                }
            }

            // DBLIST — list data for every entity in the drawing (LIST over the
            // whole database rather than the current selection).
            "DBLIST" => {
                // Format every entity first so the immutable document borrow is
                // released before writing to the command line.
                let lines: Vec<String> = self.tabs[i]
                    .scene
                    .document
                    .entities()
                    .map(|entity| {
                        let type_name = crate::entities::names::dxf_name(entity);
                        let common = entity.common();
                        let color_str = common
                            .color
                            .index()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "ByLayer".to_string());
                        let linetype =
                            if common.linetype.is_empty() || common.linetype == "ByLayer" {
                                "ByLayer".to_string()
                            } else {
                                common.linetype.clone()
                            };
                        let details = entity_list_details(entity);
                        format!(
                            "{type_name}  Handle:{:X}  Layer:{}  Color:{}  LT:{}{}",
                            common.handle.value(),
                            common.layer,
                            color_str,
                            linetype,
                            if details.is_empty() {
                                String::new()
                            } else {
                                format!("\n    {details}")
                            }
                        )
                    })
                    .collect();
                if lines.is_empty() {
                    self.command_line.push_info("DBLIST: drawing has no entities.");
                } else {
                    let count = lines.len();
                    for l in lines {
                        self.command_line.push_output(&l);
                    }
                    self.command_line.push_output(&format!(
                        "DBLIST: {count} entit{}.",
                        if count == 1 { "y" } else { "ies" }
                    ));
                }
            }

            // ── Break / Join ─────────────────────────────────────────────────
            "JOIN" | "J" => {
                use crate::modules::draw::modify::join::JoinCommand;
                let cmd = JoinCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "BREAK" | "BR" => {
                use crate::modules::draw::modify::break_cmd::BreakInteractiveCommand;
                let cmd = BreakInteractiveCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "BREAKATPOINT" | "BAP" => {
                use crate::modules::draw::modify::break_cmd::BreakAtPointCommand;
                let cmd = BreakAtPointCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "PEDIT" | "PE" => {
                use crate::modules::draw::modify::pedit::PeditCommand;
                let cmd_obj = PeditCommand::new();
                self.command_line.push_info(&cmd_obj.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd_obj));
            }

            "SPLINEDIT" | "SPE" => {
                use crate::modules::draw::modify::splinedit::SplineditCommand;
                let cmd_obj = SplineditCommand::new();
                self.command_line.push_info(&cmd_obj.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd_obj));
            }

            // Bare ATTEDIT (and the ATE alias) launch the interactive attribute
            // editor; the command-line -ATTEDIT form is handled in the draw family.
            "ATTEDIT" | "ATE" => {
                use crate::modules::draw::modify::attedit::AtteditCommand;
                let cmd_obj = AtteditCommand::new();
                self.command_line.push_info(&cmd_obj.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd_obj));
            }

            // ── REFEDIT — in-place block editing ─────────────────────────────
            "REFEDIT" => {
                use crate::modules::draw::modify::refedit::RefEditPickCommand;
                // If a session is already active, tell the user.
                if self.tabs[i].refedit_session.is_some() {
                    self.command_line
                        .push_error("REFEDIT: a session is already active. Use REFCLOSE first.");
                } else {
                    // Check if a single INSERT is already selected.
                    let selected: Vec<_> =
                        self.tabs[i].scene.selected_entities().into_iter().collect();
                    if selected.len() == 1 {
                        if let Some(acadrust::EntityType::Insert(_)) =
                            selected.first().map(|(_, e)| e)
                        {
                            let handle = selected[0].0;
                            // Skip pick phase — jump straight to begin.
                            let _ =
                                self.dispatch_command(&format!("REFEDIT_BEGIN:{}", handle.value()));
                            return Some(Task::none());
                        }
                    }
                    let cmd_obj = RefEditPickCommand::new();
                    self.command_line.push_info(&cmd_obj.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd_obj));
                }
            }

            cmd if cmd.starts_with("REFEDIT_BEGIN:") => {
                use crate::modules::draw::modify::refedit::{
                    apply_insert_transform, RefEditSession,
                };
                use acadrust::Handle;

                let handle_u64: u64 = cmd["REFEDIT_BEGIN:".len()..].parse().unwrap_or(0);
                let insert_handle = Handle::new(handle_u64);

                // Get INSERT entity.
                let insert = match self.tabs[i].scene.document.get_entity(insert_handle) {
                    Some(acadrust::EntityType::Insert(ins)) => ins.clone(),
                    _ => {
                        self.command_line
                            .push_error("REFEDIT: selected object is not an INSERT.");
                        return Some(Task::none());
                    }
                };

                // Build the INSERT's full placement transform (OCS + rotation +
                // scale, including non-uniform / mirrored) and its inverse, so
                // edits round-trip back to block-local coordinates on SAVE.
                let sx = insert.x_scale();
                let sy = insert.y_scale();
                let sz = insert.z_scale();
                let forward = insert.get_transform();
                let inverse = {
                    use acadrust::types::{Matrix3, Matrix4, Transform};
                    let ocs_t =
                        Matrix4::from_matrix3(Matrix3::arbitrary_axis(insert.normal).transpose());
                    let t_inv = Matrix4::translation(
                        -insert.insert_point.x,
                        -insert.insert_point.y,
                        -insert.insert_point.z,
                    );
                    let r_inv = Matrix4::rotation_z(-insert.rotation);
                    let s_inv = Matrix4::scaling(1.0 / sx, 1.0 / sy, 1.0 / sz);
                    // inverse(OCS·T·R·S) = S⁻¹·R⁻¹·T⁻¹·OCSᵀ
                    Transform::from_matrix(s_inv * r_inv * t_inv * ocs_t)
                };

                // Find the block record.
                let br_handle = match self.tabs[i]
                    .scene
                    .document
                    .block_records
                    .get(&insert.block_name)
                {
                    Some(br) => br.handle,
                    None => {
                        self.command_line.push_error(&format!(
                            "REFEDIT: block \"{}\" not found.",
                            insert.block_name
                        ));
                        return Some(Task::none());
                    }
                };

                // Collect block-local entities (skip structural Block/BlockEnd/AttDef).
                let block_entities: Vec<_> = {
                    let br = self.tabs[i]
                        .scene
                        .document
                        .block_records
                        .get(&insert.block_name)
                        .unwrap();
                    br.entity_handles
                        .iter()
                        .filter_map(|h| self.tabs[i].scene.document.get_entity(*h).cloned())
                        .filter(|e| {
                            !matches!(
                                e,
                                acadrust::EntityType::Block(_)
                                    | acadrust::EntityType::BlockEnd(_)
                                    | acadrust::EntityType::AttributeDefinition(_)
                            )
                        })
                        .collect()
                };

                if block_entities.is_empty() {
                    self.command_line.push_error("REFEDIT: block is empty.");
                    return Some(Task::none());
                }

                let session = RefEditSession {
                    block_name: insert.block_name.clone(),
                    br_handle,
                    temp_handles: vec![],
                    forward,
                    inverse,
                };

                self.push_undo_snapshot(i, "REFEDIT");
                self.tabs[i].refedit_session = Some(session.clone());

                // Add block entities to model space with INSERT transform applied.
                let mut temp_handles = Vec::new();
                for mut entity in block_entities {
                    apply_insert_transform(&mut entity, &session);
                    entity.common_mut().handle = acadrust::Handle::NULL;
                    entity.common_mut().owner_handle = acadrust::Handle::NULL;
                    let h = self.tabs[i].scene.add_entity(entity);
                    temp_handles.push(h);
                }
                self.tabs[i].refedit_session.as_mut().unwrap().temp_handles = temp_handles.clone();

                // Fade everything except the entities being edited, so the
                // surrounding drawing stays visible for context but the block's
                // geometry stands out. (#136)
                self.tabs[i]
                    .scene
                    .set_refedit_keep(Some(temp_handles.iter().copied().collect()));

                // Select the temp entities so user can see what they're editing.
                self.tabs[i].scene.deselect_all();
                for h in &temp_handles {
                    self.tabs[i].scene.select_entity(*h, false);
                }
                self.tabs[i].dirty = true;

                // No active command — the user edits the block's geometry freely
                // (move, grips, draw, erase…) and runs REFCLOSE when done. (#136)
                self.tabs[i].active_cmd = None;
                self.command_line.push_info(&format!(
                    "REFEDIT: Editing block \"{}\". Run REFCLOSE to save, REFCLOSE_DISCARD to cancel.",
                    insert.block_name
                ));
            }

            "REFCLOSE" => {
                if self.tabs[i].refedit_session.is_some() {
                    use crate::modules::draw::modify::refedit::RefCloseCommand;
                    let cmd_obj = RefCloseCommand::new();
                    self.command_line.push_info(&cmd_obj.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd_obj));
                } else {
                    self.command_line
                        .push_error("REFCLOSE: no REFEDIT session active.");
                }
            }

            "REFCLOSE_SAVE" => {
                use crate::modules::draw::modify::explode::normalize_entity_for_block;
                use crate::modules::draw::modify::refedit::apply_insert_inverse_transform;

                let session = match self.tabs[i].refedit_session.take() {
                    Some(s) => s,
                    None => {
                        self.command_line
                            .push_error("REFCLOSE: no REFEDIT session active.");
                        return Some(Task::none());
                    }
                };

                self.push_undo_snapshot(i, "REFCLOSE");

                // Collect the edited temp entities.
                let new_entities: Vec<acadrust::EntityType> = session
                    .temp_handles
                    .iter()
                    .filter_map(|h| self.tabs[i].scene.document.get_entity(*h).cloned())
                    .collect();

                // Remove temp entities from model space.
                self.tabs[i].scene.erase_entities(&session.temp_handles);

                // Apply inverse INSERT transform → block-local coordinates.
                let new_entities: Vec<_> = new_entities
                    .into_iter()
                    .map(|mut entity| {
                        apply_insert_inverse_transform(&mut entity, &session);
                        let mut entity = normalize_entity_for_block(entity);
                        entity.common_mut().handle = acadrust::Handle::NULL;
                        entity.common_mut().owner_handle = session.br_handle;
                        entity
                    })
                    .collect();

                // Remove old block entities from the document.
                let old_handles: Vec<_> = match self.tabs[i]
                    .scene
                    .document
                    .block_records
                    .get(&session.block_name)
                {
                    Some(br) => br.entity_handles.clone(),
                    None => vec![],
                };
                for h in &old_handles {
                    self.tabs[i].scene.document.remove_entity(*h);
                }
                // Flush the entity_handles list from the block record.
                if let Some(br) = self.tabs[i]
                    .scene
                    .document
                    .block_records
                    .get_mut(&session.block_name)
                {
                    br.entity_handles.clear();
                }

                // Add the new block entities.
                for entity in new_entities {
                    let _ = self.tabs[i].scene.document.add_entity(entity);
                }

                self.tabs[i].dirty = true;
                self.command_line.push_output(&format!(
                    "REFCLOSE: Block \"{}\" saved. All references updated.",
                    session.block_name
                ));
                // End the edit fade before rebuilding, so the restored geometry
                // recolours bright. (#136)
                self.tabs[i].scene.set_refedit_keep(None);
                // Rebuild hatch/image/mesh caches since block content changed.
                self.tabs[i].scene.rebuild_derived_caches();
            }

            "REFCLOSE_DISCARD" => {
                let session = match self.tabs[i].refedit_session.take() {
                    Some(s) => s,
                    None => {
                        self.command_line
                            .push_error("REFCLOSE: no REFEDIT session active.");
                        return Some(Task::none());
                    }
                };
                // Remove temp entities without modifying the block.
                self.tabs[i].scene.erase_entities(&session.temp_handles);
                self.tabs[i].scene.deselect_all();
                // End the edit fade — restore the drawing to full brightness.
                self.tabs[i].scene.set_refedit_keep(None);
                self.command_line
                    .push_output("REFCLOSE: Changes discarded.");
            }

            "ALIGN" | "AL" => {
                use crate::modules::draw::modify::align::AlignCommand;
                let cmd = AlignCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "LENGTHEN" | "LEN" => {
                use crate::modules::draw::modify::lengthen::LengthenCommand;
                let cmd = LengthenCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIVIDE" | "DIV" => {
                use crate::modules::draw::inquiry::divide::DivideCommand;
                let cmd = DivideCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "MEASURE" | "ME" => {
                use crate::modules::draw::inquiry::divide::MeasureCommand;
                let cmd = MeasureCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // ── Inquiry ──────────────────────────────────────────────────────
            "DIST" | "DI" => {
                use crate::modules::draw::inquiry::dist::DistCommand;
                let cmd = DistCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "ID" => {
                use crate::modules::draw::inquiry::id::IdCommand;
                let cmd = IdCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "AREA" => {
                use crate::modules::draw::inquiry::area::AreaCommand;
                let cmd = AreaCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // ── MASSPROP — area, perimeter, centroid of selected entities ────
            "MASSPROP" => {
                let selected = self.tabs[i].scene.selected_entities();
                if selected.is_empty() {
                    self.command_line
                        .push_error("MASSPROP: no entities selected. Select entities first.");
                } else {
                    for (handle, _) in &selected {
                        if let Some(entity) = self.tabs[i].scene.document.get_entity(*handle) {
                            use crate::entities::traits::EntityTypeOps;
                            if let Some(props) = entity.mass_props() {
                                self.command_line.push_output(&format!(
                                    "{}  Area={:.4}  Perimeter={:.4}  Centroid=({:.4},{:.4})",
                                    crate::entities::names::dxf_name(entity),
                                    props.area,
                                    props.perimeter,
                                    props.cx,
                                    props.cy,
                                ));
                            }
                        }
                    }
                }
            }

            // ── FLATTEN — move selected (or all) entities to Z=0 ─────────────
            "FLATTEN" => {
                let handles: Vec<acadrust::Handle> = {
                    let sel = self.tabs[i].scene.selected_entities();
                    if sel.is_empty() {
                        // Flatten all entities
                        self.tabs[i]
                            .scene
                            .document
                            .entities()
                            .map(|e| e.common().handle)
                            .collect()
                    } else {
                        sel.into_iter().map(|(h, _)| h).collect()
                    }
                };
                if handles.is_empty() {
                    self.command_line.push_error("FLATTEN: no entities.");
                } else {
                    self.push_undo_snapshot(i, "FLATTEN");
                    for h in &handles {
                        if let Some(e) = self.tabs[i].scene.document.get_entity_mut(*h) {
                            flatten_entity_z(e);
                        }
                    }
                    self.tabs[i].dirty = true;
                    self.command_line.push_output(&format!(
                        "FLATTEN: {} entity(ies) moved to Z=0.",
                        handles.len()
                    ));
                    self.refresh_properties();
                }
            }

            // ── QSELECT — quick-select entities by property ───────────────────
            // QSELECT TYPE <type>          — select all entities of given type
            // QSELECT LAYER <name>         — select all entities on layer
            // QSELECT COLOR <n>            — select all entities with color index n
            // QSELECT LINETYPE <name>      — select all entities with linetype
            cmd if cmd == "QSELECT" || cmd.starts_with("QSELECT ") => {
                let rest = cmd.split_once(' ').map(|(_, r)| r.trim()).unwrap_or("");
                let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                let prop = parts.first().map(|s| s.to_uppercase()).unwrap_or_default();
                let val = parts.get(1).map(|s| s.trim()).unwrap_or("").to_uppercase();

                let matched: Vec<acadrust::Handle> = self.tabs[i]
                    .scene
                    .document
                    .entities()
                    .filter(|e| {
                        let c = e.common();
                        match prop.as_str() {
                            "TYPE" => crate::entities::names::dxf_name(e).to_uppercase() == val,
                            "LAYER" => c.layer.to_uppercase() == val,
                            "COLOR" => c
                                .color
                                .index()
                                .map(|n| n.to_string() == val)
                                .unwrap_or(val == "BYLAYER"),
                            "LINETYPE" => c.linetype.to_uppercase() == val,
                            _ => false,
                        }
                    })
                    .map(|e| e.common().handle)
                    .collect();

                if prop.is_empty() {
                    self.command_line
                        .push_info("Usage: QSELECT TYPE|LAYER|COLOR|LINETYPE <value>");
                } else if matched.is_empty() {
                    self.command_line
                        .push_output("QSELECT: no matching entities.");
                } else {
                    self.tabs[i].scene.deselect_all();
                    for h in &matched {
                        self.tabs[i].scene.select_entity(*h, false);
                    }
                    self.command_line
                        .push_output(&format!("QSELECT: {} entity(ies) selected.", matched.len()));
                    self.refresh_properties();
                }
            }

            // ── COUNT — entity statistics ─────────────────────────────────────
            cmd if cmd == "COUNT" || cmd.starts_with("COUNT ") => {
                let filter = cmd.split_once(' ').map(|(_, r)| r.trim().to_uppercase());
                let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
                for e in self.tabs[i].scene.document.entities() {
                    let layer = &e.common().layer;
                    let type_name = crate::entities::names::dxf_name(e);
                    let key = match &filter {
                        Some(f) if f == "LAYER" => layer.clone(),
                        Some(f) if f == "TYPE" => type_name.to_string(),
                        Some(f) => {
                            // Filter by layer name
                            if layer.to_uppercase() != *f {
                                continue;
                            }
                            type_name.to_string()
                        }
                        None => type_name.to_string(),
                    };
                    *counts.entry(key).or_default() += 1;
                }
                let total: usize = counts.values().sum();
                for (k, n) in &counts {
                    self.command_line.push_output(&format!("  {k}: {n}"));
                }
                self.command_line
                    .push_output(&format!("COUNT: {total} entity(ies) total."));
            }

            "DATAEXTRACTION" | "EATTEXT" | "ATTEXT" => {
                let csv = build_data_extraction_csv(&self.tabs[i].scene.document);
                return Some(Task::done(Message::DataExtractionSave(csv)));
            }

            // ── Find / Replace ────────────────────────────────────────────────
            // FIND <search>              — list all Text/MText/Dimension containing <search>
            // FIND <search> REPLACE <rep> — replace first occurrence (case-insensitive)
            // FINDALL <search> REPLACE <rep> — replace all occurrences
            cmd if cmd == "FIND"
                || cmd.starts_with("FIND ")
                || cmd == "FINDALL"
                || cmd.starts_with("FINDALL ") =>
            {
                let all_mode = cmd.starts_with("FINDALL");
                let rest = cmd.split_once(' ').map(|(_, r)| r.trim()).unwrap_or("");

                // Split at " REPLACE " keyword (case-insensitive)
                let (search, replacement) = if let Some(pos) = rest.to_uppercase().find(" REPLACE ")
                {
                    (&rest[..pos], Some(rest[pos + 9..].trim()))
                } else {
                    (rest, None)
                };

                if search.is_empty() {
                    self.command_line.push_error("FIND: specify search text.");
                } else {
                    let search_lc = search.to_lowercase();
                    let mut count = 0usize;
                    let handles: Vec<acadrust::Handle> = self.tabs[i]
                        .scene
                        .document
                        .entities()
                        .filter_map(|e| {
                            use crate::entities::traits::EntityTypeOps; let txt = e.text_content()?;
                            if txt.to_lowercase().contains(&search_lc) {
                                Some(e.common().handle)
                            } else {
                                None
                            }
                        })
                        .collect();

                    if let Some(rep) = replacement {
                        // Replace mode
                        let targets: Vec<_> = if all_mode {
                            handles.clone()
                        } else {
                            handles.iter().copied().take(1).collect()
                        };
                        if targets.is_empty() {
                            self.command_line
                                .push_output(&format!("FIND: \"{}\" not found.", search));
                        } else {
                            self.push_undo_snapshot(i, "FIND/REPLACE");
                            for h in &targets {
                                if let Some(e) = self.tabs[i].scene.document.get_entity_mut(*h) {
                                    crate::entities::traits::EntityTypeOps::replace_text(e, search, rep);
                                    count += 1;
                                }
                            }
                            self.tabs[i].dirty = true;
                            self.command_line.push_output(&format!(
                                "FIND/REPLACE: replaced {} occurrence(s) of \"{}\" → \"{}\".",
                                count, search, rep
                            ));
                            self.refresh_properties();
                        }
                    } else {
                        // List mode
                        if handles.is_empty() {
                            self.command_line
                                .push_output(&format!("FIND: \"{}\" not found.", search));
                        } else {
                            for h in &handles {
                                if let Some(e) = self.tabs[i].scene.document.get_entity(*h) {
                                    use crate::entities::traits::EntityTypeOps; let txt = e.text_content().unwrap_or_default();
                                    self.command_line.push_output(&format!(
                                        "  Handle {:X}: \"{}\"",
                                        h.value(),
                                        txt
                                    ));
                                }
                            }
                            self.command_line.push_output(&format!(
                                "FIND: {} match(es) for \"{}\".",
                                handles.len(),
                                search
                            ));
                        }
                    }
                }
            }

            _ => return None,
        }
        Some(self.finish_dispatch(cmd))
    }
}

fn entity_list_details(entity: &acadrust::EntityType) -> String {
    use std::f64::consts::PI;
    match entity {
        acadrust::EntityType::Line(l) => format!(
            "from ({:.4},{:.4},{:.4}) to ({:.4},{:.4},{:.4})  len={:.4}",
            l.start.x,
            l.start.y,
            l.start.z,
            l.end.x,
            l.end.y,
            l.end.z,
            ((l.end.x - l.start.x).powi(2)
                + (l.end.y - l.start.y).powi(2)
                + (l.end.z - l.start.z).powi(2))
            .sqrt()
        ),
        acadrust::EntityType::Circle(c) => format!(
            "center ({:.4},{:.4},{:.4})  r={:.4}  area={:.4}",
            c.center.x,
            c.center.y,
            c.center.z,
            c.radius,
            PI * c.radius * c.radius
        ),
        acadrust::EntityType::Arc(a) => format!(
            "center ({:.4},{:.4},{:.4})  r={:.4}  start={:.2}° end={:.2}°",
            a.center.x,
            a.center.y,
            a.center.z,
            a.radius,
            a.start_angle.to_degrees(),
            a.end_angle.to_degrees()
        ),
        acadrust::EntityType::LwPolyline(p) => format!(
            "{} vertices  closed={}  elevation={:.4}",
            p.vertices.len(),
            p.is_closed,
            p.elevation
        ),
        acadrust::EntityType::Text(t) => format!(
            "\"{}\"  h={:.4}  at ({:.4},{:.4})",
            t.value, t.height, t.insertion_point.x, t.insertion_point.y
        ),
        acadrust::EntityType::MText(t) => format!(
            "\"{}\"  h={:.4}  at ({:.4},{:.4})",
            t.value.chars().take(40).collect::<String>(),
            t.height,
            t.insertion_point.x,
            t.insertion_point.y
        ),
        acadrust::EntityType::Insert(ins) => format!(
            "block=\"{}\"  at ({:.4},{:.4},{:.4})  scale=({:.4},{:.4},{:.4})  rot={:.2}°",
            ins.block_name,
            ins.insert_point.x,
            ins.insert_point.y,
            ins.insert_point.z,
            ins.x_scale(),
            ins.y_scale(),
            ins.z_scale(),
            ins.rotation.to_degrees()
        ),
        acadrust::EntityType::Spline(s) => format!(
            "{} ctrl pts  degree={}  closed={}",
            s.control_points.len(),
            s.degree,
            s.flags.closed
        ),
        acadrust::EntityType::Ellipse(e) => format!(
            "center ({:.4},{:.4})  major_len={:.4}  ratio={:.4}",
            e.center.x,
            e.center.y,
            e.major_axis_length(),
            e.minor_axis_ratio
        ),
        _ => String::new(),
    }
}

fn flatten_entity_z(entity: &mut acadrust::EntityType) {
    match entity {
        acadrust::EntityType::Line(l) => {
            l.start.z = 0.0;
            l.end.z = 0.0;
        }
        acadrust::EntityType::Circle(c) => {
            c.center.z = 0.0;
        }
        acadrust::EntityType::Arc(a) => {
            a.center.z = 0.0;
        }
        acadrust::EntityType::LwPolyline(p) => {
            p.elevation = 0.0;
        }
        acadrust::EntityType::Text(t) => {
            t.insertion_point.z = 0.0;
        }
        acadrust::EntityType::MText(t) => {
            t.insertion_point.z = 0.0;
        }
        acadrust::EntityType::Insert(ins) => {
            ins.insert_point.z = 0.0;
        }
        acadrust::EntityType::Point(p) => {
            p.location.z = 0.0;
        }
        acadrust::EntityType::Spline(s) => {
            for cp in &mut s.control_points {
                cp.z = 0.0;
            }
            for fp in &mut s.fit_points {
                fp.z = 0.0;
            }
        }
        acadrust::EntityType::Ellipse(e) => {
            e.center.z = 0.0;
        }
        _ => {}
    }
}

// ── DATAEXTRACTION ─────────────────────────────────────────────────────────

/// Build a CSV string with one row per entity in model space.
/// Columns: Type, Handle, Layer, Color, Linetype, ExtraInfo
fn build_data_extraction_csv(doc: &acadrust::CadDocument) -> String {
    use acadrust::EntityType;

    let mut out = String::from("Type,Handle,Layer,Color,Linetype,ExtraInfo\n");

    let ms_handle = doc.header.model_space_block_handle;
    for e in doc.entities() {
        // Skip Block/EndBlock sentinels and paper-space entities.
        if matches!(e, EntityType::Block(_) | EntityType::BlockEnd(_)) {
            continue;
        }
        if !ms_handle.is_null() && e.common().owner_handle != ms_handle {
            continue;
        }
        let type_name = crate::entities::names::dxf_name(e);
        let handle = format!("{:X}", e.common().handle.value());
        let layer = csv_escape(&e.common().layer);
        let color = format!("{}", e.common().color);
        let lt = csv_escape(&e.common().linetype);
        let extra = csv_escape(&entity_extra_info(e));
        out.push_str(&format!(
            "{type_name},{handle},{layer},{color},{lt},{extra}\n"
        ));
    }
    out
}


/// Return a short geometry summary for CSV ExtraInfo column.
fn entity_extra_info(entity: &acadrust::EntityType) -> String {
    use acadrust::EntityType;
    match entity {
        EntityType::Line(e) => format!(
            "({:.3},{:.3})-({:.3},{:.3})",
            e.start.x, e.start.y, e.end.x, e.end.y
        ),
        EntityType::Circle(e) => {
            format!("C({:.3},{:.3}) R={:.3}", e.center.x, e.center.y, e.radius)
        }
        EntityType::Arc(e) => format!(
            "C({:.3},{:.3}) R={:.3} {:.1}°-{:.1}°",
            e.center.x,
            e.center.y,
            e.radius,
            e.start_angle.to_degrees(),
            e.end_angle.to_degrees()
        ),
        EntityType::Text(e) => e.value.clone(),
        EntityType::MText(e) => e.value.chars().take(60).collect(),
        EntityType::Insert(e) => format!(
            "BLK={} @({:.3},{:.3})",
            e.block_name, e.insert_point.x, e.insert_point.y
        ),
        EntityType::LwPolyline(e) => format!("{} vertices", e.vertices.len()),
        EntityType::Polyline(e) => format!("{} vertices", e.vertices.len()),
        EntityType::Polyline2D(e) => format!("{} vertices", e.vertices.len()),
        EntityType::Polyline3D(e) => format!("{} vertices", e.vertices.len()),
        EntityType::Hatch(e) => format!("PAT={}", e.pattern.name),
        EntityType::Dimension(e) => format!("{:.3}", e.base().actual_measurement),
        EntityType::Spline(e) => format!("{} ctrl pts", e.control_points.len()),
        _ => String::new(),
    }
}

/// Escape a string for a CSV field (wrap in quotes if it contains comma/quote/newline).
fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
