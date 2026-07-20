// Tessellation — convert acadrust EntityType to GPU-ready WireModel or MeshModel.
//
// Flow:
//   EntityType
//     ↓  acad_to_truck::convert()
//   TruckEntity  { object: TruckObject, snap_pts, tangent_geoms, key_vertices }
//     ↓  truck_tess::tessellate_*()
//   TruckTessResult::Lines → WireModel
//   TruckTessResult::Point → WireModel (small cross)
//   TruckTessResult::Mesh  → MeshModel
//   TruckObject::Text      → one WireModel per glyph stroke (elevation from entity Z)
//
// Entities not handled by acad_to_truck (Viewport, Insert, Hatch, Ole2Frame)
// are tessellated by the FallbackTess fallback_geometry() path.

use crate::entities::leader::LeaderTess;
use acadrust::types::Color as AcadColor;
use acadrust::{CadDocument, EntityType, Handle};
use glam::Vec3;

use crate::scene::convert::acad_to_truck::{convert, TruckObject};
use crate::scene::convert::truck_tess::{
    tessellate_edge, tessellate_vertex, tessellate_wire, TruckTessResult,
};
use crate::scene::model::wire_model::{SnapHint, WireModel};

/// Split an f64 offset-relative coordinate into the double-single (high, low)
/// f32 pair the renderer consumes. `high + low ≈ value` to ~f64 precision; the
/// RTE shader subtracts the eye's own high/low so vertices stay smooth even at
/// coordinates where a plain f32 cast would quantize to half a metre.
#[inline]
fn split_ds(v: f64) -> (f32, f32) {
    let h = v as f32;
    let l = (v - h as f64) as f32;
    (h, l)
}

#[inline]
fn split_ds_xyz(x: f64, y: f64, z: f64) -> ([f32; 3], [f32; 3]) {
    let (xh, xl) = split_ds(x);
    let (yh, yl) = split_ds(y);
    let (zh, zl) = split_ds(z);
    ([xh, yh, zh], [xl, yl, zl])
}

/// Split each absolute f64 source point into double-single (high, low) f32
/// buffers in one pass — the relative-to-eye residual the GPU/CPU reconstruct
/// to f64 precision at UTM-scale coordinates.
pub(crate) fn points_to_ds(
    src: impl IntoIterator<Item = [f64; 3]>,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let it = src.into_iter();
    let (lo, hi) = it.size_hint();
    let cap = hi.unwrap_or(lo);
    let mut high = Vec::with_capacity(cap);
    let mut low = Vec::with_capacity(cap);
    for [x, y, z] in it {
        if x.is_nan() {
            // Wire-model NaN-separator: keep both buffers index-paired.
            high.push([f32::NAN; 3]);
            low.push([0.0; 3]);
            continue;
        }
        let (h, l) = split_ds_xyz(x, y, z);
        high.push(h);
        low.push(l);
    }
    (high, low)
}

/// Lift a WireModel built in a local frame (a fixed f64 origin subtracted) back
/// to absolute world coordinates, re-splitting every position — polyline points
/// and SDF glyph vertices — into double-single so it stays precise at UTM scale.
/// The MLINE complex-linetype path uses this because `apply_along` lays out its
/// dashes and glyphs in f32, which would otherwise quantise fine spacing far
/// from the origin.
fn shift_wire_to_world(w: &mut WireModel, origin: [f64; 3]) {
    if !w.points.is_empty() {
        let mut hi = Vec::with_capacity(w.points.len());
        let mut lo = Vec::with_capacity(w.points.len());
        for p in &w.points {
            if p[0].is_nan() {
                hi.push([f32::NAN; 3]);
                lo.push([0.0; 3]);
                continue;
            }
            let (h, l) = split_ds_xyz(
                p[0] as f64 + origin[0],
                p[1] as f64 + origin[1],
                p[2] as f64 + origin[2],
            );
            hi.push(h);
            lo.push(l);
        }
        w.points = hi;
        w.points_low = lo;
    }
    for tv in &mut w.text_verts {
        let (h, l) = split_ds_xyz(
            tv.pos[0] as f64 + tv.pos_low[0] as f64 + origin[0],
            tv.pos[1] as f64 + tv.pos_low[1] as f64 + origin[1],
            tv.pos[2] as f64 + tv.pos_low[2] as f64 + origin[2],
        );
        tv.pos = h;
        tv.pos_low = l;
    }
    if w.aabb != WireModel::UNBOUNDED_AABB {
        w.aabb = [
            w.aabb[0] + origin[0] as f32,
            w.aabb[1] + origin[1] as f32,
            w.aabb[2] + origin[0] as f32,
            w.aabb[3] + origin[1] as f32,
        ];
    }
}

// ── Public entry points ────────────────────────────────────────────────────

/// Tessellate one entity into a WireModel.
/// For Text/MText entities this produces one WireModel with all glyph strokes
/// encoded as NaN-separated segments (wire_gpu skips NaN pairs).
/// For Solid3D entities this returns an empty wire; mesh tessellation lives
/// in `solid3d_tess` and is uploaded via the mesh pipeline instead.
pub fn tessellate(
    document: &CadDocument,
    handle: Handle,
    entity: &EntityType,
    selected: bool,
    entity_color: [f32; 4],
    pattern_length: f32,
    pattern: [f32; 8],
    line_weight_px: f32,
    anno_scale: f32,
    world_per_pixel: Option<f32>,
    // Canvas background colour — used for the MTEXT background *mask* fill
    // (flag 0x02, "use drawing window colour") so the mask erases geometry
    // behind the text the way a wipeout does.
    bg_color: [f32; 4],
    // When true, TEXT/MTEXT run-groups ALSO emit their glyph outline strokes as
    // polyline points (not just SDF quads). The in-app MTEXT editor preview
    // draws those strokes on a 2D canvas that can't run the SDF shader; every
    // other caller passes false and gets the normal SDF-only text.
    force_text_strokes: bool,
) -> Vec<WireModel> {
    let color = if selected {
        WireModel::SELECTED
    } else {
        entity_color
    };
    let name = handle.value().to_string();

    // Determine the effective annotation scale for this entity.
    //
    // Only annotative entities are auto-scaled by the current annotation scale;
    // everything else is manually pre-scaled (old convention with $DIMSCALE and
    // oversized text). Annotative-ness is resolved centrally from the entity's
    // per-object context, legacy XDATA, or annotative style (see
    // `scene::annotative::is_annotative`) so the bake and the panel agree.
    let anno_scale = if crate::scene::annotative::is_annotative(document, entity) {
        anno_scale
    } else {
        1.0
    };

    // A HATCH is drawn as a fill by the hatch pipeline and highlighted via a
    // fill tint when selected (issue #71), so it carries no boundary outline in
    // the wire set. Skipping it here drops the dense boundary polyline — the
    // dominant wire-instance cost on hatch-heavy drawings (issue #131). Picking
    // is unaffected: hatches are caught by their fill area through the existing
    // `pick::hit_test::click_hit_hatch` path, not this outline.
    if matches!(entity, EntityType::Hatch(_)) {
        return vec![];
    }

    // MultiLeader is handled by scene/mod.rs since it emits multiple WireModels
    // (leader, text, frame, fill) with distinct colors.
    if let EntityType::Leader(leader) = entity {
        return vec![leader.tessellate(
            document,
            handle,
            selected,
            entity_color,
            line_weight_px,
            anno_scale,
        )];
    }

    // MLINE emits one WireModel per style element so each parallel line keeps
    // its own colour and linetype — a red Continuous line under a yellow dashed
    // line reads as the two-tone multiline the style defines. Handled here, like
    // Leader, because the single-colour truck `Lines` path can't carry
    // per-element colour.
    if let EntityType::MLine(m) = entity {
        let lines = crate::entities::mline::mline_lines(m, document);
        if lines.is_empty() {
            return vec![];
        }
        let lt_scale =
            document.header.linetype_scale as f32 * m.common.linetype_scale as f32;
        let snap_pts: Vec<(glam::DVec3, SnapHint)> = m
            .vertices
            .iter()
            .map(|v| {
                (
                    glam::DVec3::new(v.position.x, v.position.y, v.position.z),
                    SnapHint::Node,
                )
            })
            .collect();
        let key_vertices: Vec<[f64; 3]> = m
            .vertices
            .iter()
            .map(|v| [v.position.x, v.position.y, v.position.z])
            .collect();

        // Local-frame origin (mline start) for the CPU-dashed / glyph-laid
        // elements. `apply_along` walks positions in f32, which quantises fine
        // spacing — dash gaps AND inter-glyph advance — at UTM coordinates
        // (the low half of the double-single is dropped). Subtracting this f64
        // origin first keeps the walk near zero and precise; the result is
        // shifted back to absolute double-single afterwards. Mirrors the
        // Tolerance frame, which also builds geometry locally and applies its
        // f64 origin later.
        let origin = [
            m.vertices[0].position.x,
            m.vertices[0].position.y,
            m.vertices[0].position.z,
        ];
        // Centre-line (vertex path) length — the shared "A"-type reference so
        // every parallel element uses the same end-dash length and thus the same
        // interior phase (perpendicular dashes line up). f64 deltas so it stays
        // precise at UTM coordinates.
        let ref_total: f32 = {
            let mut acc = 0.0_f64;
            for w in m.vertices.windows(2) {
                let dx = w[1].position.x - w[0].position.x;
                let dy = w[1].position.y - w[0].position.y;
                let dz = w[1].position.z - w[0].position.z;
                acc += (dx * dx + dy * dy + dz * dz).sqrt();
            }
            acc as f32
        };
        let mut out: Vec<WireModel> = Vec::with_capacity(lines.len());
        let mut snap_attached = false;
        for l in lines {
            if l.points.is_empty() {
                continue;
            }
            // Element colour: ByLayer / ByBlock inherit the entity's resolved
            // colour; an explicit ACI / true-colour is used as-is.
            let wcolor = if selected {
                WireModel::SELECTED
            } else {
                match l.color {
                    AcadColor::ByLayer | AcadColor::ByBlock => entity_color,
                    other => {
                        let [r, g, b, _] =
                            crate::scene::convert::tess_util::aci_to_rgba(&other);
                        [r, g, b, entity_color[3]]
                    }
                }
            };
            let aci = match l.color {
                AcadColor::Index(i) => i,
                _ => 0,
            };

            // Every dashed element is CPU-dashed by `apply_along` (not the GPU
            // pattern) so all parallel lines walk the polyline with the *same*
            // arithmetic and stay in phase — otherwise a shader-dashed line and an
            // apply_along-dashed sibling drift apart at large (UTM) coordinates and
            // one line's dash lands in the other's gap, striking through embedded
            // text. Document definition wins over the bundled catalog; a
            // continuous element yields `None` and falls to the solid path below.
            // Selection forces a plain solid highlight, so skip dashing then.
            let clt = if selected {
                None
            } else if let Some(doc_seg) =
                crate::io::linetypes::document_lt_segments(document, &l.linetype)
            {
                // In-document linetype: CPU-expand (apply_along) only when it
                // embeds TEXT glyphs that must be laid out along the curve. Pure
                // dash / space / dot (and undrawn shape) elements fall through to
                // the GPU dash shader below — cheaper (one WireModel + pattern
                // instead of N CPU segments) and now UTM-precise, so they stay in
                // phase with any glyph-bearing sibling element.
                if doc_seg
                    .segments
                    .iter()
                    .any(|s| matches!(s, crate::io::linetypes::LtSegment::Text { .. }))
                {
                    Some(doc_seg)
                } else {
                    None
                }
            } else {
                // Not in the document: bundled-catalog linetype (may embed
                // text / shape) → keep the CPU path; `resolve_pattern` below can't
                // see it, so GPU-dashing would drop the pattern to solid.
                crate::io::linetypes::complex_lt(&l.linetype).cloned()
            };

            let mut elem_wires: Vec<WireModel> = if let Some(clt) = clt {
                // Walk the dash / glyph layout in a local frame so the f32 math
                // stays precise, then lift each wire back to world DS.
                let local: Vec<[f32; 3]> = l
                    .points
                    .iter()
                    .map(|p| {
                        [
                            (p[0] - origin[0]) as f32,
                            (p[1] - origin[1]) as f32,
                            (p[2] - origin[2]) as f32,
                        ]
                    })
                    .collect();
                let mut w = crate::scene::text::complex_lt::apply_along(
                    &name,
                    &local,
                    &clt,
                    lt_scale.max(1e-4),
                    wcolor,
                    selected,
                    line_weight_px,
                    // Shared "A"-type reference: this text-bearing element aligns
                    // with the GPU-dashed sibling elements (same centre-line
                    // reference) instead of tiling independently from the start.
                    Some(ref_total),
                );
                for wm in &mut w {
                    wm.aci = aci;
                    shift_wire_to_world(wm, origin);
                }
                w
            } else {
                Vec::new()
            };

            // Simple path: the linetype isn't complex, or `apply_along` bailed
            // (pattern blow-up guard) and returned nothing — draw the element as a
            // dashed / solid polyline so it is never lost.
            if elem_wires.is_empty() {
                let (pts, pts_low) = points_to_ds(l.points);
                let (pattern_length, pattern) = if selected {
                    (0.0, [0.0; 8])
                } else {
                    crate::scene::view::render::resolve_pattern(
                        &document.line_types,
                        &l.linetype,
                        lt_scale,
                    )
                };
                // Shared "A"-type: derive the begin/end solid-dash length ONCE
                // from the multiline centre-line (`ref_total`) so every parallel
                // element runs the same interior phase and its dashes line up
                // perpendicular; `align_total` stays each element's own length in
                // the shader so each still ends on a dash. Dash-first patterns
                // only (`+dash, -gap, …`); shorter-than-a-period lines fall back
                // to the per-wire path (solid).
                let dash_align_end = if pattern_length > 1e-6
                    && pattern[0] > 0.0
                    && pattern[1] < 0.0
                    && ref_total > pattern_length
                {
                    let a = pattern[0];
                    let p = pattern_length;
                    let k = ((ref_total - a) / p).round().max(1.0);
                    Some(((ref_total - k * p + a) * 0.5).max(1e-4))
                } else {
                    None
                };
                elem_wires.push(WireModel {
                    taper_widths: Vec::new(),
                    world_width: 0.0,
                    fill_is_3d: false,
                    pick_tris: Vec::new(),
                    pick_tris_low: Vec::new(),
                    // MLINE dashes: A-type aligned, but the end-dash length is
                    // shared across all parallel elements (`dash_align_end`) so
                    // their interiors stay in phase (perpendicular dashes line up)
                    // while each still ends on a dash at its own endpoint.
                    dash_from_start: false,
                    dash_align_end,
                    text_verts: Vec::new(),
                    name: name.clone(),
                    points: pts,
                    points_low: pts_low,
                    color: wcolor,
                    selected,
                    pattern_length,
                    pattern,
                    line_weight_px,
                    snap_pts: Vec::new(),
                    tangent_geoms: Vec::new(),
                    aci,
                    key_vertices: Vec::new(),
                    aabb: WireModel::UNBOUNDED_AABB,
                    plinegen: false,
                    fill_tris: Vec::new(),
                    fill_tris_low: Vec::new(),
                });
            }

            // Snap / key vertices ride the first emitted wire only (they describe
            // the whole entity, not one element).
            if !snap_attached {
                if let Some(w0) = elem_wires.first_mut() {
                    w0.snap_pts = snap_pts.clone();
                    w0.key_vertices = key_vertices.clone();
                    snap_attached = true;
                }
            }
            out.append(&mut elem_wires);
        }
        if out.is_empty() {
            return vec![];
        }
        return out;
    }

    // ── Try the truck path first ───────────────────────────────────────────
    // Relative-PDSIZE points size their glyph from the current zoom so they
    // stay a roughly constant on-screen size; otherwise the header-driven path.
    let te = crate::entities::point::relative_truck(entity, document, world_per_pixel)
        .or_else(|| crate::entities::light::relative_truck(entity, document, world_per_pixel))
        .or_else(|| convert(entity, document));
    if let Some(te) = te {
        match te.object {
            // ── Text / MText: pre-tessellated glyph strokes ───────────────
            //
            // Strokes are pre-grouped by world origin (one TextStroke per
            // line / per run / per fragment), each carrying an optional
            // colour override produced by MTEXT inline `\C` / `\c`. We bin
            // groups by override colour and emit one WireModel per bin so a
            // single MTEXT can hand back N colour-distinct wires when the
            // value mixes inline colours.
            TruckObject::Text(stroke_groups) => {
                let entity_zf = entity_z(entity) as f64;
                let elev_v = entity_zf;

                // anno_scale anchors at the first group's origin so multi-line
                // MText lines spread apart correctly as they grow.
                let ref_origin = stroke_groups
                    .first()
                    .map(|g| g.origin)
                    .unwrap_or([0.0, 0.0]);
                let ref_lx_v = ref_origin[0];
                let ref_ly_v = ref_origin[1];

                // Selection forces a single uniform colour — never split.
                let split_by_color = !selected;

                // Bins: key = (Some(rgb), bold). Bold strokes bin separately so
                // the editor preview can draw them with a wider pen.
                struct TextBin {
                    color: Option<[f32; 3]>,
                    bold: bool,
                    pts: Vec<[f32; 3]>,
                    pts_low: Vec<[f32; 3]>,
                    fill_tris: Vec<[f32; 3]>,
                    fill_tris_low: Vec<[f32; 3]>,
                }
                let mut bins: Vec<TextBin> = Vec::new();
                let mut bin_first: Vec<bool> = Vec::new();
                let find_or_make = |key: Option<[f32; 3]>,
                                    bold: bool,
                                    bins: &mut Vec<TextBin>,
                                    firsts: &mut Vec<bool>|
                 -> usize {
                    if let Some(i) = bins.iter().position(|b| b.color == key && b.bold == bold) {
                        i
                    } else {
                        bins.push(TextBin {
                            color: key,
                            bold,
                            pts: Vec::new(),
                            pts_low: Vec::new(),
                            fill_tris: Vec::new(),
                            fill_tris_low: Vec::new(),
                        });
                        firsts.push(true);
                        bins.len() - 1
                    }
                };

                let anno = anno_scale as f64;
                // SDF text is decided PER GROUP by whether the group carries a
                // `GlyphRun`: a run-group renders as textured quads (built
                // below) so its strokes are suppressed; a run-less group keeps
                // its strokes. TEXT / MTEXT / dim / block / mleader text are all
                // run-groups → fully SDF; a composite object that packs geometry
                // and text into one Text object (a tolerance frame: box lines =
                // run-less, cell text = run-groups) keeps the geometry as
                // strokes and draws only the text as SDF.
                for group in stroke_groups
                    .iter()
                    .filter(|g| force_text_strokes || g.run.is_none())
                {
                    let lx_v = group.origin[0];
                    let ly_v = group.origin[1];
                    let slx_v = (lx_v - ref_lx_v) * anno + ref_lx_v;
                    let sly_v = (ly_v - ref_ly_v) * anno + ref_ly_v;
                    let bin_key = if split_by_color { group.color } else { None };
                    let group_bold = group.run.as_ref().is_some_and(|r| r.bold);
                    let bi = find_or_make(bin_key, group_bold, &mut bins, &mut bin_first);
                    
                    // 1. Process outline strokes
                    for stroke in &group.strokes {
                        if stroke.len() < 2 {
                            continue;
                        }
                        if !bin_first[bi] && !bins[bi].pts.is_empty() {
                            bins[bi].pts.push([f32::NAN, f32::NAN, f32::NAN]);
                            bins[bi].pts_low.push([0.0; 3]);
                        }
                        bin_first[bi] = false;
                        for &[x, y] in stroke {
                            let xv = x as f64 * anno + slx_v;
                            let yv = y as f64 * anno + sly_v;
                            let (h, l) = split_ds_xyz(xv, yv, elev_v);
                            bins[bi].pts.push(h);
                            bins[bi].pts_low.push(l);
                        }
                    }

                    // 2. Process fill triangles
                    for &[x, y] in &group.fill_tris {
                        let xv = x as f64 * anno + slx_v;
                        let yv = y as f64 * anno + sly_v;
                        let (h, l) = split_ds_xyz(xv, yv, elev_v);
                        bins[bi].fill_tris.push(h);
                        bins[bi].fill_tris_low.push(l);
                    }
                }

                // ── SDF glyph quads ──────────────────────────────────────
                // Build each run's glyph quads here and carry them on the wire.
                // They ride with the wire through the tess memo, hit-
                // materialisation and (for block content) the block-expand
                // transform — no separate document-wide collector.
                let mut sdf_verts: Vec<crate::scene::pipeline::text_gpu::TextVertex> = Vec::new();
                {
                    if let Ok(mut atlas) = crate::scene::text::sdf_atlas::text_atlas().lock() {
                        // Selection tints the whole run; otherwise inline `\C`
                        // colours (bin key) win, falling back to entity colour.
                        for group in &stroke_groups {
                            let Some(run) = &group.run else { continue };
                            let slx_v = (group.origin[0] - ref_lx_v) * anno + ref_lx_v;
                            let sly_v = (group.origin[1] - ref_ly_v) * anno + ref_ly_v;
                            // Base colour only (inline `\C` wins). Selection /
                            // hover recolouring is done by the text-highlight
                            // overlay, so the base glyphs stay neutral — else a
                            // deselect would leave stale-tinted glyphs until the
                            // next geometry rebuild (the base text buffer only
                            // rebuilds on geometry, not on a pick).
                            let gcolor = group
                                .color
                                .map(|c| [c[0], c[1], c[2], entity_color[3]])
                                .unwrap_or(entity_color);
                            let quads = crate::scene::text::glyph_quads::layout_glyph_quads(
                                &mut atlas,
                                run.height,
                                run.rotation,
                                run.width_factor,
                                run.oblique,
                                run.tracking,
                                &run.font,
                                run.bold,
                                &run.text,
                            );
                            crate::scene::pipeline::text_gpu::push_glyph_vertices(
                                &mut sdf_verts,
                                &quads,
                                [slx_v, sly_v, elev_v],
                                anno,
                                gcolor,
                                0.0,
                            );
                        }
                    }
                }

                let snap_pts = te.snap_pts;
                let key_vertices: Vec<[f64; 3]> = te
                    .key_vertices
                    .into_iter()
                    .map(|[x, y, z]| [x, y, z])
                    .collect();

                // Pick box straight from the rendered glyph quads — the true
                // text extent. entity_aabb is unreliable for MTEXT (its box sits
                // beside the laid-out glyphs), so derive the AABB from
                // `sdf_verts` (accumulate in f64, reconstruct high+low, then cast
                // to f32 once) and stop the generic stamp clobbering it (tess.rs
                // guards on `text_verts`). Computed here so both the bins-empty
                // and the bins-non-empty (tolerance box + text) paths can stamp
                // it on the SDF text wire.
                let text_aabb = if !sdf_verts.is_empty() {
                    let (mut nx, mut ny, mut xx, mut xy) =
                        (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
                    for v in &sdf_verts {
                        let x = v.pos[0] as f64 + v.pos_low[0] as f64;
                        let y = v.pos[1] as f64 + v.pos_low[1] as f64;
                        nx = nx.min(x);
                        xx = xx.max(x);
                        ny = ny.min(y);
                        xy = xy.max(y);
                    }
                    [nx as f32, ny as f32, xx as f32, xy as f32]
                } else {
                    WireModel::UNBOUNDED_AABB
                };

                // Empty input (no glyphs) → emit a single empty wire so the
                // entity still has a hit-test target via snap_pts. With SDF on
                // this is the normal path (strokes suppressed) and the wire
                // also carries the glyph quads built above.
                if bins.is_empty() {
                    let mut wires: Vec<WireModel> = Vec::new();
                    // MTEXT background fill / mask: an opaque rectangle behind
                    // the glyphs, emitted first so it renders under the text.
                    // Flag 0x01 → the entity's background-fill colour; 0x02 →
                    // the drawing-window (canvas) colour, which masks geometry
                    // behind the text like a wipeout. Box = glyph bounds padded
                    // by (background_scale - 1) × text height.
                    if text_aabb != WireModel::UNBOUNDED_AABB {
                        if let EntityType::MText(m) = entity {
                            let has_fill = m.background_fill_flags & 0x03 != 0;
                            let has_frame = m.background_fill_flags & 0x10 != 0;
                            if has_fill || has_frame {
                                // Padded box shared by the fill and the frame.
                                let th = (m.height * anno) as f32;
                                let pad = ((m.background_scale as f32) - 1.0).max(0.0) * th;
                                let [bnx, bny, bxx, bxy] = text_aabb;
                                let (l, b, r, t) = (
                                    (bnx - pad) as f64,
                                    (bny - pad) as f64,
                                    (bxx + pad) as f64,
                                    (bxy + pad) as f64,
                                );
                                // Fill / mask — two triangles behind the glyphs.
                                if has_fill {
                                    // 0x02 (use the drawing-window colour) is a
                                    // MASK — it wins when set, even alongside
                                    // 0x01, so text flagged "drawing background"
                                    // erases what's behind it (a dark box on a
                                    // dark canvas = no visible colour), instead
                                    // of painting the stored background_color. A
                                    // plain 0x01 fill paints that colour.
                                    let fill_color = if m.background_fill_flags & 0x02 != 0 {
                                        bg_color
                                    } else {
                                        color_or_inherit(&m.background_color, bg_color)
                                    };
                                    let corners = [[l, b], [r, b], [r, t], [l, t]];
                                    let mut ft = Vec::with_capacity(6);
                                    let mut ftl = Vec::with_capacity(6);
                                    for &k in &[0usize, 1, 2, 0, 2, 3] {
                                        let (h, lo) =
                                            split_ds_xyz(corners[k][0], corners[k][1], elev_v);
                                        ft.push(h);
                                        ftl.push(lo);
                                    }
                                    wires.push(WireModel {
                                        taper_widths: Vec::new(),
                                        world_width: 0.0,
                                        fill_is_3d: false,
                                        pick_tris: Vec::new(),
                                        pick_tris_low: Vec::new(),
                                        dash_from_start: false,
                                        dash_align_end: None,
                                        text_verts: Vec::new(),
                                        name: name.clone(),
                                        points: vec![],
                                        points_low: Vec::new(),
                                        color: fill_color,
                                        selected,
                                        aci: 0,
                                        pattern_length: 0.0,
                                        pattern: [0.0; 8],
                                        line_weight_px,
                                        snap_pts: vec![],
                                        tangent_geoms: vec![],
                                        key_vertices: vec![],
                                        aabb: WireModel::UNBOUNDED_AABB,
                                        plinegen: true,
                                        fill_tris: ft,
                                        fill_tris_low: ftl,
                                    });
                                }
                                // Text frame — a closed rectangle in the text
                                // colour around the same box.
                                if has_frame {
                                    let loop_xy =
                                        [[l, b], [r, b], [r, t], [l, t], [l, b]];
                                    let mut fp = Vec::with_capacity(5);
                                    let mut fpl = Vec::with_capacity(5);
                                    for &[x, y] in &loop_xy {
                                        let (h, lo) = split_ds_xyz(x, y, elev_v);
                                        fp.push(h);
                                        fpl.push(lo);
                                    }
                                    wires.push(WireModel {
                                        taper_widths: Vec::new(),
                                        world_width: 0.0,
                                        fill_is_3d: false,
                                        pick_tris: Vec::new(),
                                        pick_tris_low: Vec::new(),
                                        dash_from_start: false,
                                        dash_align_end: None,
                                        text_verts: Vec::new(),
                                        name: name.clone(),
                                        points: fp,
                                        points_low: fpl,
                                        color: entity_color,
                                        selected,
                                        aci: 0,
                                        pattern_length: 0.0,
                                        pattern: [0.0; 8],
                                        line_weight_px,
                                        snap_pts: vec![],
                                        tangent_geoms: vec![],
                                        key_vertices: vec![],
                                        aabb: WireModel::UNBOUNDED_AABB,
                                        plinegen: true,
                                        fill_tris: vec![],
                                        fill_tris_low: Vec::new(),
                                    });
                                }
                            }
                        }
                    }
                    // Debug (env OCS_TEXT_BOX): draw a rectangle around the glyph
                    // bounds as a separate outline wire so the text box is
                    // visible for testing. The empty text wire below is left
                    // untouched (still the SDF + pick target).
                    if !sdf_verts.is_empty()
                        && crate::scene::text::sdf_atlas::text_box_debug()
                    {
                        let [nx, ny, xx, xy] = text_aabb;
                        let (nx, ny, xx, xy) = (nx as f64, ny as f64, xx as f64, xy as f64);
                        let mut pts = Vec::with_capacity(5);
                        let mut low = Vec::with_capacity(5);
                        for (x, y) in [(nx, ny), (xx, ny), (xx, xy), (nx, xy), (nx, ny)] {
                            let (hh, ll) = split_ds_xyz(x, y, elev_v);
                            pts.push(hh);
                            low.push(ll);
                        }
                        wires.push(WireModel {
                            taper_widths: Vec::new(),
                            world_width: 0.0,
                            fill_is_3d: false,
                            pick_tris: Vec::new(),
                            pick_tris_low: Vec::new(),
                            dash_from_start: false,
                            dash_align_end: None,
                            text_verts: Vec::new(),
                            name: name.clone(),
                            points: pts,
                            points_low: low,
                            color: [1.0, 0.0, 1.0, 1.0],
                            selected,
                            pattern_length: 0.0,
                            pattern: [0.0; 8],
                            line_weight_px,
                            snap_pts: Vec::new(),
                            tangent_geoms: Vec::new(),
                            aci: 0,
                            key_vertices: Vec::new(),
                            aabb: WireModel::UNBOUNDED_AABB,
                            plinegen: true,
                            fill_tris: vec![],
                            fill_tris_low: Vec::new(),
                        });
                    }
                    wires.push(WireModel {
                        taper_widths: Vec::new(),
                        world_width: 0.0,
                        fill_is_3d: false,
                        pick_tris: Vec::new(),
                        pick_tris_low: Vec::new(),
                        dash_from_start: false,
                        dash_align_end: None,
                        text_verts: sdf_verts,
                        name,
                        points: Vec::new(),
                        points_low: Vec::new(),
                        color,
                        selected,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts,
                        tangent_geoms: te.tangent_geoms,
                        aci: 0,
                        key_vertices,
                        aabb: text_aabb,
                        plinegen: true,
                        fill_tris: vec![],
                        fill_tris_low: Vec::new(),
                    });
                    return wires;
                }

                let mut out: Vec<WireModel> = Vec::new();
                let mut is_first = true;
                for bin in bins {
                    let wire_color = match bin.color {
                        Some([r, g, b]) => [r, g, b, color[3]],
                        None => color,
                    };
                    // Bold text strokes carry a wider pen so the editor preview
                    // draws them thicker (these stroke wires exist only when
                    // strokes are forced, i.e. the preview; the main render uses
                    // SDF where bold is a wider baked pen).
                    let bin_lw = if bin.bold {
                        line_weight_px.max(1.0) * 2.4
                    } else {
                        line_weight_px
                    };

                    if !bin.pts.is_empty() {
                        let (snap, keys, tangents) = if is_first {
                            is_first = false;
                            (
                                snap_pts.clone(),
                                key_vertices.clone(),
                                te.tangent_geoms.clone(),
                            )
                        } else {
                            (Vec::new(), Vec::new(), Vec::new())
                        };
                        out.push(WireModel {
                            taper_widths: Vec::new(),
                            world_width: 0.0,
                            fill_is_3d: false,
                            pick_tris: Vec::new(),
                            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                            name: name.clone(),
                            points: bin.pts,
                            points_low: bin.pts_low,
                            color: wire_color,
                            selected,
                            pattern_length: 0.0,
                            pattern: [0.0; 8],
                            line_weight_px: bin_lw,
                            snap_pts: snap,
                            tangent_geoms: tangents,
                            aci: 0,
                            key_vertices: keys,
                            aabb: WireModel::UNBOUNDED_AABB,
                            plinegen: true,
                            fill_tris: vec![],
                            fill_tris_low: Vec::new(),
                        });
                    }

                    if !bin.fill_tris.is_empty() {
                        let (snap, keys, tangents) = if is_first {
                            is_first = false;
                            (
                                snap_pts.clone(),
                                key_vertices.clone(),
                                te.tangent_geoms.clone(),
                            )
                        } else {
                            (Vec::new(), Vec::new(), Vec::new())
                        };
                        out.push(WireModel {
                            taper_widths: Vec::new(),
                            world_width: 0.0,
                            fill_is_3d: false,
                            pick_tris: Vec::new(),
                            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                            name: name.clone(),
                            points: Vec::new(),
                            points_low: Vec::new(),
                            color: wire_color,
                            selected,
                            pattern_length: 0.0,
                            pattern: [0.0; 8],
                            line_weight_px,
                            snap_pts: snap,
                            tangent_geoms: tangents,
                            aci: 0,
                            key_vertices: keys,
                            aabb: WireModel::UNBOUNDED_AABB,
                            plinegen: true,
                            fill_tris: bin.fill_tris,
                            fill_tris_low: bin.fill_tris_low,
                        });
                    }
                }

                // Composite Text objects (e.g. a tolerance frame) keep geometry
                // in `bins` (run-less groups) and text in `sdf_verts` (run
                // groups). Emit the glyphs on their own wire carrying the tight
                // glyph-box AABB so the text draws + picks alongside the box
                // strokes. TEXT / MTEXT never reach here with SDF on (their bins
                // are empty → the early-return path above).
                if !sdf_verts.is_empty() {
                    out.push(WireModel {
                        taper_widths: Vec::new(),
                        world_width: 0.0,
                        fill_is_3d: false,
                        pick_tris: Vec::new(),
                        pick_tris_low: Vec::new(),
                        dash_from_start: false,
                        dash_align_end: None,
                        text_verts: sdf_verts,
                        name: name.clone(),
                        points: Vec::new(),
                        points_low: Vec::new(),
                        color,
                        selected,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts: Vec::new(),
                        tangent_geoms: Vec::new(),
                        aci: 0,
                        key_vertices: Vec::new(),
                        aabb: text_aabb,
                        plinegen: true,
                        fill_tris: vec![],
                        fill_tris_low: Vec::new(),
                    });
                }

                if out.is_empty() {
                    out.push(WireModel {
                        taper_widths: Vec::new(),
                        world_width: 0.0,
                        fill_is_3d: false,
                        pick_tris: Vec::new(),
                        pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                        name,
                        points: Vec::new(),
                        points_low: Vec::new(),
                        color,
                        selected,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts,
                        tangent_geoms: te.tangent_geoms,
                        aci: 0,
                        key_vertices,
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        fill_tris: vec![],
                        fill_tris_low: Vec::new(),
                    });
                }
                return out;
            }

            // ── Standard topology objects ─────────────────────────────────
            TruckObject::Point(v) => {
                let result = tessellate_vertex(&v);
                match result {
                    TruckTessResult::Point([x, y, z], [xl, yl, zl]) => {
                        // A PDMODE=0 point is a single dot. Size its marker to
                        // ~1 px so it reads as a dot rather than a large
                        // world-space "+" in small drawings — otherwise the
                        // dimension def-points on the Defpoints layer litter
                        // the view with crosses. The tessellation cache keys on
                        // world-per-pixel, so this re-sizes on zoom and stays a
                        // constant on-screen size. (#139)
                        let s = world_per_pixel
                            .map(|w| (w * 0.75).max(1e-6))
                            .unwrap_or(0.1);
                        let snap_pts = te.snap_pts;
                        let key_vertices: Vec<[f64; 3]> = te
                            .key_vertices
                            .into_iter()
                            .map(|[kx, ky, kz]| [kx, ky, kz])
                            .collect();
                        return vec![WireModel {
                            taper_widths: Vec::new(),
                            world_width: 0.0,
                            fill_is_3d: false,
                            pick_tris: Vec::new(),
                            pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                            name,
                            points: vec![
                                [x - s, y, z],
                                [x + s, y, z],
                                [x, y - s, z],
                                [x, y + s, z],
                            ],
                            // All four cross points share the Point's residual
                            // (the cross arms are tiny, < 0.1 m, so the low
                            // component of the centre is also the right one
                            // for the arm tips at f32 precision).
                            points_low: vec![[xl, yl, zl]; 4],
                            color,
                            selected,
                            pattern_length: 0.0,
                            pattern: [0.0; 8],
                            line_weight_px: 1.0,
                            snap_pts,
                            tangent_geoms: te.tangent_geoms,
                            aci: 0,
                            key_vertices,
                            aabb: WireModel::UNBOUNDED_AABB,
                            plinegen: true,
                            fill_tris: vec![],
                            fill_tris_low: Vec::new(),
                        }];
                    }
                    _ => {}
                }
            }

            TruckObject::Curve(e) => {
                if let TruckTessResult::Lines(points, points_low) =
                    tessellate_edge(&e)
                {
                    let snap_pts = te.snap_pts;
                    let key_vertices: Vec<[f64; 3]> = te
                        .key_vertices
                        .into_iter()
                        .map(|[x, y, z]| [x, y, z])
                        .collect();
                    return vec![WireModel {
                        taper_widths: Vec::new(),
                        world_width: 0.0,
                        fill_is_3d: false,
                        pick_tris: Vec::new(),
                        pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                        name,
                        points,
                        points_low,
                        color,
                        selected,
                        pattern_length,
                        pattern,
                        line_weight_px,
                        snap_pts,
                        tangent_geoms: te.tangent_geoms,
                        aci: 0,
                        key_vertices,
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        fill_tris: vec![],
                        fill_tris_low: Vec::new(),
                    }];
                }
            }

            TruckObject::Contour(w) => {
                if let TruckTessResult::Lines(points, points_low) =
                    tessellate_wire(&w)
                {
                    let snap_pts = te.snap_pts;
                    let key_vertices: Vec<[f64; 3]> = te
                        .key_vertices
                        .into_iter()
                        .map(|[x, y, z]| [x, y, z])
                        .collect();
                    // A wide polyline arrives here: the shader expands this
                    // centre-line to `world_width` so the band IS the wire (the
                    // linetype dashes it); the band also backs pick.
                    let (pick_tris, pick_tris_low) = points_to_ds(te.pick_tris);
                    return vec![WireModel {
                        taper_widths: Vec::new(),
                        world_width: polyline_band_width(entity),
                        fill_is_3d: false,
                        pick_tris,
                        pick_tris_low,
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                        name,
                        points,
                        points_low,
                        color,
                        selected,
                        pattern_length,
                        pattern,
                        line_weight_px,
                        snap_pts,
                        tangent_geoms: te.tangent_geoms,
                        aci: 0,
                        key_vertices,
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        fill_tris: vec![],
                        fill_tris_low: Vec::new(),
                    }];
                }
            }

            TruckObject::Lines(points) => {
                // Points are world-space f64 from entity converters (polyline,
                // leader, mesh, solid2d, etc.). Subtract world_offset in f64
                // and split into double-single (high, low) f32 buffers — the
                // GPU shader pairs them so drawings at large UTM-style
                // coordinates keep sub-unit precision in the wire model and
                // don't jitter on camera movement.
                let (local_pts, local_pts_low) = points_to_ds(points);
                let snap_pts = te.snap_pts;
                let key_vertices: Vec<[f64; 3]> = te
                    .key_vertices
                    .into_iter()
                    .map(|[x, y, z]| [x, y, z])
                    .collect();
                let (fill_tris, fill_tris_low) = points_to_ds(te.fill_tris);
                // Only a real 3-D mesh surface (PolyfaceMesh / PolygonMesh /
                // the modern subdivision Mesh) fill renders shaded-only with
                // hidden-surface depth; every other `fill_tris` here (a SOLID's
                // filled quad — e.g. a `_BoxFilled` arrowhead) is a flat 2-D
                // overlay visible in every view mode. Leaving `Mesh` out drops
                // its face fill into the 2-D buffer, so it drew in wireframe too.
                let fill_is_3d = matches!(
                    entity,
                    EntityType::PolyfaceMesh(_)
                        | EntityType::PolygonMesh(_)
                        | EntityType::Mesh(_)
                );
                // Thickness walls ride on the wire that carries their edges, not
                // on a wire of their own: they are pick geometry for that entity,
                // and `fill_tris` below deliberately splits off into a fill-only
                // wire (`is_fill_only`) which has no `points` to hang them from.
                let (pick_tris, pick_tris_low) = points_to_ds(te.pick_tris);
                let mut out = Vec::new();
                let mut is_first = true;

                // A thickened polyline's extrusion — its corner / cap edges
                // frame the solid tube and read black (like solid-with-edges
                // outlines), while the tube fill keeps the entity colour. Both
                // wide polyline kinds (LwPolyline + Polyline2D) extrude tubes.
                let is_thick_extrusion = matches!(
                    entity,
                    EntityType::LwPolyline(p) if p.thickness.abs() > 1e-10
                ) || matches!(
                    entity,
                    EntityType::Polyline2D(p) if p.thickness.abs() > 1e-10
                );
                let edge_color = if is_thick_extrusion {
                    [0.0, 0.0, 0.0, 1.0]
                } else {
                    color
                };

                if !local_pts.is_empty() {
                    let (snap, keys, tangents) = if is_first {
                        is_first = false;
                        (
                            snap_pts.clone(),
                            key_vertices.clone(),
                            te.tangent_geoms.clone(),
                        )
                    } else {
                        (Vec::new(), Vec::new(), Vec::new())
                    };
                    out.push(WireModel {
                        taper_widths: Vec::new(),
                        world_width: 0.0,
                        fill_is_3d: false,
                        pick_tris,
                        pick_tris_low,
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                        name: name.clone(),
                        points: local_pts,
                        points_low: local_pts_low,
                        color: edge_color,
                        selected,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts: snap,
                        tangent_geoms: tangents,
                        aci: 0,
                        key_vertices: keys,
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        fill_tris: vec![],
                        fill_tris_low: Vec::new(),
                    });
                }

                if !fill_tris.is_empty() {
                    let (snap, keys, tangents) = if is_first {
                        (
                            snap_pts.clone(),
                            key_vertices.clone(),
                            te.tangent_geoms.clone(),
                        )
                    } else {
                        (Vec::new(), Vec::new(), Vec::new())
                    };
                    out.push(WireModel {
                        taper_widths: Vec::new(),
                        world_width: 0.0,
                        pick_tris: Vec::new(),
                        pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                        name: name.clone(),
                        points: Vec::new(),
                        points_low: Vec::new(),
                        color,
                        selected,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts: snap,
                        tangent_geoms: tangents,
                        aci: 0,
                        key_vertices: keys,
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        fill_tris,
                        fill_tris_low,
                        fill_is_3d,
                    });
                }

                if out.is_empty() {
                    out.push(WireModel {
                        taper_widths: Vec::new(),
                        world_width: 0.0,
                        fill_is_3d: false,
                        pick_tris: Vec::new(),
                        pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                        name,
                        points: Vec::new(),
                        points_low: Vec::new(),
                        color,
                        selected,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts,
                        tangent_geoms: te.tangent_geoms,
                        aci: 0,
                        key_vertices,
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        fill_tris: vec![],
                        fill_tris_low: Vec::new(),
                    });
                }

                return out;
            }

            TruckObject::SegmentedLines(points) => {
                let (local_pts, local_pts_low) = points_to_ds(points);
                let snap_pts = te.snap_pts;
                let key_vertices: Vec<[f64; 3]> = te
                    .key_vertices
                    .into_iter()
                    .map(|[x, y, z]| [x, y, z])
                    .collect();
                // A wide polyline with PLINEGEN=0 arrives here: same shader-band
                // treatment as the Contour arm, restarting the dash per segment.
                let (pick_tris, pick_tris_low) = points_to_ds(te.pick_tris);
                return vec![WireModel {
                    taper_widths: Vec::new(),
                    world_width: polyline_band_width(entity),
                    fill_is_3d: false,
                    pick_tris,
                    pick_tris_low,
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                    name,
                    points: local_pts,
                    points_low: local_pts_low,
                    color,
                    selected,
                    pattern_length,
                    pattern,
                    line_weight_px,
                    snap_pts,
                    tangent_geoms: te.tangent_geoms,
                    aci: 0,
                    key_vertices,
                    plinegen: false,
                    aabb: WireModel::UNBOUNDED_AABB,
                    fill_tris: vec![],
                    fill_tris_low: Vec::new(),
                }];
            }

            TruckObject::TaperedLines(points, widths) => {
                // A wide polyline whose width varies: one continuous band wire
                // carrying a per-point width; the shader interpolates each
                // segment's two endpoint widths. `world_width` (the widest edge)
                // stays as the constant fallback for PDF export + a hairline
                // floor when zoomed out.
                let (local_pts, local_pts_low) = points_to_ds(points);
                let snap_pts = te.snap_pts;
                let key_vertices: Vec<[f64; 3]> = te
                    .key_vertices
                    .into_iter()
                    .map(|[x, y, z]| [x, y, z])
                    .collect();
                let (pick_tris, pick_tris_low) = points_to_ds(te.pick_tris);
                let world_width = widths.iter().copied().fold(0.0f32, f32::max);
                return vec![WireModel {
                    taper_widths: widths,
                    world_width,
                    fill_is_3d: false,
                    pick_tris,
                    pick_tris_low,
                    dash_from_start: false,
                    dash_align_end: None,
                    text_verts: Vec::new(),
                    name,
                    points: local_pts,
                    points_low: local_pts_low,
                    color,
                    selected,
                    pattern_length,
                    pattern,
                    line_weight_px,
                    snap_pts,
                    tangent_geoms: te.tangent_geoms,
                    aci: 0,
                    key_vertices,
                    plinegen: true,
                    aabb: WireModel::UNBOUNDED_AABB,
                    fill_tris: vec![],
                    fill_tris_low: Vec::new(),
                }];
            }

            TruckObject::Volume(_) => {
                // Solid3D / Region / Body → mesh tessellation lives in
                // `solid3d_tess`. As a wire fallback, render the pre-computed
                // edge wires stored in the entity when present (e.g. from
                // SOLVIEW output or when the SAT kernel cannot parse the
                // ACIS data).
                let wire_pts = solid_wire_fallback(entity);
                let mut wm = WireModel::solid_f64(name, wire_pts, color, selected);
                // Add insertion snap at point_of_reference.
                if let Some(p) = crate::entities::solid3d::point_of_reference(entity) {
                    let sp = glam::DVec3::new(p.x, p.y, p.z);
                    wm.snap_pts.push((sp, SnapHint::Insertion));
                }
                return vec![wm];
            }
        }
    }

    // ── Fallback for Viewport / Insert / Hatch / Ole2Frame ────────────────
    let (points_f64, snap_pts, tangent_geoms, key_vertices) =
        fallback_geometry(entity);
    // `points_f64` are absolute world coords; split into the double-single
    // high/low pair so the outline reconstructs to f64 precision at UTM scale
    // (a NaN separator stays NaN in both buffers).
    let mut points: Vec<[f32; 3]> = Vec::with_capacity(points_f64.len());
    let mut points_low: Vec<[f32; 3]> = Vec::with_capacity(points_f64.len());
    for [x, y, z] in &points_f64 {
        if !x.is_finite() || !y.is_finite() {
            points.push([f32::NAN, f32::NAN, f32::NAN]);
            points_low.push([0.0; 3]);
            continue;
        }
        let (hx, lx) = split_ds(*x);
        let (hy, ly) = split_ds(*y);
        let (hz, lz) = split_ds(*z);
        points.push([hx, hy, hz]);
        points_low.push([lx, ly, lz]);
    }
    // fallback_geometry still emits offset-relative f32 snap points; widen to
    // f64 for the WireModel's double-single-era snap buffer.
    let snap_pts: Vec<(glam::DVec3, SnapHint)> =
        snap_pts.into_iter().map(|(p, h)| (p.as_dvec3(), h)).collect();
    vec![WireModel {
        taper_widths: Vec::new(),
        world_width: 0.0,
        fill_is_3d: false,
        pick_tris: Vec::new(),
        pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
        name,
        points,
        points_low,
        color,
        selected,
        aci: 0,
        pattern_length,
        pattern,
        line_weight_px,
        snap_pts,
        tangent_geoms,
        key_vertices,
        aabb: WireModel::UNBOUNDED_AABB,
        plinegen: true,
        fill_tris: vec![],
        fill_tris_low: Vec::new(),
    }]
}



#[derive(Clone, Copy)]
pub(crate) enum ArrowKind {
    None,
    Triangle { size: f32, filled: bool, size_mul: f32 },
    Tick { size: f32 },
    Open { size: f32, half_angle: f32 },
    Dot { size: f32, filled: bool },
    Origin { size: f32 },
    Box_ { size: f32, filled: bool },
    Datum { size: f32, filled: bool },
}

pub(crate) fn arrow_from_block(
    doc: &CadDocument,
    handle: acadrust::types::Handle,
    dimasz: f32,
) -> ArrowKind {
    let name = if handle.is_null() {
        None
    } else {
        doc.block_records
            .iter()
            .find(|b| b.handle == handle)
            .map(|b| b.name.as_str())
    };
    arrow_from_block_name(name, dimasz)
}

fn arrow_from_block_name(name: Option<&str>, dimasz: f32) -> ArrowKind {
    // AutoCAD's standard arrow blocks are prefixed with "_" (e.g. "_OPEN").
    // Strip the prefix, upper-case, and switch on canonical names. Unknown
    // / missing names default to ClosedFilled.
    let n = name
        .map(|s| s.trim().trim_start_matches('_').to_ascii_uppercase())
        .unwrap_or_default();
    match n.as_str() {
        "" | "CLOSEDFILLED" => ArrowKind::Triangle {
            size: dimasz,
            filled: true,
            size_mul: 1.0,
        },
        "CLOSED" | "CLOSEDBLANK" => ArrowKind::Triangle {
            size: dimasz,
            filled: false,
            size_mul: 1.0,
        },
        "SMALL" => ArrowKind::Triangle {
            size: dimasz,
            filled: true,
            size_mul: 0.5,
        },
        "OPEN" => ArrowKind::Open {
            size: dimasz,
            half_angle: 9.5_f32.to_radians(),
        },
        "OPEN30" => ArrowKind::Open {
            size: dimasz,
            half_angle: 15.0_f32.to_radians(),
        },
        "OPEN90" => ArrowKind::Open {
            size: dimasz,
            half_angle: 45.0_f32.to_radians(),
        },
        "DOT" => ArrowKind::Dot {
            size: dimasz,
            filled: true,
        },
        "DOTSMALL" => ArrowKind::Dot {
            size: dimasz * 0.5,
            filled: true,
        },
        "DOTBLANK" => ArrowKind::Dot {
            size: dimasz,
            filled: false,
        },
        "DOTSMALLBLANK" => ArrowKind::Dot {
            size: dimasz * 0.5,
            filled: false,
        },
        "ORIGIN" | "ORIGIN2" | "ORIGININDICATOR" | "ORIGININDICATOR2" => {
            ArrowKind::Origin { size: dimasz }
        }
        "OBLIQUE" | "ARCHTICK" => ArrowKind::Tick { size: dimasz },
        "BOXFILLED" => ArrowKind::Box_ {
            size: dimasz,
            filled: true,
        },
        "BOXBLANK" | "BOX" => ArrowKind::Box_ {
            size: dimasz,
            filled: false,
        },
        "DATUMFILLED" | "DATUMTRIANGLEFILLED" => ArrowKind::Datum {
            size: dimasz,
            filled: true,
        },
        "DATUMBLANK" | "DATUMTRIANGLE" => ArrowKind::Datum {
            size: dimasz,
            filled: false,
        },
        "NONE" => ArrowKind::None,
        // INTEGRAL and other complex glyphs aren't reproduced here; fall through.
        _ => ArrowKind::Triangle {
            size: dimasz,
            filled: true,
            size_mul: 1.0,
        },
    }
}

pub(crate) struct DimGeom {
    pub(crate) ext_lines: Vec<[f32; 3]>,
    pub(crate) dim_lines: Vec<[f32; 3]>,
    pub(crate) arrow_fill: Vec<[f32; 3]>,
}

impl DimGeom {
    pub(crate) fn new() -> Self {
        Self {
            ext_lines: Vec::new(),
            dim_lines: Vec::new(),
            arrow_fill: Vec::new(),
        }
    }
}


/// Convert an acadrust `Color` to RGBA, falling back to `inherited` for
/// `ByLayer` / `ByBlock` (assumes those are already resolved upstream).
pub(crate) fn color_or_inherit(c: &AcadColor, inherited: [f32; 4]) -> [f32; 4] {
    match c.rgb() {
        Some((r, g, b)) => [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            inherited[3],
        ],
        None => inherited,
    }
}


// ── Entity Z helper ───────────────────────────────────────────────────────

/// Extract the Z elevation from a text/mtext entity.
pub(crate) fn entity_z(entity: &EntityType) -> f32 {
    match entity {
        EntityType::Text(t) => t.insertion_point.z as f32,
        EntityType::MText(t) => t.insertion_point.z as f32,
        _ => 0.0,
    }
}

/// Band width (drawing units) of a wide polyline whose flat band is drawn by
/// expanding its centre-line wire in the shader. `0.0` = a zero-width polyline,
/// a non-polyline (a normal screen-pixel wire), or a thickness-extruded one — an
/// LwPolyline thickens into a 3-D tube (`thick_wide_band`) and a Polyline2D
/// keeps its hatch band (only its centre-line extrudes; no tube yet), so neither
/// takes the flat shader band. A tapering polyline (per-vertex start/end widths)
/// collapses to its widest edge — the shader carries one width per wire.
fn polyline_band_width(entity: &EntityType) -> f32 {
    let w = match entity {
        EntityType::LwPolyline(p) if p.thickness.abs() <= 1e-10 => {
            let mut w = p.constant_width;
            for v in &p.vertices {
                w = w.max(v.start_width).max(v.end_width);
            }
            w
        }
        EntityType::Polyline2D(p) if p.thickness.abs() <= 1e-10 => {
            let mut w = p.start_width.max(p.end_width);
            for v in &p.vertices {
                w = w.max(v.start_width).max(v.end_width);
            }
            w
        }
        _ => 0.0,
    };
    if w > 1e-9 {
        w as f32
    } else {
        0.0
    }
}

// ── Fallback geometry (Viewport, Insert, Hatch outline, Ole2Frame) ───────
//
// Per-entity blocks have moved to their respective `entities/*.rs` files
// (Viewport, Insert, Hatch, Ole2Frame) via the `FallbackTess` trait. This
// function stays as the dispatcher used by the main `tessellate()` path.

use crate::entities::traits::FallbackTess;
use crate::scene::convert::tess_util::FallbackGeometry as Geometry;

fn fallback_geometry(entity: &EntityType) -> Geometry {
    match entity {
        EntityType::Viewport(vp) => vp.fallback_geometry(),
        EntityType::Insert(ins) => ins.fallback_geometry(),
        EntityType::Hatch(h) => h.fallback_geometry(),
        EntityType::Ole2Frame(ole) => ole.fallback_geometry(),
        // Modeler solids render as meshes (solid3d_tess). Their wire path
        // contributes only the pre-computed edge wires (empty for binary SAB)
        // plus an insertion snap — never the placeholder segment below, which
        // would otherwise draw a stray 1-unit line at the origin next to the
        // solid.
        EntityType::Solid3D(_)
        | EntityType::Region(_)
        | EntityType::Body(_)
        | EntityType::Surface(_) => {
            let pts = solid_wire_fallback(entity);
            let mut snap = vec![];
            if let Some(p) = crate::entities::solid3d::point_of_reference(entity) {
                snap.push((
                    Vec3::new((p.x) as f32, (p.y) as f32, (p.z) as f32),
                    SnapHint::Insertion,
                ));
            }
            (pts, snap, vec![], vec![])
        }
        _ => {
            let s = 0.5_f64;
            (vec![[-s, 0.0, 0.0], [s, 0.0, 0.0]], vec![], vec![], vec![])
        }
    }
}

/// Extract pre-computed edge-wire points from Solid3D / Region / Body entities.
///
/// AutoCAD stores explicit wire geometry (from SOLVIEW / 3DPLOT) alongside the
/// ACIS data.  We use this as a visible fallback when the SAT tessellator
/// produces no mesh (e.g. binary SAB data or unsupported geometry).
fn solid_wire_fallback(entity: &EntityType) -> Vec<[f64; 3]> {
    let Some(wires) = crate::entities::solid3d::fallback_wires(entity) else {
        return vec![];
    };
    if wires.is_empty() {
        return vec![];
    }

    let mut pts: Vec<[f64; 3]> = Vec::new();
    for wire in wires {
        if wire.points.len() < 2 {
            continue;
        }
        for v in &wire.points {
            pts.push([v.x, v.y, v.z]);
        }
        // NaN sentinel separates distinct wire segments.
        pts.push([f64::NAN, f64::NAN, f64::NAN]);
    }
    pts
}

pub(crate) fn push_tri(out: &mut Vec<[f32; 3]>, a: Vec3, b: Vec3, c: Vec3) {
    out.push([a.x, a.y, a.z]);
    out.push([b.x, b.y, b.z]);
    out.push([c.x, c.y, c.z]);
}

pub(crate) fn append_arrow(g: &mut DimGeom, tip: Vec3, dir: Vec3, arrow: &ArrowKind) {
    let dir = normalized_or(dir, Vec3::X);
    let perp = Vec3::new(-dir.y, dir.x, 0.0);
    match *arrow {
        ArrowKind::None => {}
        ArrowKind::Triangle {
            size,
            filled,
            size_mul,
        } => {
            let size = size * size_mul;
            let base = tip + dir * size;
            // ~1:6 length:half-width ratio (≈9.5° half-angle) matches
            // AutoCAD's standard ClosedFilled block.
            let half_w = size / 6.0;
            let left = base + perp * half_w;
            let right = base - perp * half_w;
            add_segment(&mut g.dim_lines, tip, left);
            add_segment(&mut g.dim_lines, left, right);
            add_segment(&mut g.dim_lines, right, tip);
            if filled {
                push_tri(&mut g.arrow_fill, tip, left, right);
            }
        }
        ArrowKind::Tick { size } => {
            // 45° oblique tick crossing the dim line at the tip; `size` is
            // the half-length (matches AutoCAD's DIMTSZ semantics).
            let off = (dir + perp).normalize_or_zero() * size;
            add_segment(&mut g.dim_lines, tip - off, tip + off);
        }
        ArrowKind::Open { size, half_angle } => {
            let base = tip + dir * size;
            let half_w = size * half_angle.tan();
            let left = base + perp * half_w;
            let right = base - perp * half_w;
            add_segment(&mut g.dim_lines, tip, left);
            add_segment(&mut g.dim_lines, tip, right);
        }
        ArrowKind::Dot { size, filled } => {
            let r = size * 0.5;
            const N: usize = 16;
            let mut ring: Vec<Vec3> = Vec::with_capacity(N + 1);
            for i in 0..=N {
                let a = i as f32 * std::f32::consts::TAU / N as f32;
                ring.push(tip + Vec3::new(a.cos() * r, a.sin() * r, 0.0));
            }
            add_polyline(&mut g.dim_lines, &ring);
            if filled {
                for i in 0..N {
                    push_tri(&mut g.arrow_fill, tip, ring[i], ring[i + 1]);
                }
            }
        }
        ArrowKind::Origin { size } => {
            // Small filled dot at the tip with a perpendicular tick crossing
            // the dim line — matches "_ORIGIN" / "_ORIGIN2" blocks.
            let r = size * 0.25;
            const N: usize = 12;
            let mut ring: Vec<Vec3> = Vec::with_capacity(N + 1);
            for i in 0..=N {
                let a = i as f32 * std::f32::consts::TAU / N as f32;
                ring.push(tip + Vec3::new(a.cos() * r, a.sin() * r, 0.0));
            }
            add_polyline(&mut g.dim_lines, &ring);
            for i in 0..N {
                push_tri(&mut g.arrow_fill, tip, ring[i], ring[i + 1]);
            }
            let half = size * 0.5;
            add_segment(&mut g.dim_lines, tip - perp * half, tip + perp * half);
        }
        ArrowKind::Box_ { size, filled } => {
            let half = size * 0.5;
            let p1 = tip - dir * half - perp * half;
            let p2 = tip + dir * half - perp * half;
            let p3 = tip + dir * half + perp * half;
            let p4 = tip - dir * half + perp * half;
            add_segment(&mut g.dim_lines, p1, p2);
            add_segment(&mut g.dim_lines, p2, p3);
            add_segment(&mut g.dim_lines, p3, p4);
            add_segment(&mut g.dim_lines, p4, p1);
            if filled {
                push_tri(&mut g.arrow_fill, p1, p2, p3);
                push_tri(&mut g.arrow_fill, p1, p3, p4);
            }
        }
        ArrowKind::Datum { size, filled } => {
            // Right-pointing triangle with the base perpendicular to the dim
            // line at the tip and the apex along +dir.
            let half = size * 0.5;
            let base_a = tip + perp * half;
            let base_b = tip - perp * half;
            let apex = tip + dir * size;
            add_segment(&mut g.dim_lines, base_a, apex);
            add_segment(&mut g.dim_lines, apex, base_b);
            add_segment(&mut g.dim_lines, base_b, base_a);
            if filled {
                push_tri(&mut g.arrow_fill, base_a, apex, base_b);
            }
        }
    }
}

pub(crate) fn add_segment(points: &mut Vec<[f32; 3]>, a: Vec3, b: Vec3) {
    if !points.is_empty() {
        points.push([f32::NAN, f32::NAN, f32::NAN]);
    }
    points.push([a.x, a.y, a.z]);
    points.push([b.x, b.y, b.z]);
}

pub(crate) fn add_polyline(points: &mut Vec<[f32; 3]>, polyline: &[Vec3]) {
    if polyline.len() < 2 {
        return;
    }
    if !points.is_empty() {
        points.push([f32::NAN, f32::NAN, f32::NAN]);
    }
    points.extend(polyline.iter().map(|p| [p.x, p.y, p.z]));
}

/// Returns the text position of a dimension in DXF world-space (f64, no offset applied).
/// Used when building a synthetic Text entity so tessellate() can apply world_offset itself.
/// When the saved `text_middle_point` is zero (i.e. AutoCAD never wrote one),
/// computes a fallback from the dim geometry and applies DIMTAD/DIMGAP.
pub(crate) fn normalized_or(v: Vec3, fallback: Vec3) -> Vec3 {
    if v.length_squared() <= 1e-12 {
        fallback
    } else {
        v.normalize()
    }
}

