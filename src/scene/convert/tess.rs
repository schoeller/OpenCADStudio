// Auto-split from scene/mod.rs. Pure text-move; behaviour unchanged.
use super::super::*;

// ── Parallel tessellation free function ──────────────────────────────────────
//
// Takes only the `Send + Sync` data needed for tessellation so that
// `wires_for_block` can dispatch work across rayon's thread pool without
// requiring `Scene` (which contains `Rc<RefCell<...>>` and is `!Send`) to
// cross thread boundaries.


/// Tessellate a synthesised dimension-text entity through `tessellate_entity`
/// so it picks up the standard text LOD ladder (baseline / greek / full),
/// then re-color the returned wires with the dimension's resolved text colour
/// (so DIMCLRT / DIMSTYLE colours win over the synthetic Text's defaults).
pub(crate) fn tessellate_entity_dim_text(
    document: &acadrust::CadDocument,
    selected: &HashSet<Handle>,
    active_viewport: Option<Handle>,
    bg_color: [f32; 4],
    anno_scale: f32,
    e: &EntityType,
    view_aabb: Option<[f32; 4]>,
    world_per_pixel: Option<f32>,
    text_color: [f32; 4],
) -> Vec<WireModel> {
    let mut wires = tessellate_entity(
        document, selected, active_viewport, bg_color,
        anno_scale, e, None, view_aabb, world_per_pixel,
    );
    for w in &mut wires {
        // Synth dim text carries no real entity colour — paint everything
        // (including greek-LOD fill tris which read `wire.color`) with the
        // dim's text colour. Selection highlight already baked in by
        // tessellate_entity, so leave that alone.
        if !w.selected {
            w.color = text_color;
        }
    }
    wires
}

pub(crate) fn tessellate_entity(
    document: &acadrust::CadDocument,
    selected: &HashSet<Handle>,
    active_viewport: Option<Handle>,
    bg_color: [f32; 4],
    anno_scale: f32,
    e: &EntityType,
    block_cache: Option<&cache::block_cache::BlockCache>,
    // World-space XY view AABB (post `world_offset` subtraction). When
    // `Some`, entities whose AABB doesn't intersect this rect are skipped.
    view_aabb: Option<[f32; 4]>,
    // World units per screen pixel for LOD culling. `None` = no LOD.
    world_per_pixel: Option<f32>,
) -> Vec<WireModel> {
    let h = e.common().handle;
    let sel = selected.contains(&h);

    // Frustum + LOD cull for non-Insert, non-Viewport entities. Insert is
    // handled separately (its WCS bbox depends on the block defn AABB ×
    // Insert transform — done inside expand_insert). Viewports always emit
    // so the viewport frame stays visible regardless of zoom.
    let needs_cull = view_aabb.is_some() || world_per_pixel.is_some();
    if needs_cull {
        match e {
            EntityType::Viewport(_) | EntityType::Insert(_) => {}
            _ => {
                let ab = entity_aabb(e);
                if ab != WireModel::UNBOUNDED_AABB {
                    if let Some(view) = view_aabb {
                        if cache::block_cache::aabb_disjoint_xy(ab, view) {
                            return vec![];
                        }
                    }
                    if let Some(wpp) = world_per_pixel {
                        let w_px = (ab[2] - ab[0]).abs();
                        let h_px = (ab[3] - ab[1]).abs();
                        // Keep in sync with `cache::block_cache::MIN_PIXEL_SIZE`.
                        // Text/MText have their own LOD ladder below
                        // (baseline-line / greek / full) and must reach it
                        // even when projected size is sub-5 px.
                        let is_text = matches!(e, EntityType::Text(_) | EntityType::MText(_));
                        // Face3D is exempt from the sub-pixel stub: it is trivially
                        // cheap to tessellate (4 corners → 2 tris), so there is no
                        // cost to draw it full at any zoom, and the cube-stub
                        // otherwise pops/coarsens flat faces across the threshold.
                        let is_face3d = matches!(e, EntityType::Face3D(_));
                        let is_3d_entity = matches!(
                            e,
                            EntityType::Solid3D(_)
                                | EntityType::Mesh(_)
                                | EntityType::PolyfaceMesh(_)
                                | EntityType::PolygonMesh(_)
                                | EntityType::Body(_)
                                | EntityType::Region(_)
                                | EntityType::Surface(_)
                        );
                        if !is_text && !is_face3d && w_px.max(h_px) / wpp < 5.0 {
                            // Sub-pixel entity: emit a stub instead of
                            // nothing so it stays visible / selectable /
                            // hit-test'able at any zoom. 2-D entities
                            // get the cheap diagonal segment; 3-D
                            // entities get an AABB cube so their
                            // footprint doesn't drift when the camera
                            // crosses the LOD threshold. See #19.
                            let (entity_color, _, _, _, aci_idx) =
                                view::render::render_style_for(document, e);
                            let entity_color = view::render::adapt_to_bg(entity_color, bg_color);
                            if is_3d_entity {
                                // `ab` is already in the local frame
                                // (entity_aabb subtracted world_offset
                                // XY). The bbox z fields are still in
                                // WCS, so subtract `world_offset[2]` to
                                // match — otherwise the stub sits at a
                                // different z than the full tessellation
                                // and the geometry visibly shifts when
                                // the camera crosses the LOD threshold.
                                let bbox = e.as_entity().bounding_box();
                                let oz = 0.0_f64;
                                let z_min = (bbox.min.z - oz) as f32;
                                let z_max = (bbox.max.z - oz) as f32;
                                return vec![lod_stub_wire_3d(
                                    h.value().to_string(),
                                    entity_color,
                                    sel,
                                    aci_idx,
                                    ab,
                                    z_min,
                                    z_max,
                                )];
                            }
                            return vec![lod_stub_wire(
                                h.value().to_string(),
                                entity_color,
                                sel,
                                aci_idx,
                                ab,
                                0.0,
                                0.0,
                            )];
                        }
                    }
                }
            }
        }
    }

    if let EntityType::Viewport(vp) = e {
        // The sheet viewport (overall/id=1) is never shown — it represents the
        // paper boundary, not a user-defined content window.
        if !Scene::is_content_viewport(vp) {
            return vec![];
        }
        let is_active = active_viewport == Some(h);
        let is_locked = vp.status.locked;
        let color = if sel {
            [1.0, 1.0, 1.0, 1.0]
        } else if is_active {
            [1.0, 0.90, 0.20, 1.0]
        } else if is_locked {
            [0.90, 0.55, 0.10, 1.0]
        } else {
            [0.0, 0.75, 0.75, 1.0]
        };
        let (pattern_length, pattern) = if is_active {
            (1.5_f32, [0.8, -0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0_f32])
        } else {
            (0.0_f32, [0.0f32; 8])
        };
        let mut wires = convert::tessellate::tessellate(
            document,
            h,
            e,
            sel,
            color,
            pattern_length,
            pattern,
            1.5,
            1.0,
            world_per_pixel,
        );
        let ab = entity_aabb(e);
        for w in &mut wires {
            w.aabb = ab;
        }
        return wires;
    }

    let (entity_color, pattern_length, pattern, line_weight_px, aci) =
        view::render::render_style_for(document, e);
    let entity_color = view::render::adapt_to_bg(entity_color, bg_color);
    let lt_scale = document.header.linetype_scale as f32 * e.common().linetype_scale as f32;
    let lt_name = view::render::linetype_name_for(document, e);
    // PSLTSCALE: scale linetype dashes by viewport anno_scale so they appear uniform in paper space.
    let pslt_factor = if document.header.paper_space_linetype_scaling {
        anno_scale
    } else {
        1.0
    };
    let pattern_length = pattern_length * pslt_factor;
    let pattern = pattern.map(|v| v * pslt_factor);

    // ── Dimension baked-block fast path ─────────────────────────────────────
    //
    // AutoCAD bakes each dimension's final geometry (extension lines, dim
    // line, arrows, text MText) into a per-instance block — usually
    // `*D<n>`, but custom names like `DIMBLOCK###-4NP` also occur. When the
    // block exists we render its contents through `tessellate_entity` so
    // sub-Text/MText get the standard baseline/greek/full LOD ladder, and
    // DIMTXT × DIMSCALE isn't re-applied on already-baked geometry.
    if let EntityType::Dimension(dim) = e {
        let block_name = &dim.base().block_name;
        if !block_name.trim().is_empty() {
            if let Some(br) = document
                .block_records
                .iter()
                .find(|br| br.name.eq_ignore_ascii_case(block_name))
            {
                if !br.entity_handles.is_empty() {
                    let mut wires: Vec<WireModel> =
                        Vec::with_capacity(br.entity_handles.len());
                    for &eh in &br.entity_handles {
                        let Some(sub) = document.get_entity(eh) else { continue };
                        // Sub-entities inside *D### / DIMBLOCK## blocks
                        // typically use ByBlock color/linetype/lineweight —
                        // they should inherit from the Dimension entity.
                        let sub_color_is_byblock =
                            sub.common().color == acadrust::types::Color::ByBlock;
                        let sub_wires = tessellate_entity(
                            document, selected, active_viewport, bg_color,
                            // Block contents are baked at the final WCS size —
                            // don't let downstream paths re-apply anno_scale.
                            1.0, sub, block_cache, view_aabb, world_per_pixel,
                        );
                        for mut w in sub_wires {
                            w.name = h.value().to_string();
                            // Override ByBlock colour with the dim's resolved
                            // colour so text matches `DIMCLRT`-style behaviour
                            // (or layer colour) instead of the raw ByBlock
                            // fallback that render_style_for produces.
                            if sub_color_is_byblock {
                                w.color = if sel { WireModel::SELECTED } else { entity_color };
                                w.aci = aci;
                            }
                            wires.push(w);
                        }
                    }
                    if !wires.is_empty() {
                        let aabb = entity_aabb(e);
                        for w in &mut wires {
                            w.aabb = aabb;
                        }
                        return wires;
                    }
                }
            }
        }
        // Fall through to the synthesis path below when no block is attached.
    }

    if let EntityType::Dimension(dim) = e {
        let aabb = entity_aabb(e);
        use crate::entities::dimension::DimensionTess;
        let mut wires = dim.tessellate(
            document,
            h,
            sel,
            entity_color,
            line_weight_px,
            anno_scale,
            selected,
            active_viewport,
            bg_color,
            view_aabb,
            world_per_pixel,
        );
        for w in &mut wires {
            w.aci = aci;
            w.aabb = aabb;
        }
        return wires;
    }

    if let EntityType::MultiLeader(ml) = e {
        let aabb = entity_aabb(e);
        use crate::entities::multileader::MultiLeaderTess;
        let mut wires = ml.tessellate(
            document,
            h,
            sel,
            entity_color,
            line_weight_px,
            anno_scale,
            world_per_pixel,
        );
        for w in &mut wires {
            w.aci = aci;
            w.aabb = aabb;
        }
        return wires;
    }

    // ── Table baked-block fast path ─────────────────────────────────────────
    //
    // AutoCAD bakes a Table's final rendered geometry (cell text, gridlines,
    // fill) into a per-instance block (usually `*T###`) referenced through
    // `table.block_record_handle`. The block's text uses the *displayed*
    // height; synthesising cells from `self.rows + TableStyle` instead would
    // re-apply the table's scale factor on top of already-baked geometry.
    // When the block exists we render it directly. Same pattern as
    // Dimension's `block_name`.
    if let EntityType::Table(tab) = e {
        if let Some(br_h) = tab.block_record_handle {
            if let Some(br) = document
                .block_records
                .iter()
                .find(|br| br.handle == br_h)
            {
                if !br.entity_handles.is_empty() {
                    let mut wires: Vec<WireModel> =
                        Vec::with_capacity(br.entity_handles.len());
                    for &eh in &br.entity_handles {
                        let Some(sub) = document.get_entity(eh) else { continue };
                        let sub_color_is_byblock =
                            sub.common().color == acadrust::types::Color::ByBlock;
                        let sub_wires = tessellate_entity(
                            document, selected, active_viewport, bg_color,
                            anno_scale, sub, block_cache, view_aabb, world_per_pixel,
                        );
                        for mut w in sub_wires {
                            w.name = h.value().to_string();
                            if sub_color_is_byblock {
                                w.color = if sel { WireModel::SELECTED } else { entity_color };
                                w.aci = aci;
                            }
                            wires.push(w);
                        }
                    }
                    if !wires.is_empty() {
                        let aabb = entity_aabb(e);
                        for w in &mut wires {
                            w.aabb = aabb;
                        }
                        return wires;
                    }
                }
            }
        }
        // No baked block (e.g. a table created in-app) — synthesise coloured
        // geometry from the rows + TableStyle so fills/colours/borders/margins
        // are honoured instead of the monochrome fallback.
        let mut wires = crate::entities::table::tessellate_table(
            tab, document, sel, entity_color, line_weight_px,
        );
        if !wires.is_empty() {
            let aabb = entity_aabb(e);
            for w in &mut wires {
                w.aci = aci;
                w.aabb = aabb;
            }
            return wires;
        }
    }

    if let EntityType::Insert(ins) = e {
        // Resolve the INSERT's own style so ByBlock sub-entities can inherit it.
        let (ins_color, ins_pat_len, ins_pat, ins_lw_px, _) = view::render::render_style_for(document, e);
        let ins_color = view::render::adapt_to_bg(ins_color, bg_color);
        let ip = glam::Vec3::new(
            (ins.insert_point.x) as f32,
            (ins.insert_point.y) as f32,
            (ins.insert_point.z) as f32,
        );
        let marker = WireModel {
            name: h.value().to_string(),
            points: vec![],
            points_low: Vec::new(),
            color: entity_color,
            selected: sel,
            aci: 0,
            pattern_length: 0.0,
            pattern: [0.0; 8],
            line_weight_px: 1.0,
            snap_pts: vec![(ip.as_dvec3(), model::wire_model::SnapHint::Insertion)],
            tangent_geoms: vec![],
            key_vertices: vec![],
            aabb: WireModel::UNBOUNDED_AABB,
            plinegen: true,
            vp_scissor: None,
            fill_tris: vec![],
            fill_tris_low: Vec::new(),
        };

        if let Some(cache) = block_cache {
            // Xrefs render with the same hue but faded toward `bg_color` so
            // the user can recognise external-reference geometry at a glance.
            let is_xref = document
                .block_records
                .get(&ins.block_name)
                .map(|br| br.flags.is_xref || br.flags.is_xref_overlay)
                .unwrap_or(false);
            if let Some(arc) = cache::block_cache::expand_insert(
                cache,
                ins,
                h,
                ins_color,
                ins_pat_len,
                ins_pat,
                ins_lw_px,
                sel,
                pslt_factor,
                view_aabb,
                world_per_pixel,
                is_xref,
                bg_color,
            ) {
                let mut wires: Vec<WireModel> = arc.iter().cloned().collect();                // XCLIP: if this INSERT carries an enabled spatial filter,
                // clip the expanded block geometry to the boundary polygon so
                // only the portion inside the clip is drawn.
                if let Some(sf) = pick::xclip::insert_spatial_filter(document, ins) {
                    let poly = pick::xclip::world_clip_polygon_f64(sf, ins);
                    pick::xclip::clip_wires(&mut wires, &poly);
                }

                // Per-INSERT attribute values. The block defn carries the
                // AttributeDefinitions (templates) which expand_insert skips;
                // the AttributeEntity instances live on the Insert itself in
                // WCS and need their own tessellation so the user sees the
                // values they actually filled in. See #20.
                crate::entities::insert::append_insert_attribute_wires(
                    &mut wires,
                    document,
                    ins,
                    h,
                    sel,
                    ins_color,
                    ins_pat_len,
                    ins_pat,
                    ins_lw_px,
                    bg_color,
                    is_xref,
                    pslt_factor,
                    anno_scale,
                );
                wires.push(marker);
                return wires;
            }
        }

        // Cache miss / unavailable: fall back to the original explode path.
        // The block_cache primary path covers all typical Inserts; this
        // branch only fires for pathological cache failures.
        let br = document.block_records.get(&ins.block_name);
        let is_xref = br
            .map(|br| br.flags.is_xref || br.flags.is_xref_overlay)
            .unwrap_or(false);
        let mut wires: Vec<WireModel> = ins
            .explode_from_document(document)
            .iter()
            .cloned()
            .map(crate::modules::draw::modify::explode::normalize_insert_entity)
            .flat_map(|sub| {
                let (sub_color, sub_pattern_length, sub_pattern, sub_line_weight_px, sub_aci) =
                    view::render::render_style_for_block_sub(
                        document,
                        &sub,
                        ins_color,
                        ins_pat_len,
                        ins_pat,
                        ins_lw_px,
                    );
                let sub_color = view::render::adapt_to_bg(sub_color, bg_color);
                let sub_color = if is_xref && !sel {
                    cache::block_cache::fade_toward_bg(sub_color, bg_color)
                } else {
                    sub_color
                };
                let sub_aabb = entity_aabb(&sub);
                let sub_pattern_length = sub_pattern_length * pslt_factor;
                let sub_pattern = sub_pattern.map(|v| v * pslt_factor);
                let mut wires = convert::tessellate::tessellate(
                    document,
                    h,
                    &sub,
                    sel,
                    sub_color,
                    sub_pattern_length,
                    sub_pattern,
                    sub_line_weight_px,
                    anno_scale,
                    world_per_pixel,
                );
                for w in &mut wires {
                    w.name = h.value().to_string();
                    w.aci = sub_aci;
                    w.aabb = sub_aabb;
                }
                wires
            })
            .collect();
        crate::entities::insert::append_insert_attribute_wires(
            &mut wires,
            document,
            ins,
            h,
            sel,
            ins_color,
            ins_pat_len,
            ins_pat,
            ins_lw_px,
            bg_color,
            is_xref,
            pslt_factor,
            anno_scale,
        );
        wires.push(marker);
        return wires;
    }

    let aabb = entity_aabb(e);

    // Text-specific LOD ladder, keyed off the entity's glyph height in
    // pixels (anno-scaled):
    //   < 1 px  → baseline line in the text's color (text-here hint)
    //   1–5 px  → greeked OBB rect in the text's color
    //   ≥ 5 px  → full per-glyph stroke tessellation
    //
    // Applies to every entity that is "primarily a piece of text" — Text,
    // MText, ATTDEF, ATTRIB, Tolerance — so far-out drawings don't pay the
    // full glyph-tessellation cost. Composite entities (Dimension, Table,
    // MultiLeader) carry non-text geometry and have their own LOD paths.
    if let Some(wpp) = world_per_pixel {
        let text_height: Option<f64> = match e {
            EntityType::Text(t) => Some(t.height * anno_scale as f64),
            EntityType::MText(m) => Some(m.height * anno_scale as f64),
            EntityType::AttributeDefinition(a) => Some(a.height * anno_scale as f64),
            EntityType::AttributeEntity(a) => Some(a.height * anno_scale as f64),
            EntityType::Tolerance(t) => {
                // Tolerance text_height defaults to 0.18 from creation; treat
                // 0 as missing and fall back to the AutoCAD default so the
                // pixel check still kicks in for legitimately tiny dimensions.
                let raw = if t.text_height > 0.0 { t.text_height } else { 2.5 };
                Some(raw * anno_scale as f64)
            }
            _ => None,
        };
        if let Some(h_world) = text_height {
            let h_px = (h_world as f32) / wpp;
            // Wrap-expanded line count for MText (Text = 1).
            let n_lines = match e {
                EntityType::MText(m) => {
                    crate::entities::text_support::mtext_line_count(m, document, anno_scale)
                }
                _ => 1,
            };
            if h_px < 1.0 {
                let pts = crate::entities::text_support::text_baseline_points(e, anno_scale, n_lines);
                if pts.len() < 2 {
                    return vec![];
                }
                // Skip the baseline too if the line itself projects under
                // 2 px (e.g. a 1-char text seen edge-on). All wrap lines
                // share the same baseline length, so the first segment is
                // a representative sample.
                let dx = pts[1][0] - pts[0][0];
                let dy = pts[1][1] - pts[0][1];
                let len_px = (dx * dx + dy * dy).sqrt() / wpp;
                if len_px < 2.0 {
                    // Text projects to under 2 px — fall back to the
                    // generic LOD stub so the entity stays visible /
                    // selectable. #19. Text is 2-D in the XY plane so
                    // z_min = z_max = 0 keeps the historical behaviour.
                    return vec![lod_stub_wire(
                        h.value().to_string(),
                        entity_color,
                        sel,
                        aci,
                        aabb,
                        0.0,
                        0.0,
                    )];
                }
                return vec![WireModel {
                    name: h.value().to_string(),
                    points: pts,
                    points_low: Vec::new(),
                    color: entity_color,
                    selected: sel,
                    aci,
                    pattern_length: 0.0,
                    pattern: [0.0; 8],
                    line_weight_px: 1.0,
                    snap_pts: vec![],
                    tangent_geoms: vec![],
                    key_vertices: vec![],
                    aabb,
                    plinegen: true,
                    vp_scissor: None,
                    fill_tris: vec![],
                    fill_tris_low: Vec::new(),
                }];
            }
            if h_px < 5.0 && aabb != WireModel::UNBOUNDED_AABB {
                let fill_tris = crate::entities::text_support::text_greek_obb_tris(e, anno_scale, n_lines);
                if fill_tris.is_empty() {
                    // Text greek fallback: also 2-D, keep stub at z=0.
                    return vec![lod_stub_wire(
                        h.value().to_string(),
                        entity_color,
                        sel,
                        aci,
                        aabb,
                        0.0,
                        0.0,
                    )];
                }
                // Greek text renders via the face3d fill batch, which colours
                // each tri with `wire.color`. Bake the selected colour in so
                // a selected text stays highlighted across the LOD boundary.
                // hit_test's AABB fallback handles window / crossing. #19.
                let fill_color = if sel { WireModel::SELECTED } else { entity_color };
                return vec![WireModel {
                    name: h.value().to_string(),
                    points: vec![],
                    points_low: Vec::new(),
                    color: fill_color,
                    selected: sel,
                    aci,
                    pattern_length: 0.0,
                    pattern: [0.0; 8],
                    line_weight_px: 1.0,
                    snap_pts: vec![],
                    tangent_geoms: vec![],
                    key_vertices: vec![],
                    aabb,
                    plinegen: true,
                    vp_scissor: None,
                    fill_tris,
                    fill_tris_low: Vec::new(),
                }];
            }
        }
    }

    let mut bases = convert::tessellate::tessellate(
        document,
        h,
        e,
        sel,
        entity_color,
        pattern_length,
        pattern,
        line_weight_px,
        anno_scale,
        world_per_pixel,
    );
    for b in &mut bases {
        b.aci = aci;
        b.aabb = aabb;
    }

    // Complex linetypes (with embedded shapes / text) expand the *base*
    // polyline along its tangent. Text-type entities never have a complex
    // linetype assigned, so we only consult the first wire here — multi-wire
    // returns come exclusively from MTEXT colour splits which can't trigger
    // this path.
    if let Some(clt) = crate::io::linetypes::complex_lt(lt_name) {
        if let Some(base) = bases.first() {
            let mut wires = text::complex_lt::apply_along(
                &base.name,
                &base.points,
                clt,
                (lt_scale * pslt_factor).max(1e-4),
                entity_color,
                sel,
                base.line_weight_px,
            );
            if !wires.is_empty() {
                for w in &mut wires {
                    w.aabb = aabb;
                }
                return wires;
            }
        }
    }

    bases
}

/// Build the 4 OBB corners (CCW: bl, br, tr, tl) of a Text / MText entity
/// in its **native frame** — for top-level entities this is world coords,
/// for block-defn subs it's block-local. No offset/transform applied.
/// Width is approximated from glyph height × character count (TEXT) or
/// from `rectangle_width` (MTEXT). Returns `None` for non-text entities.
///
/// `mtext_lines_override` lets the caller plug in a wrap-aware line count
/// (from `text_support::mtext_line_count`). Without it, MText's OBB
/// height collapses to a single line when the file omits `rectangle_height`,
/// which makes downstream per-line LOD math degenerate.

/// Build a "low-LOD stub" wire for an entity that would otherwise be culled
/// to nothing — the entity's AABB diagonal as a 2-point segment, plus the
/// AABB itself so window / crossing selection picks the entity up. The
/// stored `selected` flag tracks across zoom levels so highlight visuals
/// don't disappear when the LOD level changes. See #19.
fn lod_stub_wire(
    name: String,
    color: [f32; 4],
    selected: bool,
    aci: u8,
    aabb: [f32; 4],
    z_min: f32,
    z_max: f32,
) -> WireModel {
    let [ax, ay, bx, by] = aabb;
    let cx = (ax + bx) * 0.5;
    let cy = (ay + by) * 0.5;
    let cz = (z_min + z_max) * 0.5;
    // Mirror what tessellate.rs does for the non-stub paths: bake the
    // selection-highlight colour into the wire so a re-tessellate triggered
    // by a zoom-induced LOD change keeps the entity highlighted. Without
    // this swap the wire's `selected` flag is true but its colour stays at
    // the entity's own hue, so the user sees the highlight vanish at the
    // LOD boundary. #19.
    let stored_color = if selected { WireModel::SELECTED } else { color };
    WireModel {
        name,
        // Diagonal of the entity's 3D AABB so depth tests against
        // shaded / hidden-line geometry are correct — the stub doesn't
        // flatten to z=0 and pop in front of objects that sit at a
        // different elevation. 2D entities (text fallbacks) pass
        // z_min = z_max = 0 to keep the historical behaviour.
        points: vec![[ax, ay, z_min], [bx, by, z_max]],
        points_low: Vec::new(),
        color: stored_color,
        selected,
        aci,
        pattern_length: 0.0,
        pattern: [0.0; 8],
        line_weight_px: 1.0,
        snap_pts: vec![],
        tangent_geoms: vec![],
        key_vertices: vec![[cx as f64, cy as f64, cz as f64]],
        aabb,
        plinegen: true,
        vp_scissor: None,
        fill_tris: vec![],
        fill_tris_low: Vec::new(),
    }
}

/// Sub-pixel LOD stub for 3D entities. Emits the entity's 3D AABB as a
/// 12-edge cube so the geometry occupies the same screen footprint and
/// depth range as the full tessellation, just with a tiny constant cost
/// (12 line segments). Without this, the diagonal stub used by
/// `lod_stub_wire` cuts off at two opposite bbox corners and drifts
/// visibly when the camera crosses the LOD threshold.
fn lod_stub_wire_3d(
    name: String,
    color: [f32; 4],
    selected: bool,
    aci: u8,
    aabb: [f32; 4],
    z_min: f32,
    z_max: f32,
) -> WireModel {
    let [x0, y0, x1, y1] = aabb;
    let (z0, z1) = if z_min <= z_max { (z_min, z_max) } else { (z_max, z_min) };
    let p = [
        [x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
        [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1],
    ];
    // 12 edges = 4 bottom-face + 4 top-face + 4 vertical connectors.
    const EDGES: [(usize, usize); 12] = [
        (0, 1), (1, 2), (2, 3), (3, 0),
        (4, 5), (5, 6), (6, 7), (7, 4),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ];
    let mut points: Vec<[f32; 3]> = Vec::with_capacity(EDGES.len() * 3);
    for (a, b) in EDGES {
        if !points.is_empty() {
            points.push([f32::NAN; 3]);
        }
        points.push(p[a]);
        points.push(p[b]);
    }
    let stored_color = if selected { WireModel::SELECTED } else { color };
    WireModel {
        name,
        points,
        points_low: Vec::new(),
        color: stored_color,
        selected,
        aci,
        pattern_length: 0.0,
        pattern: [0.0; 8],
        line_weight_px: 1.0,
        snap_pts: vec![],
        tangent_geoms: vec![],
        // No `key_vertices` — Face3DGpu requires 4 corners to emit a
        // fill quad, and we don't want this stub painted as a solid
        // face. The wire pass still draws its 12 edges.
        key_vertices: vec![],
        aabb,
        plinegen: true,
        vp_scissor: None,
        fill_tris: vec![],
        fill_tris_low: Vec::new(),
    }
}

/// Tessellate each visible AttributeEntity attached to an Insert and append
/// the resulting wires. AttributeEntity positions are already in WCS — the
/// INSERT only stamps the geometry once, attribute text sits at the world
/// position recorded on each ATTRIB. See #20.
#[allow(clippy::too_many_arguments)]
pub(crate) fn entity_aabb(e: &acadrust::EntityType) -> [f32; 4] {
    let bbox = e.as_entity().bounding_box();
    let min_x = (bbox.min.x) as f32;
    let min_y = (bbox.min.y) as f32;
    let max_x = (bbox.max.x) as f32;
    let max_y = (bbox.max.y) as f32;
    // A degenerate box (min == max == 0) means bounding_box() returned Default —
    // use UNBOUNDED so the wire is never wrongly pre-rejected.
    if min_x == max_x && min_y == max_y {
        return WireModel::UNBOUNDED_AABB;
    }
    [min_x, min_y, max_x, max_y]
}

/// AABB of `e` in WCS f64 (no world_offset subtraction). `None` for
/// entities whose `bounding_box()` returned the degenerate default
/// (which `entity_aabb` collapses to `UNBOUNDED_AABB`). Quadtree
/// indexing uses this so changing `world_offset` doesn't invalidate
/// the index.
pub(crate) fn entity_world_aabb_f64(e: &acadrust::EntityType) -> Option<[f64; 4]> {
    let bbox = e.as_entity().bounding_box();
    let (xmin, ymin, xmax, ymax) = (bbox.min.x, bbox.min.y, bbox.max.x, bbox.max.y);
    if xmin == xmax && ymin == ymax {
        return None;
    }
    if !xmin.is_finite() || !ymin.is_finite() || !xmax.is_finite() || !ymax.is_finite() {
        return None;
    }
    Some([xmin, ymin, xmax, ymax])
}

/// True if `e` is a type the quadtree should skip. `Insert` and
/// `Viewport` are sized only after extra transformation; tessellation
/// already handles them via dedicated code paths. `Block`/`BlockEnd`
/// are block-defn sentinels with no geometry.
pub(crate) fn is_unindexable_entity(e: &acadrust::EntityType) -> bool {
    use acadrust::EntityType as E;
    matches!(
        e,
        E::Insert(_) | E::Viewport(_) | E::Block(_) | E::BlockEnd(_)
    )
}
