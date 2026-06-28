use super::*;

impl OpenCADStudio {
    pub(super) fn dispatch_dim(&mut self, cmd: &str, i: usize) -> Option<Task<Message>> {
        match cmd {
            "DIMALIGNED" | "DAL" => {
                use crate::modules::annotate::aligned_dim::AlignedDimensionCommand;
                let cmd = AlignedDimensionCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMDIAMETER" | "DDI" => {
                use crate::modules::annotate::diameter_dim::DiameterDimensionCommand;
                let cmd = DiameterDimensionCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMLINEAR" | "DLI" => {
                use crate::modules::annotate::linear_dim::LinearDimensionCommand;
                let new_cmd = LinearDimensionCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "DIMRADIUS" | "DRA" => {
                use crate::modules::annotate::radius_dim::RadiusDimensionCommand;
                let new_cmd = RadiusDimensionCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "DIMANGULAR" | "DAN" => {
                use crate::modules::annotate::angular_dim::AngularDimensionCommand;
                let new_cmd = AngularDimensionCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "DIMORDINATE" | "DOR" => {
                use crate::modules::annotate::ordinate_dim::OrdinateDimCommand;
                let new_cmd = OrdinateDimCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "LEADER" | "LE" => {
                use crate::modules::annotate::leader_cmd::LeaderCommand;
                let new_cmd = LeaderCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "MLEADER" | "MLD" => {
                use crate::modules::annotate::mleader_cmd::MLeaderCommand;
                let new_cmd = MLeaderCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "TOLERANCE" | "TOL" => {
                use crate::modules::annotate::tolerance_cmd::ToleranceCommand;
                let cmd = ToleranceCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "TABLE" => {
                use crate::modules::annotate::table_cmd::TableCommand;
                let cmd = TableCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMCONTINUE" | "DCO" => {
                use crate::modules::annotate::dim_continue::DimContinueCommand;
                let cmd = if let Some((p1, p2, dp, rot, trot)) =
                    find_last_linear_dim(&self.tabs[i].scene)
                {
                    DimContinueCommand::from_base(p1, p2, dp, rot, trot)
                } else {
                    DimContinueCommand::new()
                };
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMBASELINE" | "DBA" => {
                use crate::modules::annotate::dim_baseline::DimBaselineCommand;
                let cmd = if let Some((p1, p2, dp, rot, trot)) =
                    find_last_linear_dim(&self.tabs[i].scene)
                {
                    let doc = &self.tabs[i].scene.document;
                    let dimdli = doc
                        .dim_styles
                        .iter()
                        .find(|s| {
                            s.name
                                .eq_ignore_ascii_case(&doc.header.current_dimstyle_name)
                        })
                        .map(|s| s.dimdli as f32)
                        .unwrap_or(1.5);
                    DimBaselineCommand::from_base(p1, p2, dp, rot, trot, dimdli)
                } else {
                    DimBaselineCommand::new()
                };
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "QDIM" => {
                use crate::modules::annotate::qdim::QdimCommand;
                let cmd = QdimCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // QDIM second stage: the front-end relaunched with the picked
            // dimension-line point and the gathered geometry now selected. Build
            // a continuous chain of linear dimensions across the endpoints.
            cmd if cmd.starts_with("QDIM_PLACE ") => {
                let nums: Vec<f32> = cmd
                    .split_whitespace()
                    .skip(1)
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if nums.len() < 3 {
                    return Some(Task::none());
                }
                let place = glam::Vec3::new(nums[0], nums[1], nums[2]);
                let handles: Vec<acadrust::Handle> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .iter()
                    .map(|(h, _)| *h)
                    .collect();
                let mut pts: Vec<glam::Vec3> = Vec::new();
                for h in &handles {
                    if let Some(e) = self.tabs[i].scene.document.get_entity(*h) {
                        qdim_collect_points(e, &mut pts);
                    }
                }
                if pts.len() < 2 {
                    self.command_line
                        .push_error("QDIM: no dimensionable endpoints in the selection.");
                    return Some(Task::none());
                }
                // Choose the dimension axis from the points' spread: a wider X
                // span dimensions horizontally (ordered by X), else vertically.
                let (minx, maxx) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| {
                    (a.min(p.x), b.max(p.x))
                });
                let (miny, maxy) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| {
                    (a.min(p.y), b.max(p.y))
                });
                let horizontal = (maxx - minx) >= (maxy - miny);
                if horizontal {
                    pts.sort_by(|a, b| a.x.total_cmp(&b.x));
                    pts.dedup_by(|a, b| (a.x - b.x).abs() < 1e-4);
                } else {
                    pts.sort_by(|a, b| a.y.total_cmp(&b.y));
                    pts.dedup_by(|a, b| (a.y - b.y).abs() < 1e-4);
                }
                if pts.len() < 2 {
                    self.command_line
                        .push_error("QDIM: endpoints collapse to a single position.");
                    return Some(Task::none());
                }
                self.push_undo_snapshot(i, "QDIM");
                let v = |p: glam::Vec3| acadrust::types::Vector3::new(p.x as f64, p.y as f64, p.z as f64);
                let mut made = 0usize;
                for w in pts.windows(2) {
                    let (p1, p2) = (w[0], w[1]);
                    let mut dim = acadrust::entities::DimensionLinear::new(v(p1), v(p2));
                    dim.rotation = if horizontal {
                        0.0
                    } else {
                        std::f64::consts::FRAC_PI_2
                    };
                    // Dim line passes through the picked perpendicular position.
                    let def = if horizontal {
                        glam::Vec3::new((p1.x + p2.x) * 0.5, place.y, 0.0)
                    } else {
                        glam::Vec3::new(place.x, (p1.y + p2.y) * 0.5, 0.0)
                    };
                    dim.definition_point = v(def);
                    dim.base.definition_point = v(def);
                    dim.base.actual_measurement = dim.measurement();
                    self.commit_entity(acadrust::EntityType::Dimension(
                        acadrust::entities::Dimension::Linear(dim),
                    ));
                    made += 1;
                }
                self.tabs[i].dirty = true;
                self.command_line
                    .push_output(&format!("QDIM  {made} dimensions created."));
                return Some(Task::none());
            }

            "DIMEDIT" | "DED" => {
                use crate::modules::annotate::dimedit::DimEditCommand;
                let cmd = DimEditCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMTEDIT" | "DIMTED" => {
                use crate::modules::annotate::dimtedit::DimTeditCommand;
                let cmd = DimTeditCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMBREAK" | "DBR" => {
                use crate::modules::annotate::dimbreak::DimBreakCommand;
                let cmd = DimBreakCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMSPACE" | "DSPACE" => {
                use crate::modules::annotate::dimspace::DimSpaceCommand;
                let cmd = DimSpaceCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "DIMJOGLINE" | "DJL" => {
                use crate::modules::annotate::dimjogline::DimJogLineCommand;
                let cmd = DimJogLineCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "MLEADERADD" | "MLA" => {
                use crate::modules::annotate::mleader_edit::MLeaderAddCommand;
                let cmd = MLeaderAddCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "MLEADERREMOVE" | "MLR" => {
                use crate::modules::annotate::mleader_edit::MLeaderRemoveCommand;
                let cmd = MLeaderRemoveCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "MLEADERALIGN" | "MLAL" => {
                use crate::modules::annotate::mleader_edit::MLeaderAlignCommand;
                let cmd = MLeaderAlignCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "MLEADERCOLLECT" | "MLC" => {
                use crate::modules::annotate::mleader_edit::MLeaderCollectCommand;
                let cmd = MLeaderCollectCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            "ZOOM EXTENTS" | "ZOOMEXTENTS" | "ZE" => {
                self.tabs[i].scene.fit_all();
                self.command_line.push_output("Zoom Extents");
            }

            "ZOOM IN" | "ZI" => {
                self.tabs[i].scene.zoom_camera(1.0 / 1.5);
                self.command_line.push_output("Zoom In");
            }

            "ZOOM OUT" | "ZO" => {
                self.tabs[i].scene.zoom_camera(1.5);
                self.command_line.push_output("Zoom Out");
            }

            // ZOOM ALL — fit all entities (same as EXTENTS for now)
            "ZOOM ALL" | "ZOOM A" | "ZA" => {
                self.tabs[i].scene.fit_all();
                self.command_line.push_output("Zoom All");
            }

            // ZOOM SCALE — set zoom factor (e.g. "ZOOM SCALE 2" or "ZS 0.5")
            cmd if cmd.starts_with("ZOOM SCALE ") || cmd.starts_with("ZS ") => {
                let rest = cmd
                    .split_once(' ')
                    .and_then(|(_, r)| r.split_once(' ').map(|(_, v)| v).or(Some(r)))
                    .unwrap_or("1");
                if let Ok(factor) = rest.trim().parse::<f32>() {
                    if factor > 0.0 {
                        self.tabs[i].scene.zoom_camera(1.0 / factor);
                        self.command_line
                            .push_output(&format!("Zoom Scale ×{factor:.3}"));
                    }
                }
            }

            "PLOTWINDOW" | "PW" => {
                use crate::modules::view::plot_window::PlotWindowCommand;
                let cmd = PlotWindowCommand::new();
                self.command_line.push_info(&cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(cmd));
            }

            // Bare ZOOM enters the interactive window zoom (pick two corners) —
            // the common "zoom to a rectangle" action. The sub-keyword forms
            // (ZOOM EXTENTS / IN / OUT / …) are matched above.
            "ZOOM WINDOW" | "ZOOM W" | "ZW" | "ZOOM" => {
                use crate::modules::view::zoom_window::ZoomWindowCommand;
                let new_cmd = ZoomWindowCommand::new();
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "STRETCH" | "SS" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("STRETCH");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::stretch::StretchCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = StretchCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "FILLET" | "F" => {
                use crate::modules::draw::modify::fillet::FilletCommand;
                let entities: Vec<_> = self.tabs[i]
                    .scene
                    .entity_wires()
                    .iter()
                    .filter_map(|w| {
                        let h = Scene::handle_from_wire_name(&w.name)?;
                        self.tabs[i]
                            .scene
                            .document
                            .get_entity(h)
                            .cloned()
                            .map(|e| (h, e))
                    })
                    .collect();
                let all_entities: Vec<_> = entities.into_iter().map(|(_, e)| e).collect();
                let new_cmd = FilletCommand::new(
                    crate::modules::draw::defaults::get_fillet_radius(),
                    all_entities,
                );
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "ARRAY" | "AR" | "ARRAYRECT" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("ARRAYRECT");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::array::ArrayRectCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = ArrayRectCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "ARRAYPOLAR" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("ARRAYPOLAR");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::array::ArrayPolarCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let new_cmd = ArrayPolarCommand::new(handles, wires);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "ARRAYPATH" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("ARRAYPATH");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::array::ArrayPathCommand;
                    let wires = self.tabs[i].scene.wire_models_for(&handles);
                    let all_entities: Vec<_> = self.tabs[i]
                        .scene
                        .entity_wires()
                        .iter()
                        .filter_map(|w| {
                            let h = Scene::handle_from_wire_name(&w.name)?;
                            self.tabs[i].scene.document.get_entity(h).cloned()
                        })
                        .collect();
                    let new_cmd = ArrayPathCommand::new(handles, wires, all_entities);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "ARRAY3D" | "3DARRAY" => {
                let handles: Vec<_> = self.tabs[i]
                    .scene
                    .selected_entities()
                    .into_iter()
                    .map(|(h, _)| h)
                    .collect();
                if handles.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("ARRAY3D");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    use crate::modules::draw::modify::array::Array3DCommand;
                    let new_cmd = Array3DCommand::new(handles);
                    self.command_line.push_info(&new_cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(new_cmd));
                }
            }

            "CHAMFER" | "CHA" => {
                use crate::modules::draw::modify::fillet::ChamferCommand;
                let entities: Vec<_> = self.tabs[i]
                    .scene
                    .entity_wires()
                    .iter()
                    .filter_map(|w| {
                        let h = Scene::handle_from_wire_name(&w.name)?;
                        self.tabs[i]
                            .scene
                            .document
                            .get_entity(h)
                            .cloned()
                            .map(|e| (h, e))
                    })
                    .collect();
                let all_entities: Vec<_> = entities.into_iter().map(|(_, e)| e).collect();
                let new_cmd = ChamferCommand::new(
                    crate::modules::draw::defaults::get_chamfer_dist1(),
                    all_entities,
                );
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "EXPLODE" | "X" => {
                use crate::modules::draw::modify::explode::explode_entity;
                let entities: Vec<_> = self.tabs[i].scene.selected_entities().into_iter().collect();
                if entities.is_empty() {
                    use crate::modules::draw::select::SelectObjectsCommand;
                    let cmd = SelectObjectsCommand::new("EXPLODE");
                    self.command_line.push_info(&cmd.prompt());
                    self.tabs[i].active_cmd = Some(Box::new(cmd));
                } else {
                    let replacements: Vec<(acadrust::Handle, Vec<acadrust::EntityType>)> = entities
                        .iter()
                        .filter_map(|(h, e)| {
                            let pieces = explode_entity(e, &self.tabs[i].scene.document);
                            if pieces.is_empty() {
                                None
                            } else {
                                Some((*h, pieces))
                            }
                        })
                        .collect();
                    let exploded = replacements.len();
                    if exploded > 0 {
                        self.push_undo_snapshot(i, "EXPLODE");
                    }
                    for (handle, pieces) in replacements {
                        self.tabs[i].scene.erase_entities(&[handle]);
                        for piece in pieces {
                            self.tabs[i].scene.add_entity(piece);
                        }
                    }
                    if exploded > 0 {
                        self.tabs[i].dirty = true;
                        self.refresh_properties();
                        self.command_line
                            .push_output(&format!("{exploded} object(s) exploded."));
                    } else {
                        self.command_line
                            .push_info("EXPLODE: no explodable objects selected.");
                    }
                }
            }

            "OFFSET" | "O" => {
                use crate::modules::draw::modify::offset::OffsetCommand;
                let all_entities: Vec<_> = self.tabs[i]
                    .scene
                    .entity_wires()
                    .iter()
                    .filter_map(|w| {
                        let h = Scene::handle_from_wire_name(&w.name)?;
                        self.tabs[i].scene.document.get_entity(h).cloned()
                    })
                    .collect();
                let new_cmd = OffsetCommand::new(all_entities);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "TRIM" | "TR" => {
                use crate::modules::draw::modify::trim::TrimCommand;
                let entities: Vec<_> = self.tabs[i]
                    .scene
                    .entity_wires()
                    .iter()
                    .filter_map(|w| {
                        let h = Scene::handle_from_wire_name(&w.name)?;
                        self.tabs[i]
                            .scene
                            .document
                            .get_entity(h)
                            .cloned()
                            .map(|e| (h, e))
                    })
                    .collect();
                let all_entities: Vec<_> = entities.into_iter().map(|(_, e)| e).collect();
                let new_cmd = TrimCommand::new(all_entities);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            "EXTEND" | "EX" => {
                use crate::modules::draw::modify::trim::ExtendCommand;
                let entities: Vec<_> = self.tabs[i]
                    .scene
                    .entity_wires()
                    .iter()
                    .filter_map(|w| {
                        let h = Scene::handle_from_wire_name(&w.name)?;
                        self.tabs[i]
                            .scene
                            .document
                            .get_entity(h)
                            .cloned()
                            .map(|e| (h, e))
                    })
                    .collect();
                let all_entities: Vec<_> = entities.into_iter().map(|(_, e)| e).collect();
                let new_cmd = ExtendCommand::new(all_entities);
                self.command_line.push_info(&new_cmd.prompt());
                self.tabs[i].active_cmd = Some(Box::new(new_cmd));
            }

            _ => return None,
        }
        Some(self.finish_dispatch(cmd))
    }
}

/// Find the last placed linear or aligned dimension in the document.
/// Returns `(first_point, second_point, definition_point, rotation_rad)` in world-space.
fn find_last_linear_dim(
    scene: &crate::scene::Scene,
) -> Option<(glam::Vec3, glam::Vec3, glam::Vec3, f64, f64)> {
    use acadrust::entities::Dimension;
    let mut best_handle: u64 = 0;
    let mut result: Option<(glam::Vec3, glam::Vec3, glam::Vec3, f64, f64)> = None;

    for entity in scene.document.entities() {
        if let acadrust::EntityType::Dimension(dim) = entity {
            let h = entity.common().handle.value();
            if h <= best_handle {
                continue;
            }
            let item = match dim {
                Dimension::Linear(d) => {
                    let p1 = glam::Vec3::new(
                        d.first_point.x as f32,
                        d.first_point.y as f32,
                        d.first_point.z as f32,
                    );
                    let p2 = glam::Vec3::new(
                        d.second_point.x as f32,
                        d.second_point.y as f32,
                        d.second_point.z as f32,
                    );
                    let dp = glam::Vec3::new(
                        d.base.definition_point.x as f32,
                        d.base.definition_point.y as f32,
                        d.base.definition_point.z as f32,
                    );
                    Some((p1, p2, dp, d.rotation, d.base.text_rotation))
                }
                Dimension::Aligned(d) => {
                    let p1 = glam::Vec3::new(
                        d.first_point.x as f32,
                        d.first_point.y as f32,
                        d.first_point.z as f32,
                    );
                    let p2 = glam::Vec3::new(
                        d.second_point.x as f32,
                        d.second_point.y as f32,
                        d.second_point.z as f32,
                    );
                    let dp = glam::Vec3::new(
                        d.base.definition_point.x as f32,
                        d.base.definition_point.y as f32,
                        d.base.definition_point.z as f32,
                    );
                    let dx = (d.second_point.x - d.first_point.x) as f32;
                    let dy = (d.second_point.y - d.first_point.y) as f32;
                    let rot = dy.atan2(dx) as f64;
                    Some((p1, p2, dp, rot, d.base.text_rotation))
                }
                _ => None,
            };
            if let Some(data) = item {
                best_handle = h;
                result = Some(data);
            }
        }
    }
    result
}

/// Collect candidate dimension endpoints from an entity for QDIM — line ends,
/// polyline vertices, arc endpoints — in world space.
fn qdim_collect_points(e: &acadrust::EntityType, out: &mut Vec<glam::Vec3>) {
    use acadrust::EntityType as ET;
    let p = |v: &acadrust::types::Vector3| glam::Vec3::new(v.x as f32, v.y as f32, v.z as f32);
    match e {
        ET::Line(l) => {
            out.push(p(&l.start));
            out.push(p(&l.end));
        }
        ET::LwPolyline(pl) => {
            for v in &pl.vertices {
                out.push(glam::Vec3::new(
                    v.location.x as f32,
                    v.location.y as f32,
                    pl.elevation as f32,
                ));
            }
        }
        ET::Polyline(pl) => {
            for v in &pl.vertices {
                out.push(p(&v.location));
            }
        }
        ET::Arc(a) => {
            for &ang in &[a.start_angle, a.end_angle] {
                out.push(glam::Vec3::new(
                    (a.center.x + a.radius * ang.cos()) as f32,
                    (a.center.y + a.radius * ang.sin()) as f32,
                    a.center.z as f32,
                ));
            }
        }
        _ => {}
    }
}
