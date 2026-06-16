use super::helpers::{entity_type_key, entity_type_label, title_case_word};
use super::{OpenCADStudio, VARIES_LABEL};
use crate::linetypes;
use crate::scene::view::dispatch;
use crate::ui;
use acadrust::{EntityType, Handle};

impl OpenCADStudio {
    /// Rebuild the PropertiesPanel from the current entity selection.
    /// Preserves UI state (open pickers, edit buffer) across refreshes.
    pub(super) fn refresh_properties(&mut self) {
        let i = self.active_tab;
        // Note: the color-picker dropdown is intentionally NOT carried over — a
        // rebuild means the selection (or a property) changed, so the dropdown
        // closes, matching the deselect / reselect / click-away expectation.
        let color_palette_open = self.tabs[i].properties.color_palette_open;
        let edit_buf = std::mem::take(&mut self.tabs[i].properties.edit_buf);
        let selected_group = self.tabs[i].properties.selected_group.clone();

        // Seed the per-thread unit context from the document header so the
        // entity property builders (which only see f64 values) can format
        // lengths/angles per LUNITS / LUPREC / AUNITS / AUPREC.
        {
            let h = &self.tabs[i].scene.document.header;
            crate::entities::common::set_unit_context(crate::entities::common::UnitContext {
                lunits: h.linear_unit_format,
                luprec: h.linear_unit_precision,
                aunits: h.angular_unit_format,
                auprec: h.angular_unit_precision,
            });
        }

        let layer_names: Vec<String> = self.tabs[i]
            .scene
            .document
            .layers
            .iter()
            .map(|l| l.name.clone())
            .collect();
        let linetype_items: Vec<ui::properties::LinetypeItem> = self.tabs[i]
            .scene
            .document
            .line_types
            .iter()
            .map(|lt| {
                let name = if lt.name.is_empty() {
                    "ByLayer".to_string()
                } else {
                    lt.name.clone()
                };
                let art = linetypes::extract_pattern(&lt.description);
                ui::properties::LinetypeItem { name, art }
            })
            .collect();
        let text_style_names: Vec<String> = self.tabs[i]
            .scene
            .document
            .text_styles
            .iter()
            .map(|style| style.name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();

        let new_panel = {
            let selected = self.tabs[i].scene.selected_entities();
            let mut panel = match selected.len() {
                0 => ui::PropertiesPanel::empty(),
                1 => {
                    let (handle, entity) = selected[0];
                    let group_names = self.tabs[i].scene.group_names_for_entity(handle);
                    let mut sections =
                        dispatch::properties_sectioned(handle, entity, &text_style_names);

                    // Inject viewport-only properties that require doc access.
                    if let acadrust::EntityType::Viewport(vp) = entity {
                        let frozen_names: Vec<String> = vp
                            .frozen_layers
                            .iter()
                            .filter_map(|&h| {
                                self.tabs[i]
                                    .scene
                                    .document
                                    .layers
                                    .iter()
                                    .find(|l| l.handle == h)
                                    .map(|l| l.name.clone())
                            })
                            .collect();

                        // Collect available UCS names for the name picker.
                        let ucs_names: Vec<String> = self.tabs[i]
                            .scene
                            .document
                            .ucss
                            .iter()
                            .map(|u| u.name.clone())
                            .filter(|n| !n.is_empty())
                            .collect();

                        // Current UCS name (resolved from vp.ucs_handle).
                        let current_ucs = self.tabs[i]
                            .scene
                            .document
                            .ucss
                            .iter()
                            .find(|u| u.handle == vp.ucs_handle)
                            .map(|u| u.name.clone())
                            .unwrap_or_default();

                        // Collect available named view names.
                        let view_names: Vec<String> = self.tabs[i]
                            .scene
                            .document
                            .views
                            .iter()
                            .map(|v| v.name.clone())
                            .filter(|n| !n.is_empty())
                            .collect();

                        if let Some(geom) = sections.last_mut() {
                            geom.props.push(crate::scene::model::object::Property {
                                label: "Frozen Layers".to_string(),
                                field: "frozen_layers",
                                value: crate::scene::model::object::PropValue::EditText(
                                    frozen_names.join(", "),
                                ),
                            });
                            if !ucs_names.is_empty() {
                                geom.props.push(crate::scene::model::object::Property {
                                    label: "UCS Name".to_string(),
                                    field: "vp_ucs_name",
                                    value: crate::scene::model::object::PropValue::Choice {
                                        selected: current_ucs,
                                        options: ucs_names,
                                    },
                                });
                            }
                            if !view_names.is_empty() {
                                geom.props.push(crate::scene::model::object::Property {
                                    label: "Named View".to_string(),
                                    field: "vp_named_view",
                                    value: crate::scene::model::object::PropValue::Choice {
                                        selected: String::new(),
                                        options: view_names,
                                    },
                                });
                            }
                        }

                        // Drive the viewport scale picker from the drawing's
                        // own scale list instead of a built-in set.
                        let file_scales = self.tabs[i].scene.scale_list();
                        if !file_scales.is_empty() {
                            let eff = crate::scene::vp_effective_scale(
                                vp.custom_scale,
                                vp.view_height,
                                vp.height,
                            );
                            let selected = file_scales
                                .iter()
                                .find(|(_, _, f)| (f - eff).abs() < 0.001 * f.max(0.001))
                                .map(|(n, _, _)| n.clone())
                                .unwrap_or_default();
                            let options: Vec<String> =
                                file_scales.iter().map(|(n, _, _)| n.clone()).collect();
                            if let Some(geom) = sections.last_mut() {
                                if let Some(prop) =
                                    geom.props.iter_mut().find(|p| p.field == "vscale_std")
                                {
                                    prop.value = crate::scene::model::object::PropValue::Choice {
                                        selected,
                                        options,
                                    };
                                }
                            }
                        }
                    }

                    // Inject DimStyle picker for Dimension entities.
                    if let acadrust::EntityType::Dimension(_) = entity {
                        let dim_style_names: Vec<String> = self.tabs[i]
                            .scene
                            .document
                            .dim_styles
                            .iter()
                            .map(|s| s.name.clone())
                            .filter(|n| !n.is_empty())
                            .collect();
                        if !dim_style_names.is_empty() {
                            // Current style is already shown as EditText in the geom section;
                            // replace/upgrade it to a Choice if we have a list.
                            if let Some(geom) = sections.last_mut() {
                                // Find and replace the style_name EditText with a Choice.
                                if let Some(prop) =
                                    geom.props.iter_mut().find(|p| p.field == "style_name")
                                {
                                    let current = match &prop.value {
                                        crate::scene::model::object::PropValue::EditText(s) => s.clone(),
                                        _ => String::new(),
                                    };
                                    prop.value = crate::scene::model::object::PropValue::Choice {
                                        selected: current,
                                        options: dim_style_names,
                                    };
                                }
                            }
                        }
                    }

                    if !group_names.is_empty() {
                        let label = group_names.join(", ");
                        if let Some(general) = sections.first_mut() {
                            general.props.push(crate::scene::model::object::Property {
                                label: "Group".to_string(),
                                field: "group",
                                value: crate::scene::model::object::PropValue::ReadOnly(label),
                            });
                        }
                    }
                    let title = match entity {
                        acadrust::EntityType::Insert(ins) => {
                            let is_xref = self.tabs[i]
                                .scene
                                .document
                                .block_records
                                .iter()
                                .find(|br| br.name == ins.block_name)
                                .map(|br| br.flags.is_xref || br.flags.is_xref_overlay)
                                .unwrap_or(false);
                            if is_xref {
                                "External Reference".to_string()
                            } else {
                                entity_type_label(entity)
                            }
                        }
                        _ => entity_type_label(entity),
                    };
                    ui::PropertiesPanel {
                        choice_combos: sections
                            .iter()
                            .flat_map(|section| section.props.iter())
                            .filter_map(|prop| match &prop.value {
                                crate::scene::model::object::PropValue::Choice { options, .. } => Some((
                                    prop.field.to_string(),
                                    iced::widget::combo_box::State::new(options.clone()),
                                )),
                                _ => None,
                            })
                            .collect(),
                        sections,
                        title,
                        layer_combo: iced::widget::combo_box::State::new(layer_names.clone()),
                        linetype_combo: iced::widget::combo_box::State::new(linetype_items.clone()),
                        hatch_pattern_combo: iced::widget::combo_box::State::new(
                            crate::scene::model::hatch_patterns::names(),
                        ),
                        lineweight_combo: iced::widget::combo_box::State::new(
                            ui::properties::lw_options(),
                        ),
                        linetype_items,
                        ..Default::default()
                    }
                }
                _ => {
                    let groups = build_selection_groups(&selected);
                    let active_group = selected_group
                        .and_then(|group| groups.iter().find(|g| g.label == group.label).cloned())
                        .or_else(|| groups.first().cloned());

                    let filtered: Vec<(Handle, &EntityType)> = active_group
                        .as_ref()
                        .map(|group| {
                            selected
                                .iter()
                                .filter(|(handle, _)| group.handles.contains(handle))
                                .copied()
                                .collect()
                        })
                        .unwrap_or_default();

                    let sections = aggregate_sections(&filtered, &text_style_names);
                    ui::PropertiesPanel {
                        choice_combos: sections
                            .iter()
                            .flat_map(|section| section.props.iter())
                            .filter_map(|prop| match &prop.value {
                                crate::scene::model::object::PropValue::Choice { options, .. } => Some((
                                    prop.field.to_string(),
                                    iced::widget::combo_box::State::new(options.clone()),
                                )),
                                _ => None,
                            })
                            .collect(),
                        sections,
                        title: format!("{} objects selected", selected.len()),
                        selection_group_combo: iced::widget::combo_box::State::new(groups.clone()),
                        selection_groups: groups,
                        selected_group: active_group,
                        layer_combo: iced::widget::combo_box::State::new(layer_names.clone()),
                        linetype_combo: iced::widget::combo_box::State::new(linetype_items.clone()),
                        hatch_pattern_combo: iced::widget::combo_box::State::new(
                            crate::scene::model::hatch_patterns::names(),
                        ),
                        lineweight_combo: iced::widget::combo_box::State::new(
                            ui::properties::lw_options(),
                        ),
                        linetype_items,
                        ..Default::default()
                    }
                }
            };
            panel.color_palette_open = color_palette_open;
            panel.edit_buf = edit_buf;
            panel
        };

        self.tabs[i].properties = new_panel;
        self.refresh_selected_grips();
        self.sync_ribbon_from_selection();
    }

    /// Drive the Home-ribbon Layer / Color / Linetype / Lineweight dropdowns
    /// from the current entity selection. With no selection the ribbon falls
    /// back to the active creation defaults (per-tab active_layer + ByLayer).
    /// Mixed selections keep the prior value (we'd need a UI "*Varies*"
    /// marker to do better).
    pub(super) fn sync_ribbon_from_selection(&mut self) {
        let i = self.active_tab;
        // The Start (welcome) tab has no document — keep the ribbon's
        // current-layer chip empty rather than re-seeding it with a default.
        if self.tabs[i].is_start {
            self.ribbon.active_layer = String::new();
            return;
        }
        let selected = self.tabs[i].scene.selected_entities();
        if selected.is_empty() {
            // Creation defaults: prefer the file's saved CECOLOR / CELTYPE /
            // CELWEIGHT (and current_layer_name); fall back to ByLayer when
            // those slots are still at their factory default.
            let header = &self.tabs[i].scene.document.header;
            let layer = if header.current_layer_name.is_empty() {
                self.tabs[i].active_layer.clone()
            } else {
                header.current_layer_name.clone()
            };
            self.ribbon.active_layer = layer;
            self.ribbon.active_color = header.current_entity_color;
            // current_linetype_name may be empty when only the handle was
            // written; resolve via line_types table in that case.
            let lt = if !header.current_linetype_name.is_empty() {
                header.current_linetype_name.clone()
            } else if !header.current_linetype_handle.is_null() {
                self.tabs[i]
                    .scene
                    .document
                    .line_types
                    .iter()
                    .find(|lt| lt.handle == header.current_linetype_handle)
                    .map(|lt| lt.name.clone())
                    .unwrap_or_else(|| "ByLayer".to_string())
            } else {
                "ByLayer".to_string()
            };
            self.ribbon.active_linetype = lt;
            self.ribbon.active_lineweight =
                acadrust::types::LineWeight::from_value(header.current_line_weight);
            return;
        }

        let mut layer: Option<String> = None;
        let mut color: Option<acadrust::types::Color> = None;
        let mut linetype: Option<String> = None;
        let mut lineweight: Option<acadrust::types::LineWeight> = None;
        let mut layer_mixed = false;
        let mut color_mixed = false;
        let mut linetype_mixed = false;
        let mut lineweight_mixed = false;

        for (_h, e) in &selected {
            let c = e.common();
            let lt = if c.linetype.is_empty() {
                "ByLayer".to_string()
            } else {
                c.linetype.clone()
            };
            match &layer {
                None => layer = Some(c.layer.clone()),
                Some(prev) if prev != &c.layer => layer_mixed = true,
                _ => {}
            }
            match &color {
                None => color = Some(c.color),
                Some(prev) if prev != &c.color => color_mixed = true,
                _ => {}
            }
            match &linetype {
                None => linetype = Some(lt),
                Some(prev) if prev != &lt => linetype_mixed = true,
                _ => {}
            }
            match &lineweight {
                None => lineweight = Some(c.line_weight),
                Some(prev) if prev != &c.line_weight => lineweight_mixed = true,
                _ => {}
            }
        }
        if !layer_mixed {
            if let Some(l) = layer {
                self.ribbon.active_layer = l;
            }
        }
        if !color_mixed {
            if let Some(c) = color {
                self.ribbon.active_color = c;
            }
        }
        if !linetype_mixed {
            if let Some(l) = linetype {
                self.ribbon.active_linetype = l;
            }
        }
        if !lineweight_mixed {
            if let Some(lw) = lineweight {
                self.ribbon.active_lineweight = lw;
            }
        }
    }

    /// Rebuild the cached selected_grips from the current entity selection.
    pub(super) fn refresh_selected_grips(&mut self) {
        let i = self.active_tab;
        let is_paper = self.tabs[i].scene.current_layout != "Model";
        // Paper-space entity coordinates are NOT offset by world_offset (same rule
        // as wire tessellation in wires_for_block). Only subtract in model space.
        let wo = if is_paper {
            [0.0f64; 3]
        } else {
            self.tabs[i].scene.world_offset
        };
        let (new_handle, new_grips) = {
            let selected = self.tabs[i].scene.selected_entities();
            if selected.len() == 1 {
                let (handle, entity) = selected[0];
                let grips = dispatch::grips(entity)
                    .into_iter()
                    .map(|mut g| {
                        // Subtract in f64: at UTM magnitudes an f32 cast before
                        // the offset costs ~1 unit and draws the grip off the
                        // wire.
                        g.world.x -= wo[0];
                        g.world.y -= wo[1];
                        g.world.z -= wo[2];
                        g
                    })
                    .collect();
                (Some(handle), grips)
            } else {
                (None, vec![])
            }
        };
        self.tabs[i].selected_handle = new_handle;
        self.tabs[i].selected_grips = new_grips;
        // Append the dynamic-block visibility (lookup) grip, if the lone
        // selection is a visibility-parametric block reference.
        self.refresh_visibility_grip(wo);
    }

    pub(super) fn property_target_handles(&self, i: usize) -> Vec<Handle> {
        let handles = self.tabs[i].properties.selected_handles();
        if !handles.is_empty() {
            handles
        } else {
            self.tabs[i].selected_handle.into_iter().collect()
        }
    }

    pub(super) fn invalidate_property_targets(&mut self, i: usize, handles: &[Handle]) {
        for &handle in handles {
            self.tabs[i].scene.mark_entity_dirty(handle);
        }
        self.tabs[i].scene.bump_geometry_no_blocks();
    }

    /// Add an entity to the correct space (model or paper space layout).
    pub(super) fn commit_entity(&mut self, entity: acadrust::EntityType) {
        let _ = self.commit_entity_handle(entity);
    }

    /// Like [`commit_entity`] but returns the handle the new entity was given
    /// (or `None` if it could not be added). Lets callers follow up — e.g.
    /// open the in-place text editor on a freshly created MultiLeader.
    pub(super) fn commit_entity_handle(
        &mut self,
        mut entity: acadrust::EntityType,
    ) -> Option<Handle> {
        let i = self.active_tab;
        let layer = &self.tabs[i].active_layer;
        if layer != "0" || entity.as_entity().layer().is_empty() {
            entity.as_entity_mut().set_layer(layer.clone());
        }

        // INSUNITS: when inserting a block whose BlockRecord.units differ
        // from the host's header.insertion_units, scale the new INSERT so
        // 1 source-unit equals the matching host length. When either side
        // is unitless (0) AutoCAD falls back to MEASUREMENT (0 = Imperial /
        // inches, 1 = Metric / mm); honour the same fallback.
        if let acadrust::EntityType::Insert(ref mut ins) = entity {
            let header = &self.tabs[i].scene.document.header;
            let measurement_fallback = if header.measurement == 1 { 4 } else { 1 };
            let host_raw = header.insertion_units;
            let host_units = if host_raw == 0 { measurement_fallback } else { host_raw };
            let src_raw = self.tabs[i]
                .scene
                .document
                .block_records
                .get(&ins.block_name)
                .map(|br| br.units)
                .unwrap_or(0);
            let src_units = if src_raw == 0 { measurement_fallback } else { src_raw };
            if src_units != host_units {
                let ratio = insunits_to_mm(src_units) / insunits_to_mm(host_units);
                if ratio.is_finite() && (ratio - 1.0).abs() > 1e-9 {
                    ins.set_x_scale(ratio);
                    ins.set_y_scale(ratio);
                    ins.set_z_scale(ratio);
                }
            }
        }

        crate::scene::view::dispatch::apply_color(&mut entity, self.ribbon.active_color);
        crate::scene::view::dispatch::apply_common_prop(
            &mut entity,
            "linetype",
            &self.ribbon.active_linetype.clone(),
        );
        crate::scene::view::dispatch::apply_line_weight(&mut entity, self.ribbon.active_lineweight);
        // CELTSCALE (header.current_entity_linetype_scale): new entities
        // pick up the document's saved per-entity linetype scale. The user
        // can override per entity later via the properties panel.
        let celtscale = self.tabs[i].scene.document.header.current_entity_linetype_scale;
        if (celtscale - 1.0).abs() > 1e-9 && celtscale.abs() > 1e-9 {
            entity.common_mut().linetype_scale = celtscale;
        }

        // A new dimension inherits the document's current dimension style
        // (DIMSTYLE) instead of staying at the entity "Standard" default. Only
        // fill in when still at the default so an explicitly-styled dimension
        // is preserved. See #92.
        if let acadrust::EntityType::Dimension(ref mut d) = entity {
            let cur = self.tabs[i]
                .scene
                .document
                .header
                .current_dimstyle_name
                .clone();
            let s = d.base().style_name.clone();
            if (s.is_empty() || s.eq_ignore_ascii_case("Standard")) && !cur.is_empty() {
                d.base_mut().style_name = cur;
            }
        }

        // MultiLeader / Table inherit the document's current style (#92). These
        // styles live in the objects dictionary, so resolve the current style
        // name to its object handle. Left untouched when the command already
        // assigned a style or no matching style object exists.
        match &mut entity {
            acadrust::EntityType::MultiLeader(ml) if ml.style_handle.is_none() => {
                let name = self.tabs[i]
                    .scene
                    .document
                    .header
                    .current_mleader_style_name
                    .clone();
                if !name.is_empty() {
                    let found =
                        self.tabs[i].scene.document.objects.iter().find_map(|(h, o)| match o {
                            acadrust::objects::ObjectType::MultiLeaderStyle(s)
                                if s.name.eq_ignore_ascii_case(&name) =>
                            {
                                Some((*h, s.clone()))
                            }
                            _ => None,
                        });
                    if let Some((h, s)) = found {
                        ml.style_handle = Some(h);
                        // Inherit the style's settings so a new multileader
                        // reflects the current MLeaderStyle (the renderer reads
                        // these entity fields). See #94.
                        // The entity and style enums are distinct types with
                        // matching discriminants — round-trip through i16.
                        ml.content_type = (s.content_type as i16).into();
                        ml.path_type = (s.path_type as i16).into();
                        ml.line_color = s.line_color;
                        ml.line_type_handle = s.line_type_handle;
                        ml.line_weight = s.line_weight;
                        ml.enable_landing = s.enable_landing;
                        ml.enable_dogleg = s.enable_dogleg;
                        ml.dogleg_length = s.landing_distance;
                        ml.arrowhead_handle = s.arrowhead_handle;
                        ml.arrowhead_size = s.arrowhead_size;
                        ml.text_style_handle = s.text_style_handle;
                        ml.text_color = s.text_color;
                        ml.text_frame = s.text_frame;
                        ml.text_height = s.text_height;
                        ml.context.text_height = s.text_height;
                        ml.text_left_attachment = (s.text_left_attachment as i16).into();
                        ml.text_right_attachment = (s.text_right_attachment as i16).into();
                        ml.text_top_attachment = (s.text_top_attachment as i16).into();
                        ml.text_bottom_attachment = (s.text_bottom_attachment as i16).into();
                        ml.text_attachment_direction =
                            (s.text_attachment_direction as i16).into();
                        ml.text_alignment = (s.text_alignment as i16).into();
                        ml.text_angle_type = (s.text_angle_type as i16).into();
                        ml.block_content_handle = s.block_content_handle;
                        ml.block_content_color = s.block_content_color;
                        ml.block_connection_type = (s.block_content_connection as i16).into();
                        ml.block_rotation = s.block_content_rotation;
                        ml.block_scale = acadrust::types::Vector3::new(
                            s.block_content_scale_x,
                            s.block_content_scale_y,
                            s.block_content_scale_z,
                        );
                        ml.scale_factor = s.scale_factor;
                    }
                }
            }
            acadrust::EntityType::Table(t) if t.table_style_handle.is_none() => {
                let name = self.tabs[i]
                    .scene
                    .document
                    .header
                    .current_table_style_name
                    .clone();
                if !name.is_empty() {
                    t.table_style_handle =
                        self.tabs[i].scene.document.objects.iter().find_map(|(h, o)| {
                            match o {
                                acadrust::objects::ObjectType::TableStyle(s)
                                    if s.name.eq_ignore_ascii_case(&name) =>
                                {
                                    Some(*h)
                                }
                                _ => None,
                            }
                        });
                }
            }
            _ => {}
        }

        // Commands pick points in local space (camera coordinates with world_offset
        // already subtracted). Re-add world_offset so the entity lands at the correct
        // DXF coordinate. Skip for paper-space entities (they use sheet mm coords).
        let is_paper = self.tabs[i].scene.current_layout != "Model";
        if !is_paper {
            let wo = self.tabs[i].scene.world_offset;
            if wo[0] != 0.0 || wo[1] != 0.0 || wo[2] != 0.0 {
                let delta = acadrust::types::Vector3::new(wo[0], wo[1], wo[2]);
                let t = acadrust::types::Transform::from_translation(delta);
                entity.apply_transform(&t);
            }
        }

        if matches!(&entity, acadrust::EntityType::Viewport(_))
            && self.tabs[i].scene.current_layout != "Model"
        {
            // Assign a unique viewport ID (max existing id + 1, min 2).
            if let acadrust::EntityType::Viewport(ref mut vp) = entity {
                let layout_block = self.tabs[i].scene.current_layout_block_handle_pub();
                let max_id = self.tabs[i]
                    .scene
                    .document
                    .entities()
                    .filter_map(|e| {
                        if let acadrust::EntityType::Viewport(v) = e {
                            if v.common.owner_handle == layout_block {
                                Some(v.id)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .max()
                    .unwrap_or(1);
                vp.id = (max_id + 1).max(2);
            }

            let layout = self.tabs[i].scene.current_layout.clone();
            match self.tabs[i]
                .scene
                .document
                .add_entity_to_layout(entity, &layout)
            {
                Ok(new_handle) => {
                    self.tabs[i].scene.auto_fit_viewport(new_handle);
                    Some(new_handle)
                }
                Err(e) => {
                    self.command_line
                        .push_error(&format!("Viewport could not be added: {e}"));
                    None
                }
            }
        } else {
            Some(self.tabs[i].scene.add_entity(entity))
        }
    }
}

// ── Multi-selection property aggregation ───────────────────────────────────

pub(super) fn build_selection_groups(
    selected: &[(Handle, &EntityType)],
) -> Vec<ui::properties::SelectionGroup> {
    let mut groups = vec![ui::properties::SelectionGroup {
        label: format!("All({})", selected.len()),
        handles: selected.iter().map(|(handle, _)| *handle).collect(),
    }];

    let mut by_type: std::collections::BTreeMap<String, Vec<Handle>> =
        std::collections::BTreeMap::new();
    for (handle, entity) in selected {
        by_type
            .entry(entity_type_key(entity))
            .or_default()
            .push(*handle);
    }

    for (kind, handles) in by_type {
        groups.push(ui::properties::SelectionGroup {
            label: format!("{}({})", title_case_word(&kind), handles.len()),
            handles,
        });
    }

    groups
}

pub(super) fn aggregate_sections(
    selected: &[(Handle, &EntityType)],
    text_style_names: &[String],
) -> Vec<crate::scene::model::object::PropSection> {
    if selected.is_empty() {
        return vec![];
    }

    let mut all_sections: Vec<Vec<crate::scene::model::object::PropSection>> = selected
        .iter()
        .map(|(handle, entity)| dispatch::properties_sectioned(*handle, entity, text_style_names))
        .collect();

    let mut result = all_sections.remove(0);
    for sections in all_sections {
        result = merge_sections(&result, &sections);
    }
    result
}

fn merge_sections(
    left: &[crate::scene::model::object::PropSection],
    right: &[crate::scene::model::object::PropSection],
) -> Vec<crate::scene::model::object::PropSection> {
    left.iter()
        .filter_map(|section| {
            let rhs = right
                .iter()
                .find(|candidate| candidate.title == section.title)?;
            let props: Vec<crate::scene::model::object::Property> = section
                .props
                .iter()
                .filter_map(|prop| {
                    let other = rhs
                        .props
                        .iter()
                        .find(|candidate| candidate.field == prop.field)?;
                    Some(crate::scene::model::object::Property {
                        label: prop.label.clone(),
                        field: prop.field,
                        value: merge_prop_value(&prop.value, &other.value),
                    })
                })
                .collect();
            if props.is_empty() {
                None
            } else {
                Some(crate::scene::model::object::PropSection {
                    title: section.title.clone(),
                    props,
                })
            }
        })
        .collect()
}

fn merge_prop_value(
    left: &crate::scene::model::object::PropValue,
    right: &crate::scene::model::object::PropValue,
) -> crate::scene::model::object::PropValue {
    use crate::scene::model::object::PropValue;

    if left == right {
        return left.clone();
    }

    match (left, right) {
        (PropValue::LayerChoice(_), PropValue::LayerChoice(_)) => {
            PropValue::LayerChoice(VARIES_LABEL.into())
        }
        (PropValue::ColorChoice(_), PropValue::ColorChoice(_))
        | (PropValue::ColorVaries, _)
        | (_, PropValue::ColorVaries) => PropValue::ColorVaries,
        (PropValue::LwChoice(_), PropValue::LwChoice(_))
        | (PropValue::LwVaries, _)
        | (_, PropValue::LwVaries) => PropValue::LwVaries,
        (PropValue::LinetypeChoice(_), PropValue::LinetypeChoice(_)) => {
            PropValue::LinetypeChoice(VARIES_LABEL.into())
        }
        (
            PropValue::Choice { options, .. },
            PropValue::Choice {
                options: other_options,
                ..
            },
        ) if options == other_options => PropValue::Choice {
            selected: VARIES_LABEL.into(),
            options: options.clone(),
        },
        (PropValue::EditText(_), PropValue::EditText(_)) => {
            PropValue::EditText(VARIES_LABEL.into())
        }
        (PropValue::ReadOnly(_), PropValue::ReadOnly(_)) => {
            PropValue::ReadOnly(VARIES_LABEL.into())
        }
        (PropValue::HatchPatternChoice(_), PropValue::HatchPatternChoice(_)) => {
            PropValue::HatchPatternChoice(VARIES_LABEL.into())
        }
        (
            PropValue::BoolToggle { field, .. },
            PropValue::BoolToggle {
                field: other_field, ..
            },
        ) if field == other_field => PropValue::ReadOnly(VARIES_LABEL.into()),
        _ => left.clone(),
    }
}

/// Convert INSUNITS (DXF group 70) to millimetres.
/// 0 = unitless / unknown: returns 1.0 so the caller treats it as "do not scale".
fn insunits_to_mm(code: i16) -> f64 {
    match code {
        1 => 25.4,            // Inches
        2 => 304.8,           // Feet
        3 => 1_609_344.0,     // Miles
        4 => 1.0,             // Millimeters
        5 => 10.0,            // Centimeters
        6 => 1_000.0,         // Meters
        7 => 1_000_000.0,     // Kilometers
        8 => 0.000_025_4,     // Microinches
        9 => 0.025_4,         // Mils
        10 => 914.4,          // Yards
        11 => 1.0e-7,         // Angstroms
        12 => 1.0e-6,         // Nanometers
        13 => 0.001,          // Microns
        14 => 100.0,          // Decimeters
        15 => 10_000.0,       // Decameters
        16 => 100_000.0,      // Hectometers
        17 => 1.0e12,         // Gigameters
        18 => 1.496e14,       // Astronomical Units
        19 => 9.461e18,       // Light Years
        20 => 3.086e19,       // Parsecs
        21 => 304.800_609_6,  // US Survey Feet
        _ => 1.0,
    }
}
