use super::*;

impl OpenCADStudio {
    pub(super) fn dispatch_draw(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            // ── Draw commands ──────────────────────────────────────────────
            "LINE" | "L" => {
                use crate::modules::draw::draw::line::LineCommand;
                let new_cmd = LineCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "MLINE" | "ML" => {
                use crate::modules::draw::draw::mline::MlineCommand;
                let style = self.tabs[i].scene.document.header.multiline_style.clone();
                let cmd_obj = MlineCommand::with_style(style);
                self.command_line.push_info(&cmd_obj.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd_obj));
            }

            cmd if cmd == "WIPEOUT" || cmd == "WO" || cmd.starts_with("WIPEOUT ") => {
                use crate::modules::draw::draw::wipeout::WipeoutCommand;
                let args = cmd
                    .split_once(' ')
                    .map(|(_, r)| r.trim().to_uppercase())
                    .unwrap_or_default();
                let wo_cmd = if args == "P" || args == "POLYGONAL" {
                    WipeoutCommand::new_polygonal()
                } else {
                    WipeoutCommand::new_rectangular()
                };
                self.command_line.push_info(&wo_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(wo_cmd));
            }

            cmd if cmd == "IMAGE" || cmd == "IMAGEATTACH" || cmd == "IM" => {
                return Some(Task::done(Message::ImagePick));
            }

            "REVCLOUD" => {
                use crate::modules::draw::draw::revcloud::RevCloudCommand;
                let cmd = RevCloudCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "ATTDEF" => {
                use crate::modules::draw::draw::attdef::AttdefCommand;
                let cmd = AttdefCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // Command-line attribute editing on selected Insert entities. Bare
            // ATTEDIT and the ATE alias launch the interactive editor instead
            // (see the ATTEDIT arm in the inquiry family); the dash form is the
            // command-line entry point.
            // Usage:
            //   -ATTEDIT          — list all attributes on selected Insert(s)
            //   ATTEDIT <tag> <v> — quick-set attribute <tag> to <v>
            cmd if cmd.starts_with("ATTEDIT ")
                || cmd == "-ATTEDIT"
                || cmd.starts_with("-ATTEDIT ") =>
            {
                let rest = cmd
                    .trim_start_matches("-ATTEDIT")
                    .trim_start_matches("ATTEDIT")
                    .trim();
                let parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
                let selected_handles: Vec<acadrust::Handle> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .iter()
                    .map(|(h, _)| *h)
                    .collect();
                if selected_handles.is_empty() {
                    self.command_line
                        .push_error("ATTEDIT: select an Insert entity first.");
                } else {
                    let mut found_any = false;
                    for sh in &selected_handles {
                        if let Some(acadrust::EntityType::Insert(ins)) = self.tabs[i]
                            .scene
                            .document
                            .entities()
                            .find(|e| e.common().handle == *sh)
                        {
                            found_any = true;
                            if rest.is_empty() {
                                // List attributes.
                                if ins.attributes.is_empty() {
                                    self.command_line.push_output(&format!(
                                        "  Insert {:x}: no attributes.",
                                        sh.value()
                                    ));
                                } else {
                                    for attr in &ins.attributes {
                                        self.command_line.push_output(&format!(
                                            "  [{tag}] = {val}",
                                            tag = attr.tag,
                                            val = attr.get_value()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    if !found_any {
                        self.command_line
                            .push_error("ATTEDIT: no Insert entities in selection.");
                    }
                    // If tag + value supplied, mutate attributes.
                    if parts.len() == 2 && !parts[0].is_empty() {
                        let tag_up = parts[0].to_uppercase();
                        let new_val = parts[1];
                        let mut changed = 0usize;
                        self.push_undo_snapshot(i, "ATTEDIT");
                        for sh in &selected_handles {
                            if let Some(acadrust::EntityType::Insert(ins)) = self.tabs[i]
                                .scene
                                .document
                                .entities_mut()
                                .find(|e| e.common().handle == *sh)
                            {
                                for attr in &mut ins.attributes {
                                    if attr.tag.to_uppercase() == tag_up {
                                        attr.set_value(new_val);
                                        changed += 1;
                                    }
                                }
                            }
                        }
                        if changed > 0 {
                            self.tabs[i].dirty = true;
                            self.command_line.push_output(&format!(
                                "ATTEDIT: updated {changed} attribute(s) [{tag_up}] = {new_val}."
                            ));
                        } else {
                            self.command_line.push_error(&format!(
                                "ATTEDIT: tag '{tag_up}' not found in selection."
                            ));
                        }
                    }
                }
            }

            // ATTDISP — control attribute display visibility.
            // ATTDISP ON   — make all AttributeDefinitions visible
            // ATTDISP OFF  — make all AttributeDefinitions invisible
            // ATTDISP NORMAL — restore: show only those without the invisible flag
            cmd if cmd == "ATTDISP" || cmd.starts_with("ATTDISP ") => {
                let sub = cmd.split_whitespace().nth(1).unwrap_or("").to_uppercase();
                match sub.as_str() {
                    "ON" | "OFF" | "NORMAL" => {
                        self.push_undo_snapshot(i, "ATTDISP");
                        let mut count = 0usize;
                        for entity in self.tabs[i].scene.document.entities_mut() {
                            if let acadrust::EntityType::AttributeDefinition(ad) = entity {
                                match sub.as_str() {
                                    "ON" => {
                                        ad.flags.invisible = false;
                                        count += 1;
                                    }
                                    "OFF" => {
                                        ad.flags.invisible = true;
                                        count += 1;
                                    }
                                    "NORMAL" => { /* leave existing flags — they are already the "normal" state */
                                    }
                                    _ => {}
                                }
                            }
                        }
                        self.tabs[i].dirty = true;
                        self.command_line.push_output(&format!(
                            "ATTDISP {sub}: {count} attribute definition(s) updated."
                        ));
                    }
                    _ => {
                        self.command_line
                            .push_info("Usage: ATTDISP ON | OFF | NORMAL");
                    }
                }
            }

            "DONUT" | "DO" => {
                use crate::modules::draw::draw::donut::DonutCommand;
                let cmd = DonutCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "CIRCLE" | "C" => {
                use crate::modules::draw::draw::circle::CircleCommand;
                let new_cmd = CircleCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "CIRCLE_CD" => {
                use crate::modules::draw::draw::circle::CircleCDCommand;
                let new_cmd = CircleCDCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "CIRCLE_2P" => {
                use crate::modules::draw::draw::circle::Circle2PCommand;
                let new_cmd = Circle2PCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "CIRCLE_3P" => {
                use crate::modules::draw::draw::circle::Circle3PCommand;
                let new_cmd = Circle3PCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "CIRCLE_TTR" => {
                use crate::modules::draw::draw::circle::CircleTTRCommand;
                let new_cmd = CircleTTRCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.pre_cmd_tangent = Some(self.snapper.is_on(crate::snap::SnapType::Tangent));
                self.snapper.enabled.insert(crate::snap::SnapType::Tangent);
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "CIRCLE_TTT" => {
                use crate::modules::draw::draw::circle::CircleTTTCommand;
                let new_cmd = CircleTTTCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.pre_cmd_tangent = Some(self.snapper.is_on(crate::snap::SnapType::Tangent));
                self.snapper.enabled.insert(crate::snap::SnapType::Tangent);
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "ARC" | "A" => {
                use crate::modules::draw::draw::arc::ArcCommand;
                let new_cmd = ArcCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_3P" => {
                use crate::modules::draw::draw::arc::Arc3PCommand;
                let new_cmd = Arc3PCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_SCE" => {
                use crate::modules::draw::draw::arc::ArcSCECommand;
                let new_cmd = ArcSCECommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_SCA" => {
                use crate::modules::draw::draw::arc::ArcSCACommand;
                let new_cmd = ArcSCACommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_SCL" => {
                use crate::modules::draw::draw::arc::ArcSCLCommand;
                let new_cmd = ArcSCLCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_SEA" => {
                use crate::modules::draw::draw::arc::ArcSEACommand;
                let new_cmd = ArcSEACommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_SER" => {
                use crate::modules::draw::draw::arc::ArcSERCommand;
                let new_cmd = ArcSERCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_SED" => {
                use crate::modules::draw::draw::arc::ArcSEDCommand;
                let new_cmd = ArcSEDCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_CSA" => {
                use crate::modules::draw::draw::arc::ArcCSACommand;
                let new_cmd = ArcCSACommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "ARC_CSL" => {
                use crate::modules::draw::draw::arc::ArcCSLCommand;
                let new_cmd = ArcCSLCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "RECT" | "RECTANG" | "REC" => {
                use crate::modules::draw::draw::shapes::RectCommand;
                let new_cmd = RectCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                if self.ortho_mode {
                    self.rect_suppressed_ortho = true;
                    self.ortho_mode = false;
                }
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "RECT_ROT" => {
                use crate::modules::draw::draw::shapes::RectRotCommand;
                let new_cmd = RectRotCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                if self.ortho_mode {
                    self.rect_suppressed_ortho = true;
                    self.ortho_mode = false;
                }
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "RECT_CEN" => {
                use crate::modules::draw::draw::shapes::RectCenCommand;
                let new_cmd = RectCenCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                if self.ortho_mode {
                    self.rect_suppressed_ortho = true;
                    self.ortho_mode = false;
                }
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "POLY" | "POLYGON" | "POL" => {
                use crate::modules::draw::draw::shapes::PolyCommand;
                let new_cmd = PolyCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "POLY_C" => {
                use crate::modules::draw::draw::shapes::PolyCCommand;
                let new_cmd = PolyCCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }
            "POLY_E" => {
                use crate::modules::draw::draw::shapes::PolyECommand;
                let new_cmd = PolyECommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "PLINE" | "PL" => {
                use crate::modules::draw::draw::polyline::PlineCommand;
                let new_cmd = PlineCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "3DPOLY" => {
                use crate::modules::draw::draw::poly3d::Poly3dCommand;
                let new_cmd = Poly3dCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            // 2D filled solid. Reached via SO / SOLID2D — the bare SOLID verb is
            // currently the shaded-display toggle (token collision tracked).
            "SO" | "SOLID2D" => {
                use crate::modules::draw::draw::solid2d::Solid2dCommand;
                let new_cmd = Solid2dCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "HELIX" => {
                use crate::modules::draw::draw::helix::HelixCommand;
                let new_cmd = HelixCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "TRACE" => {
                use crate::modules::draw::draw::trace::TraceCommand;
                let new_cmd = TraceCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "DIMCENTER" | "DCE" | "CENTERMARK" => {
                use crate::modules::draw::draw::dimcenter::DimCenterCommand;
                let new_cmd = DimCenterCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "SKETCH" => {
                use crate::modules::draw::draw::sketch::SketchCommand;
                let new_cmd = SketchCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "REVERSE" => {
                use crate::modules::draw::modify::reverse::ReverseCommand;
                let new_cmd = ReverseCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "MEASUREGEOM" | "MEA" => {
                use crate::modules::draw::inquiry::measuregeom::MeasureGeomCommand;
                let new_cmd = MeasureGeomCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            // ── Modify commands ────────────────────────────────────────────
            "MOVE" | "M" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("MOVE");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::translate::MoveCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = MoveCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "COPY" | "CO" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("COPY");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::copy::CopyCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = CopyCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "ROTATE" | "RO" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("ROTATE");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::rotate::RotateCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = RotateCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "TORIENT" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("TORIENT");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::torient::TorientCommand;
                    let entities: Vec<_> = handles
                        .iter()
                        .filter_map(|&h| self.tabs[i].scene.document.get_entity(h).cloned().map(|e| (h, e)))
                        .collect();
                    let cam_rot = self.tabs[i].scene.camera.borrow().rotation;
                    let right = cam_rot * glam::Vec3::X;
                    let view_twist = right.y.atan2(right.x) as f64;
                    let new_cmd = TorientCommand::new(entities, view_twist);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "POINT" | "PO" => {
                use crate::modules::draw::draw::point::PointCommand;
                let new_cmd = PointCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "RAY" => {
                use crate::modules::draw::draw::ray::RayCommand;
                let new_cmd = RayCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "XLINE" | "XL" | "CONSTRUCTIONLINE" => {
                use crate::modules::draw::draw::ray::XLineCommand;
                let new_cmd = XLineCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "HATCH" | "H" => {
                use crate::modules::draw::draw::hatch::HatchCommand;
                let outlines = self.tabs[i].scene.closed_outlines();
                let new_cmd = HatchCommand::new(outlines);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "HATCHEDIT" | "HE" => {
                use crate::modules::draw::draw::hatchedit::HatcheditCommand;
                // If a single hatch is already selected, skip the pick step.
                let sel = self.tabs[i].scene.selected_entities();
                if sel.len() == 1 {
                    let (h, _) = sel[0];
                    if let Some(model) = self.tabs[i].scene.hatches.get(&h).cloned() {
                        let cmd = HatcheditCommand::with_handle(
                            h,
                            model.name.clone(),
                            model.scale,
                            model.angle_offset,
                        );
                        self.command_line.push_info(&cmd.prompt());
                        self.tabs[i].active_cmd = Some(Box::new(cmd));
                    } else {
                        self.command_line
                            .push_error("HATCHEDIT: selected entity is not a hatch.");
                    }
                } else {
                    let cmd = HatcheditCommand::new();
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                }
            }

            "GRADIENT" => {
                use crate::modules::draw::draw::hatch::GradientCommand;
                let outlines = self.tabs[i].scene.closed_outlines();
                let new_cmd = GradientCommand::new(outlines);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "BOUNDARY" => {
                use crate::modules::draw::draw::hatch::BoundaryCommand;
                let outlines = self.tabs[i].scene.closed_outlines();
                let new_cmd = BoundaryCommand::new(outlines);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "ELLIPSE" | "EL" => {
                use crate::modules::draw::draw::ellipse::EllipseCommand;
                let new_cmd = EllipseCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "ELLIPSE_AXIS" => {
                use crate::modules::draw::draw::ellipse::EllipseAxisCommand;
                let new_cmd = EllipseAxisCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "ELLIPSE_ARC" => {
                use crate::modules::draw::draw::ellipse::EllipseArcCommand;
                let new_cmd = EllipseArcCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "SPLINE" | "SPL" => {
                use crate::modules::draw::draw::spline::SplineCommand;
                let new_cmd = SplineCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "SCALE" | "SC" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("SCALE");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::scale::ScaleCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = ScaleCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "MIRROR" | "MI" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("MIRROR");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::mirror::MirrorCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = MirrorCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "ERASE" | "E" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("ERASE");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    let n = handles.len();
                    self.push_undo_snapshot(i, "ERASE");
                    self.tabs[i].scene.erase_entities(&handles);
                    self.tabs[i].dirty = true;
                    self.refresh_properties();
                    self.command_line
                        .push_output(&format!("{n} object(s) erased."));
                }
            }

            // ── Model commands (3D primitives) ─────────────────────────────
            "BOX" | "WEDGE" | "CYLINDER" | "CONE" | "SPHERE" | "TORUS" => {
                use crate::modules::model::primitive_cmd::PrimitiveCommand;
                let new_cmd = PrimitiveCommand::new(cmd);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            // ── Design commands (solid booleans) ───────────────────────────
            "UNION" | "SUBTRACT" | "INTERSECT" => {
                use crate::modules::model::boolean_cmd::BoolOp;
                if let Some(op) = BoolOp::from_id(cmd) {
                    return Some(self.solid_boolean(op));
                }
            }

            // ── Annotate commands ──────────────────────────────────────────
            "TEXT" | "T" | "DT" => {
                use crate::modules::annotate::text::TextCommand;
                let new_cmd = TextCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "DDEDIT" | "ED" => {
                use crate::modules::annotate::ddedit::DdeditCommand;
                // A single text entity already selected opens its in-place
                // editor directly; otherwise prompt for a pick.
                let sel = self.tabs[i].scene.selected_entities();
                let editable = (sel.len() == 1).then(|| sel[0].0).filter(|h| {
                    self.tabs[i].scene.document.get_entity(*h).is_some_and(|e| {
                        super::super::text_inline::read_text_field(e).is_some()
                            || matches!(e, acadrust::EntityType::Leader(_))
                    })
                });
                if let Some(h) = editable {
                    return Some(self.begin_text_edit(h));
                }
                if sel.len() == 1 {
                    self.command_line
                        .push_error("DDEDIT: selected entity is not text.");
                } else {
                    let cmd = DdeditCommand::new();
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                }
            }

            "MTEXT" | "MT" => {
                use crate::modules::annotate::mtext::MTextCommand;
                let new_cmd = MTextCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "TEXTEDIT" | "TEDIT" => {
                use crate::modules::annotate::textedit::TexteditCommand;
                let mode_str = if self.texteditmode { "Single" } else { "Multiple" };
                self.command_line.push_output(&format!("Current settings: Edit mode = {}", mode_str));
                let new_cmd = TexteditCommand::new(self.texteditmode);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "TEXTEDITMODE" => {
                use crate::modules::annotate::textedit::TexteditmodeCommand;
                let cmd = TexteditmodeCommand::new(self.texteditmode);
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            _ => return None,
        }
        Some(self.finish_dispatch(cmd))
    }
}
