use super::*;

impl OpenCADStudio {
    pub(super) fn dispatch_display(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            // ── Display refresh (no-op in GPU raster pipeline) ────────────────
            "REGEN" | "REGENALL" | "REDRAW" | "REDRWALL" => {
                // Display is always up-to-date in the GPU raster pipeline.
                self.command_line.push_output("Display regenerated.");
            }

            // Interactive pan: left-drag pans the view until Esc. The only pan
            // path when there is no middle mouse button (trackpad / web).
            "PAN" | "P" => {
                self.tabs[i].pan_mode = true;
                self.command_line
                    .push_output("PAN: drag with the left mouse button. Press Esc to exit.");
            }

            // RMBENTER [ON|OFF] — toggle whether a viewport right-click acts as
            // Enter (commit / close) while a command is active. Idle right-click
            // still opens the context menu and right-drag still orbits. The
            // preference is persisted across runs.
            cmd if cmd == "RMBENTER" || cmd.starts_with("RMBENTER ") => {
                let arg = cmd
                    .split_once(' ')
                    .map(|(_, r)| r.trim().to_uppercase())
                    .unwrap_or_default();
                self.rmb_enter = match arg.as_str() {
                    "ON" | "1" => true,
                    "OFF" | "0" => false,
                    _ => !self.rmb_enter,
                };
                self.command_line.push_output(if self.rmb_enter {
                    "RMBENTER on: right-click acts as Enter while a command is active."
                } else {
                    "RMBENTER off: right-click opens the context menu."
                });
            }

            // ── TABLE cell editing ─────────────────────────────────────────────
            // TABLE CELL <row> <col> <text> — set text for a cell in the selected Table
            cmd if cmd.starts_with("TABLE ") => {
                let rest = cmd.trim_start_matches("TABLE").trim();
                let sub_up = rest.split_whitespace().next().unwrap_or("").to_uppercase();
                if sub_up == "CELL" {
                    let parts: Vec<&str> = rest.splitn(4, char::is_whitespace).collect();
                    // parts: ["CELL", "<row>", "<col>", "<text>"]
                    let row_res = parts.get(1).and_then(|s| s.parse::<usize>().ok());
                    let col_res = parts.get(2).and_then(|s| s.parse::<usize>().ok());
                    let text = parts.get(3).copied().unwrap_or("");
                    match (row_res, col_res) {
                        (Some(row), Some(col)) => {
                            let selected_handles: Vec<acadrust::Handle> = self.tabs[i]
                                .scene
                                .selected_entities()
                                .iter()
                                .map(|(h, _)| *h)
                                .collect();
                            let mut found = false;
                            for sh in &selected_handles {
                                if let Some(acadrust::EntityType::Table(tbl)) = self.tabs[i]
                                    .scene
                                    .document
                                    .entities_mut()
                                    .find(|e| e.common().handle == *sh)
                                {
                                    if tbl.set_cell_text(row, col, text) {
                                        found = true;
                                    }
                                }
                            }
                            if found {
                                self.push_undo_snapshot(i, "TABLE CELL");
                                self.tabs[i].dirty = true;
                                self.command_line.push_output(&format!(
                                    "TABLE CELL: set [{row},{col}] = \"{text}\"."
                                ));
                            } else {
                                self.command_line.push_error(
                                    "TABLE CELL: select a Table entity first, or row/col out of range."
                                );
                            }
                        }
                        _ => {
                            self.command_line
                                .push_info("Usage: TABLE CELL <row> <col> <text>");
                        }
                    }
                } else {
                    self.command_line.push_info(
                        "Usage: TABLE  (creates new table)  or  TABLE CELL <row> <col> <text>",
                    );
                }
            }

            // ── UCSICON — toggle UCS icon visibility on all viewports ────────────
            // UCSICON ON       — show UCS icon in all viewports
            // UCSICON OFF      — hide UCS icon in all viewports
            // UCSICON NOORIGIN — show icon but not at origin (show at corner)
            // UCSICON ORIGIN   — show icon at UCS origin
            cmd if cmd == "UCSICON" || cmd.starts_with("UCSICON ") => {
                let sub = cmd.split_whitespace().nth(1).unwrap_or("").to_uppercase();
                match sub.as_str() {
                    "ON" | "OFF" | "NOORIGIN" | "ORIGIN" => {
                        self.push_undo_snapshot(i, "UCSICON");
                        let visible = sub != "OFF";
                        let at_origin = sub == "ORIGIN";
                        // Update model-space icon flags.
                        self.show_ucs_icon = visible;
                        if sub == "NOORIGIN" || sub == "ORIGIN" {
                            self.ucs_icon_at_origin = at_origin;
                        }
                        let mut count = 0usize;
                        for entity in self.tabs[i].scene.document.entities_mut() {
                            if let acadrust::EntityType::Viewport(vp) = entity {
                                vp.status.ucs_icon_visible = visible;
                                if sub == "NOORIGIN" || sub == "ORIGIN" {
                                    vp.status.ucs_icon_at_origin = at_origin;
                                }
                                count += 1;
                            }
                        }
                        self.tabs[i].dirty = true;
                        self.command_line.push_output(&format!(
                            "UCSICON {sub}: updated {count} viewport(s) + model space."
                        ));
                    }
                    "" => {
                        // Bare UCSICON toggles visibility.
                        self.push_undo_snapshot(i, "UCSICON");
                        let visible = !self.show_ucs_icon;
                        self.show_ucs_icon = visible;
                        for entity in self.tabs[i].scene.document.entities_mut() {
                            if let acadrust::EntityType::Viewport(vp) = entity {
                                vp.status.ucs_icon_visible = visible;
                            }
                        }
                        self.tabs[i].dirty = true;
                        let state = if visible { "ON" } else { "OFF" };
                        self.command_line.push_output(&format!("UCSICON {state}"));
                    }
                    _ => {
                        self.command_line
                            .push_info("Usage: UCSICON ON | OFF | NOORIGIN | ORIGIN");
                    }
                }
            }

            // ── NAVVCUBE — toggle ViewCube visibility ────────────────────────────
            "NAVVCUBE" => {
                return Some(Task::done(Message::ToggleViewCube));
            }

            // ── PROPERTIES — toggle Properties panel visibility ──────────────────
            "PROPERTIES" | "PR" | "PROPS" => {
                return Some(Task::done(Message::ToggleProperties));
            }

            // ── FILETAB — toggle file/document tabs ──────────────────────────────
            "FILETAB" => {
                return Some(Task::done(Message::ToggleFileTabs));
            }

            // ── LAYOUTTAB — toggle layout/paper-space tabs ───────────────────────
            "LAYOUTTAB" => {
                return Some(Task::done(Message::ToggleLayoutTabs));
            }

            // ── TOOLPALETTES — not yet implemented ───────────────────────────────
            "TOOLPALETTES" | "TP" => {
                self.command_line
                    .push_info("TOOLPALETTES: Tool Palettes not yet implemented.");
            }

            // ── SHEETSET — not yet implemented ───────────────────────────────────
            "SHEETSET" | "SSM" => {
                self.command_line
                    .push_info("SHEETSET: Sheet Set Manager not yet implemented.");
            }

            // ── XDATA — read/write extended entity data ──────────────────────────
            // XDATA LIST             — show all xdata records on selected entities
            // XDATA SET <app> <str>  — append a string xdata value for <app>
            // XDATA CLEAR            — remove all xdata from selected entities
            // XDATA CLEAR <app>      — remove xdata for a specific application
            cmd if cmd == "XDATA" || cmd.starts_with("XDATA ") => {
                use acadrust::xdata::{ExtendedDataRecord, XDataValue};
                let rest = cmd.trim_start_matches("XDATA").trim();
                let parts: Vec<&str> = rest.splitn(3, char::is_whitespace).collect();
                let sub = parts.first().map(|s| s.to_uppercase()).unwrap_or_default();
                let selected_handles: Vec<acadrust::Handle> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .iter()
                    .map(|(h, _)| *h)
                    .collect();
                if selected_handles.is_empty() {
                    self.command_line
                        .push_error("XDATA: select entities first.");
                } else {
                    match sub.as_str() {
                        "LIST" | "" => {
                            for sh in &selected_handles {
                                if let Some(entity) = self.tabs[i].scene.document.get_entity(*sh) {
                                    let xd = &entity.common().extended_data;
                                    if xd.is_empty() {
                                        self.command_line
                                            .push_output(&format!("  {:x}: no xdata.", sh.value()));
                                    } else {
                                        for rec in xd.records() {
                                            self.command_line.push_output(&format!(
                                                "  {:x} [{}]: {} value(s)",
                                                sh.value(),
                                                rec.application_name,
                                                rec.values.len()
                                            ));
                                            for v in &rec.values {
                                                self.command_line
                                                    .push_output(&format!("    {:?}", v));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        "SET" => {
                            let app = parts.get(1).copied().unwrap_or("OpenCADStudio");
                            let val = parts.get(2).copied().unwrap_or("");
                            self.push_undo_snapshot(i, "XDATA SET");
                            for sh in &selected_handles {
                                if let Some(entity) =
                                    self.tabs[i].scene.document.get_entity_mut(*sh)
                                {
                                    let mut rec = ExtendedDataRecord::new(app);
                                    rec.add_value(XDataValue::String(val.to_string()));
                                    entity.common_mut().extended_data.add_record(rec);
                                }
                            }
                            self.tabs[i].dirty = true;
                            self.command_line.push_output(&format!(
                                "XDATA: set [{app}] = \"{val}\" on {} entity/entities.",
                                selected_handles.len()
                            ));
                        }
                        "CLEAR" => {
                            let app_filter = parts.get(1).copied();
                            self.push_undo_snapshot(i, "XDATA CLEAR");
                            for sh in &selected_handles {
                                if let Some(entity) =
                                    self.tabs[i].scene.document.get_entity_mut(*sh)
                                {
                                    let xd = &mut entity.common_mut().extended_data;
                                    if let Some(app) = app_filter {
                                        // Rebuild without the matching app.
                                        let kept: Vec<_> = xd
                                            .records()
                                            .iter()
                                            .filter(|r| r.application_name != app)
                                            .cloned()
                                            .collect();
                                        xd.clear();
                                        for r in kept {
                                            xd.add_record(r);
                                        }
                                    } else {
                                        xd.clear();
                                    }
                                }
                            }
                            self.tabs[i].dirty = true;
                            self.command_line.push_output("XDATA: cleared.");
                        }
                        _ => {
                            self.command_line
                                .push_info("Usage: XDATA LIST | SET <app> <value> | CLEAR [app]");
                        }
                    }
                }
            }

            // BOX / SPHERE / CYLINDER / CONE / WEDGE / TORUS are handled by the
            // Model-tab primitive command above (with truck boolean caching).

            // ── EXTRUDE ────────────────────────────────────────────────────
            "EXTRUDE" | "EXT" => {
                use crate::modules::insert::solid3d_cmds::ExtrudeCommand;
                // If a single entity is already selected, skip the pick step.
                let selected: Vec<_> = self.tabs[i].scene.selected_entities().into_iter().collect();
                let color = self.tabs[i].scene.layer_color(&self.tabs[i].active_layer);
                if selected.len() == 1 {
                    let handle = selected[0].0;
                    let mut cmd = ExtrudeCommand::new(color);
                    cmd.on_entity_pick(handle, glam::DVec3::ZERO);
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    let cmd = ExtrudeCommand::new(color);
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                }
            }

            // ── REVOLVE ────────────────────────────────────────────────────
            "REVOLVE" | "REV" => {
                use crate::modules::insert::solid3d_cmds::RevolveCommand;
                let color = self.tabs[i].scene.layer_color(&self.tabs[i].active_layer);
                let cmd = RevolveCommand::new(color);
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // ── SWEEP ──────────────────────────────────────────────────────
            "SWEEP" => {
                use crate::modules::insert::solid3d_cmds::SweepCommand;
                let color = self.tabs[i].scene.layer_color(&self.tabs[i].active_layer);
                let cmd = SweepCommand::new(color);
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // ── LOFT ───────────────────────────────────────────────────────
            "LOFT" => {
                use crate::modules::insert::solid3d_cmds::LoftCommand;
                let color = self.tabs[i].scene.layer_color(&self.tabs[i].active_layer);
                let cmd = LoftCommand::new(color);
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // ── OBJ import ───────────────────────────────────────────────
            "IMPORTOBJ" | "OBJIMPORT" => {
                return Some(Task::done(Message::ObjImport));
            }

            // ── STL export ────────────────────────────────────────────────
            "STLOUT" | "EXPORTSTL" => {
                return Some(Task::done(Message::StlExport));
            }

            // STEPOUT — export 3D meshes to STEP AP203 format
            "STEPOUT" | "EXPORTSTEP" | "STPOUT" => {
                return Some(Task::done(Message::StepExport));
            }

            // ── Plot Style Editor GUI ─────────────────────────────────────
            "PLOTSTYLEPANEL" | "PLOTSTYLEEDITOR" | "STYLESMANAGER" => {
                return Some(Task::done(Message::PlotStylePanelOpen));
            }

            // ── Plot / Page Setup ──────────────────────────────────────────
            "PLOT" | "EXPORT" => {
                return Some(Task::done(Message::PlotExport));
            }
            // PRINT — send current layout to the system default printer.
            "PRINT" => {
                return Some(Task::done(Message::PrintToPrinter));
            }
            // PLOTSTYLE — load or clear CTB/STB plot style table
            cmd if cmd == "PLOTSTYLE" || cmd.starts_with("PLOTSTYLE ") => {
                let sub = cmd
                    .split_once(' ')
                    .map(|(_, r)| r.trim().to_uppercase())
                    .unwrap_or_default();
                match sub.as_str() {
                    "CLEAR" | "NONE" => {
                        return Some(Task::done(Message::PlotStyleClear));
                    }
                    "" | "LOAD" => {
                        let active = self
                            .active_plot_style
                            .as_ref()
                            .map(|t| format!("Active: {}", t.name))
                            .unwrap_or_else(|| "No plot style loaded.".into());
                        self.command_line.push_info(&active);
                        return Some(Task::done(Message::PlotStyleLoad));
                    }
                    "?" | "STATUS" => {
                        let msg = self
                            .active_plot_style
                            .as_ref()
                            .map(|t| {
                                format!(
                                    "Plot style: {}  ({} color overrides)",
                                    t.name,
                                    t.aci_entries.iter().filter(|e| e.color.is_some()).count()
                                )
                            })
                            .unwrap_or_else(|| "No plot style table loaded.".into());
                        self.command_line.push_output(&msg);
                    }
                    _ => {
                        self.command_line
                            .push_error("Usage: PLOTSTYLE [LOAD | CLEAR | STATUS]");
                    }
                }
            }
            // UNDERLAY — edit properties of selected PDF/DWF/DGN underlay entities.
            // Usage:
            //   UNDERLAY FADE <0-80>
            //   UNDERLAY CONTRAST <0-100>
            //   UNDERLAY ON | OFF
            //   UNDERLAY CLIP ON | OFF
            //   UNDERLAY MONO ON | OFF
            cmd if cmd == "UNDERLAY" || cmd.starts_with("UNDERLAY ") => {
                let sub = cmd
                    .split_once(' ')
                    .map(|(_, r)| r.trim().to_uppercase())
                    .unwrap_or_default();
                let handles: Vec<acadrust::Handle> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .iter()
                    .map(|(h, _)| *h)
                    .collect();
                if handles.is_empty() {
                    self.command_line
                        .push_error("UNDERLAY: select underlay entities first.");
                } else {
                    let parts: Vec<&str> = sub.splitn(2, char::is_whitespace).collect();
                    let action = parts.first().copied().unwrap_or("");
                    let arg = parts.get(1).copied().unwrap_or("").trim();
                    let mut changed = 0usize;
                    self.push_undo_snapshot(i, "UNDERLAY");
                    for h in &handles {
                        if let Some(acadrust::EntityType::Underlay(ul)) = self.tabs[i]
                            .scene
                            .document
                            .entities_mut()
                            .find(|e| e.common().handle == *h)
                        {
                            match action {
                                "FADE" => {
                                    if let Ok(v) = arg.parse::<u8>() {
                                        ul.set_fade(v);
                                        changed += 1;
                                    }
                                }
                                "CONTRAST" => {
                                    if let Ok(v) = arg.parse::<u8>() {
                                        ul.set_contrast(v);
                                        changed += 1;
                                    }
                                }
                                "ON" => {
                                    ul.set_on(true);
                                    changed += 1;
                                }
                                "OFF" => {
                                    ul.set_on(false);
                                    changed += 1;
                                }
                                "CLIP" => match arg {
                                    "ON" => {
                                        ul.flags |=
                                            acadrust::entities::UnderlayDisplayFlags::CLIPPING;
                                        changed += 1;
                                    }
                                    "OFF" => {
                                        ul.clear_clip();
                                        changed += 1;
                                    }
                                    _ => {}
                                },
                                "MONO" => match arg {
                                    "ON" => {
                                        ul.set_monochrome(true);
                                        changed += 1;
                                    }
                                    "OFF" => {
                                        ul.set_monochrome(false);
                                        changed += 1;
                                    }
                                    _ => {}
                                },
                                _ => {
                                    // No sub-command: print status.
                                    self.command_line.push_output(&format!(
                                        "Underlay {:x}: fade={}, contrast={}, on={}, clip={}, mono={}",
                                        h.value(),
                                        ul.fade,
                                        ul.contrast,
                                        ul.is_on(),
                                        ul.is_clipping(),
                                        ul.is_monochrome(),
                                    ));
                                }
                            }
                        }
                    }
                    if changed > 0 {
                        self.tabs[i].dirty = true;
                        self.command_line
                            .push_info(&format!("Updated {changed} underlay(s)."));
                    } else if !action.is_empty() {
                        self.command_line.push_error(
                            "Usage: UNDERLAY [FADE <n>|CONTRAST <n>|ON|OFF|CLIP ON|OFF|MONO ON|OFF]"
                        );
                    }
                }
            }

            "PAGESETUP" => {
                if self.tabs[i].scene.current_layout == "Model" {
                    self.command_line
                        .push_error("PAGESETUP: switch to a paper space layout first.");
                } else {
                    return Some(Task::done(Message::PageSetupOpen));
                }
            }
            _ => return None,
        }
        Some(self.finish_dispatch(cmd))
    }
}
