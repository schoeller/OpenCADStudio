use acadrust::entities::Table;
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{ro_prop as ro, square_grip};
use crate::entities::text_support::{
    layout_mtext, MTextRenderOpts, MTextVAnchor, ResolvedTextStyle,
};
use crate::entities::traits::{Grippable, PropertyEditable, Transformable, TruckConvertible};
use crate::scene::convert::acad_to_truck::{TruckEntity, TruckObject};
use crate::scene::model::object::{GripApply, GripDef, PropSection, Property, PropValue};
use crate::scene::view::transform;
use crate::scene::model::wire_model::SnapHint;

fn v3(v: &acadrust::types::Vector3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

impl TruckConvertible for Table {
    fn to_truck(&self, document: &acadrust::CadDocument) -> Option<TruckEntity> {
        if self.rows.is_empty() || self.columns.is_empty() {
            return None;
        }

        // Lay the table out in a local frame with the origin at zero. The world
        // insertion point is added back as f64 at the widening step below, so
        // large coordinates (UTM etc.) keep full precision instead of snapping
        // onto the coarse f32 grid — which would collide cell-text baselines and
        // overflow the integer border-dedup keys.
        let base = [
            self.insertion_point.x,
            self.insertion_point.y,
            self.insertion_point.z,
        ];
        let origin = Vec3::ZERO;
        let h_raw = v3(&self.horizontal_direction);
        let h = if h_raw.length_squared() > 1e-10 {
            h_raw.normalize()
        } else {
            Vec3::X
        };
        // Perpendicular "down" direction in the drawing plane (tables grow downward)
        let v_down = Vec3::new(h.y, -h.x, 0.0);

        let col_offsets: Vec<f32> = {
            let mut off = 0.0f32;
            let mut v = vec![0.0f32];
            for col in &self.columns {
                off += col.width as f32;
                v.push(off);
            }
            v
        };
        let total_w = *col_offsets.last().unwrap_or(&0.0);

        let row_offsets: Vec<f32> = {
            let mut off = 0.0f32;
            let mut v = vec![0.0f32];
            for row in &self.rows {
                off += row.height as f32;
                v.push(off);
            }
            v
        };
        let total_h = *row_offsets.last().unwrap_or(&0.0);

        let mut pts: Vec<[f32; 3]> = Vec::new();
        let mut tris_pts: Vec<[f32; 3]> = Vec::new();

        // Per-cell borders. When a cell carries a CellStyle, honour the
        // visibility / `invisible` flag of each of its four borders so
        // hidden borders disappear from the grid. Cells with no style still
        // emit the standard four borders. To avoid drawing each shared edge
        // twice we coalesce the segments by their (start, end) coordinates.
        use rustc_hash::FxHashSet as HashSet;
        let mut emitted: HashSet<(i32, i32, i32, i32)> = HashSet::default();
        let try_add = |a: Vec3,
                       b: Vec3,
                       vis: bool,
                       emitted: &mut HashSet<(i32, i32, i32, i32)>,
                       pts: &mut Vec<[f32; 3]>| {
            if !vis {
                return;
            }
            let key = (
                (a.x * 1_000.0) as i32,
                (a.y * 1_000.0) as i32,
                (b.x * 1_000.0) as i32,
                (b.y * 1_000.0) as i32,
            );
            let key_rev = (key.2, key.3, key.0, key.1);
            if emitted.contains(&key) || emitted.contains(&key_rev) {
                return;
            }
            emitted.insert(key);
            if !pts.is_empty() {
                pts.push([f32::NAN; 3]);
            }
            pts.push([a.x, a.y, a.z]);
            pts.push([b.x, b.y, b.z]);
        };
        for (ri, row) in self.rows.iter().enumerate() {
            let row_top = row_offsets[ri];
            let row_bot = row_offsets
                .get(ri + 1)
                .copied()
                .unwrap_or(row_top + row.height as f32);
            for (ci, cell) in row.cells.iter().enumerate() {
                let col_left = col_offsets[ci];
                let col_right = col_offsets.get(ci + 1).copied().unwrap_or(
                    col_left + self.columns.get(ci).map(|c| c.width as f32).unwrap_or(1.0),
                );
                // Default to visible when no style override is present.
                let (top_vis, right_vis, bottom_vis, left_vis) = cell
                    .style
                    .as_ref()
                    .map(|s| {
                        (
                            !s.top_border.invisible,
                            !s.right_border.invisible,
                            !s.bottom_border.invisible,
                            !s.left_border.invisible,
                        )
                    })
                    .unwrap_or((true, true, true, true));
                let tl = origin + h * col_left + v_down * row_top;
                let tr = origin + h * col_right + v_down * row_top;
                let br_ = origin + h * col_right + v_down * row_bot;
                let bl = origin + h * col_left + v_down * row_bot;
                try_add(tl, tr, top_vis, &mut emitted, &mut pts);
                try_add(tr, br_, right_vis, &mut emitted, &mut pts);
                try_add(bl, br_, bottom_vis, &mut emitted, &mut pts);
                try_add(tl, bl, left_vis, &mut emitted, &mut pts);
            }
        }
        // Suppress unused-variable warnings now that the simple grid-pass
        // is gone — col/row offsets still feed cell drawing below.
        let _ = (total_w, total_h);

        // Cell text — resolve defaults via TableStyle, then layer per-cell
        // overrides on top. Resolution order (text height, text style, alignment):
        //   1. CellContent.* (per-content explicit override)
        //   2. CellStyle.*   (per-cell explicit override)
        //   3. TableStyle.<row_kind>_row_style.* (table-wide default for this row class)
        //   4. compiled-in fallback (0.18 / "txt" / MiddleCenter)
        //
        // Row classification: row 0 is Title (when not suppressed), row 1 is
        // Header (when not suppressed), everything else is Data. The two
        // suppressed flags shift the leading rows down to Data.
        let lookup_style = |h: acadrust::Handle| -> Option<&acadrust::tables::TextStyle> {
            document.text_styles.iter().find(|s| s.handle == h)
        };
        let table_style: Option<&acadrust::objects::TableStyle> =
            self.table_style_handle.and_then(|h| {
                document.objects.get(&h).and_then(|obj| match obj {
                    acadrust::objects::ObjectType::TableStyle(ts) => Some(ts),
                    _ => None,
                })
            });
        let title_suppressed = table_style.map(|t| t.title_suppressed).unwrap_or(false);
        let header_suppressed = table_style.map(|t| t.header_suppressed).unwrap_or(false);

        let font_for_handle = |handle: Option<acadrust::Handle>| -> Option<String> {
            handle.and_then(|h| lookup_style(h)).and_then(|s| {
                let mut font_name = if !s.true_type_font.trim().is_empty() {
                    s.true_type_font.trim().to_string()
                } else {
                    let file = s.font_file.trim();
                    if !file.is_empty() {
                        let basename = file.rsplit(['/', '\\']).next().unwrap_or(file);
                        let stem = basename.split('.').next().unwrap_or(basename).trim();
                        if !stem.is_empty() {
                            stem.to_string()
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                };
                if !crate::scene::text::lff::is_builtin(&font_name) {
                    if let Some(canonical) = crate::scene::text::sysfont::canonical_family_name(&font_name) {
                        font_name = canonical;
                    }
                }
                Some(font_name)
            })
        };
        // Build a ResolvedTextStyle for the cell — needed by the shared MText
        // pipeline so inline `\W`, `\Q`, etc. compose with the style baseline.
        let resolved_style_for_handle =
            |handle: Option<acadrust::Handle>, font_name: String| -> ResolvedTextStyle {
                let style = handle.and_then(|h| lookup_style(h));
                ResolvedTextStyle {
                    font_name,
                    width_factor: style.map(|s| s.width_factor as f32).unwrap_or(1.0),
                    oblique_angle: style.map(|s| s.oblique_angle as f32).unwrap_or(0.0),
                    is_backward: style.map(|s| s.is_backward()).unwrap_or(false),
                    is_upside_down: style.map(|s| s.is_upside_down()).unwrap_or(false),
                }
            };

        for (ri, row) in self.rows.iter().enumerate() {
            let row_top = row_offsets[ri];
            let row_bot = row_offsets
                .get(ri + 1)
                .copied()
                .unwrap_or(row_top + row.height as f32);
            let row_mid = (row_top + row_bot) * 0.5;

            // Pick the appropriate row_style from TableStyle for this row's role.
            let row_style: Option<&acadrust::objects::RowCellStyle> = table_style.map(|ts| {
                let kind = match (title_suppressed, header_suppressed, ri) {
                    (false, _, 0) => 0,     // title
                    (false, false, 1) => 1, // header
                    (true, false, 0) => 1,  // header pulled up
                    _ => 2,                 // data
                };
                match kind {
                    0 => &ts.title_row_style,
                    1 => &ts.header_row_style,
                    _ => &ts.data_row_style,
                }
            });

            for (ci, cell) in row.cells.iter().enumerate() {
                let text = cell.text_value();
                if text.is_empty() {
                    continue;
                }

                let col_left = col_offsets[ci];
                let col_width = self.columns.get(ci).map(|c| c.width as f32).unwrap_or(1.0);
                let col_right = col_left + col_width;

                // Resolve text height: content → cell-style → row-style → 0.18.
                let content = cell.contents.first();
                let cell_h = content
                    .map(|c| c.text_height)
                    .filter(|h| *h > 1e-6)
                    .or_else(|| {
                        cell.style
                            .as_ref()
                            .map(|s| s.text_height)
                            .filter(|h| *h > 1e-6)
                    })
                    .or_else(|| row_style.map(|s| s.text_height).filter(|h| *h > 1e-6))
                    .map(|h| h as f32)
                    .unwrap_or(0.18);
                let margin = cell_h * 0.5_f32;

                // Resolve text-style handle: content → cell-style → row-style.
                let style_handle = content
                    .and_then(|c| c.text_style_handle)
                    .or_else(|| cell.style.as_ref().and_then(|s| s.text_style_handle))
                    .or_else(|| row_style.and_then(|s| s.text_style_handle));
                let font_owned = font_for_handle(style_handle).unwrap_or_else(|| "txt".to_string());
                let resolved = resolved_style_for_handle(style_handle, font_owned);

                // Alignment resolution: cell.style.alignment (1-9) overrides;
                // otherwise fall back to row_style.alignment, then MiddleCenter.
                let align = cell
                    .style
                    .as_ref()
                    .map(|s| s.alignment)
                    .filter(|a| *a != 0)
                    .or_else(|| row_style.map(|s| s.alignment as i32))
                    .unwrap_or(5);
                let horiz = ((align - 1).rem_euclid(3)) + 1; // 1=left, 2=center, 3=right
                let vert = ((align - 1) / 3) + 1; // 1=top, 2=middle, 3=bottom

                // Position the cell's MText block anchor at the requested
                // alignment corner / midpoint of the cell's content area.
                let (x_offset, attach_h_anchor) = match horiz {
                    1 => (col_left + margin, 0.0_f32),
                    3 => (col_right - margin, 1.0_f32),
                    _ => (col_left + col_width * 0.5, 0.5_f32),
                };
                let (y_offset, v_anchor) = match vert {
                    1 => (row_top + margin, MTextVAnchor::Top),
                    3 => (row_bot - margin, MTextVAnchor::Bottom),
                    _ => (row_mid, MTextVAnchor::Middle),
                };
                let text_origin = origin + h * x_offset + v_down * y_offset;

                // Content rotation (radians) on top of table cell rotation.
                let rot = content.map(|c| c.rotation as f32).unwrap_or(0.0) + cell.rotation as f32;
                let layout = layout_mtext(&MTextRenderOpts {
                    // Not an MTEXT: text in a fixed box, never columnar.
                    columns: Default::default(),
                    value: text,
                    insertion: [text_origin.x as f64, text_origin.y as f64, origin.z as f64],
                    height: cell_h,
                    rect_w: 0.0,
                    rotation: rot,
                    style: &resolved,
                    attach_h_anchor,
                    v_anchor,
                    line_spacing_factor: 1.0,
                    vertical_text: false,
                    want_glyph_boxes: false,
                });
                // Flatten TextStroke groups into the table's Lines buffer.
                // Per-run inline `\C` / `\c` colour is dropped here because the
                // table emits a single TruckObject::Lines for borders + text;
                // tracking it would require splitting the table into multiple
                // WireModels per cell colour. Borders + uniform-coloured runs
                // honour the entity's outer colour.
                for ts in &layout.strokes {
                    let ox = ts.origin[0] as f32;
                    let oy = ts.origin[1] as f32;
                    for stroke in &ts.strokes {
                        if stroke.len() < 2 {
                            continue;
                        }
                        if !pts.is_empty() {
                            pts.push([f32::NAN; 3]);
                        }
                        for &[x, y] in stroke {
                            pts.push([x + ox, y + oy, origin.z]);
                        }
                    }
                    for &[x, y] in &ts.fill_tris {
                        tris_pts.push([x + ox, y + oy, origin.z]);
                    }
                }
            }
        }

        // The layout above is in a local f32 frame (small magnitudes). Widen to
        // f64 and add the world insertion so the absolute position carries full
        // precision; tessellate.rs then applies world_offset.
        let pts_f64: Vec<[f64; 3]> = pts
            .into_iter()
            .map(|[x, y, z]| {
                if x.is_nan() {
                    [f64::NAN, f64::NAN, f64::NAN]
                } else {
                    [x as f64 + base[0], y as f64 + base[1], z as f64 + base[2]]
                }
            })
            .collect();
        let fill_tris_f64: Vec<[f64; 3]> = tris_pts
            .into_iter()
            .map(|[x, y, z]| {
                [x as f64 + base[0], y as f64 + base[1], z as f64 + base[2]]
            })
            .collect();
        Some(TruckEntity {
            pick_tris: Vec::new(),
            object: TruckObject::Lines(pts_f64),
            snap_pts: vec![(glam::DVec3::new(self.insertion_point.x, self.insertion_point.y, self.insertion_point.z), SnapHint::Insertion)],
            tangent_geoms: vec![],
            key_vertices: vec![],
            fill_tris: fill_tris_f64,
        })
    }
}

/// Coloured synthesis render for tables WITHOUT a baked block (e.g. tables
/// created in-app). Emits one `WireModel` per distinct colour for cell fills,
/// per-cell text, and grid borders, honouring every `TableStyle` /
/// `RowCellStyle` / per-cell `CellStyle` field: fill colour + enable, text
/// colour, border type/weight/colour/visibility (incl. inside borders),
/// margins, and flow direction. Imported tables keep using AutoCAD's baked
/// block (see scene/mod.rs).
pub fn tessellate_table(
    tab: &Table,
    document: &acadrust::CadDocument,
    selected: bool,
    entity_color: [f32; 4],
    line_weight_px: f32,
    // Annotation scale: multiplies the table's paper-size geometry so an
    // annotative table renders at the current annotation scale. 1.0 for a
    // non-annotative table (its geometry is already at model size).
    anno_scale: f32,
) -> Vec<crate::scene::model::wire_model::WireModel> {
    use crate::scene::convert::tess_util::aci_to_rgba;
    use crate::scene::model::wire_model::WireModel;
    use acadrust::types::Color;
    use rustc_hash::FxHashMap as HashMap;

    if tab.rows.is_empty() || tab.columns.is_empty() {
        return Vec::new();
    }

    let rel = |p: Vec3| -> [f32; 3] {
        [
            (p.x as f64) as f32,
            (p.y as f64) as f32,
            (p.z as f64) as f32,
        ]
    };
    let resolve_col = |c: &Color, fallback: [f32; 4]| -> [f32; 4] {
        match c {
            Color::ByLayer | Color::ByBlock => fallback,
            _ => aci_to_rgba(c),
        }
    };
    let key4 = |c: [f32; 4]| -> [u8; 4] {
        [
            (c[0] * 255.0) as u8,
            (c[1] * 255.0) as u8,
            (c[2] * 255.0) as u8,
            (c[3] * 255.0) as u8,
        ]
    };
    let lw_px = |w: &acadrust::types::LineWeight| -> f32 {
        match w {
            acadrust::types::LineWeight::Value(v) if *v >= 0 => (*v as f32 / 100.0) * (96.0 / 25.4),
            _ => line_weight_px,
        }
    };

    let origin = v3(&tab.insertion_point);
    let h_raw = v3(&tab.horizontal_direction);
    let h = if h_raw.length_squared() > 1e-10 {
        h_raw.normalize()
    } else {
        Vec3::X
    };
    let v_down = Vec3::new(h.y, -h.x, 0.0);
    // Flow direction: `Up` stacks rows upward instead of downward.
    let table_style: Option<&acadrust::objects::TableStyle> =
        tab.table_style_handle.and_then(|h| {
            document.objects.get(&h).and_then(|obj| match obj {
                acadrust::objects::ObjectType::TableStyle(ts) => Some(ts),
                _ => None,
            })
        });
    let flow_up = matches!(
        table_style.map(|t| t.flow_direction),
        Some(acadrust::objects::TableFlowDirection::Up)
    );
    let v_flow = if flow_up { -v_down } else { v_down };

    let col_offsets: Vec<f32> = {
        let mut off = 0.0f32;
        let mut v = vec![0.0f32];
        for col in &tab.columns {
            off += col.width as f32 * anno_scale;
            v.push(off);
        }
        v
    };
    let row_offsets: Vec<f32> = {
        let mut off = 0.0f32;
        let mut v = vec![0.0f32];
        for row in &tab.rows {
            off += row.height as f32 * anno_scale;
            v.push(off);
        }
        v
    };

    let title_suppressed = table_style.map(|t| t.title_suppressed).unwrap_or(false);
    let header_suppressed = table_style.map(|t| t.header_suppressed).unwrap_or(false);
    let h_margin = table_style
        .map(|t| t.horizontal_margin as f32)
        .unwrap_or(0.0) * anno_scale;
    let v_margin = table_style.map(|t| t.vertical_margin as f32).unwrap_or(0.0) * anno_scale;

    let lookup_style = |hh: acadrust::Handle| -> Option<&acadrust::tables::TextStyle> {
        document.text_styles.iter().find(|s| s.handle == hh)
    };
    let font_for_handle = |handle: Option<acadrust::Handle>| -> Option<String> {
        handle.and_then(lookup_style).and_then(|s| {
            let mut font_name = if !s.true_type_font.trim().is_empty() {
                s.true_type_font.trim().to_string()
            } else {
                let file = s.font_file.trim();
                let basename = file.rsplit(['/', '\\']).next().unwrap_or(file);
                let stem = basename.split('.').next().unwrap_or(basename).trim();
                if !stem.is_empty() {
                    stem.to_string()
                } else {
                    return None;
                }
            };
            if !crate::scene::text::lff::is_builtin(&font_name) {
                if let Some(canonical) = crate::scene::text::sysfont::canonical_family_name(&font_name) {
                    font_name = canonical;
                }
            }
            Some(font_name)
        })
    };
    let resolved_style_for_handle =
        |handle: Option<acadrust::Handle>, font_name: String| -> ResolvedTextStyle {
            let style = handle.and_then(lookup_style);
            ResolvedTextStyle {
                font_name,
                width_factor: style.map(|s| s.width_factor as f32).unwrap_or(1.0),
                oblique_angle: style.map(|s| s.oblique_angle as f32).unwrap_or(0.0),
                is_backward: style.map(|s| s.is_backward()).unwrap_or(false),
                is_upside_down: style.map(|s| s.is_upside_down()).unwrap_or(false),
            }
        };

    // Accumulators keyed by quantised colour (+ weight for borders).
    let mut fills: HashMap<[u8; 4], ([f32; 4], Vec<[f32; 3]>)> = HashMap::default();
    // SDF cell text: glyph quads (per-vertex coloured) collected across all
    // cells; emitted as one text-carrying wire at the end.
    let mut text_verts: Vec<crate::scene::pipeline::text_gpu::TextVertex> = Vec::new();
    let mut borders: HashMap<([u8; 4], u32), ([f32; 4], f32, Vec<[f32; 3]>)> = HashMap::default();
    let mut emitted: rustc_hash::FxHashSet<(i32, i32, i32, i32)> = rustc_hash::FxHashSet::default();
    let sel_col = WireModel::SELECTED;

    let mut add_edge = |a: Vec3, b: Vec3, col: [f32; 4], lw: f32| {
        let k = (
            (a.x * 1000.0) as i32,
            (a.y * 1000.0) as i32,
            (b.x * 1000.0) as i32,
            (b.y * 1000.0) as i32,
        );
        let kr = (k.2, k.3, k.0, k.1);
        if emitted.contains(&k) || emitted.contains(&kr) {
            return;
        }
        emitted.insert(k);
        let entry = borders
            .entry((key4(col), (lw * 100.0) as u32))
            .or_insert_with(|| (col, lw, Vec::new()));
        if !entry.2.is_empty() {
            entry.2.push([f32::NAN; 3]);
        }
        entry.2.push(rel(a));
        entry.2.push(rel(b));
    };

    for (ri, row) in tab.rows.iter().enumerate() {
        let row_top = row_offsets[ri];
        let row_bot = row_offsets
            .get(ri + 1)
            .copied()
            .unwrap_or(row_top + row.height as f32 * anno_scale);
        let row_mid = (row_top + row_bot) * 0.5;
        let row_style: Option<&acadrust::objects::RowCellStyle> = table_style.map(|ts| {
            let kind = match (title_suppressed, header_suppressed, ri) {
                (false, _, 0) => 0,
                (false, false, 1) => 1,
                (true, false, 0) => 1,
                _ => 2,
            };
            match kind {
                0 => &ts.title_row_style,
                1 => &ts.header_row_style,
                _ => &ts.data_row_style,
            }
        });

        for (ci, cell) in row.cells.iter().enumerate() {
            let col_left = col_offsets[ci];
            let col_width = tab.columns.get(ci).map(|c| c.width as f32).unwrap_or(1.0) * anno_scale;
            let col_right = col_left + col_width;
            let tl = origin + h * col_left + v_flow * row_top;
            let tr = origin + h * col_right + v_flow * row_top;
            let br_ = origin + h * col_right + v_flow * row_bot;
            let bl = origin + h * col_left + v_flow * row_bot;

            // ── Fill ──────────────────────────────────────────────────────
            let (fill_on, fill_color) = if let Some(cs) = &cell.style {
                (cs.fill_enabled, cs.background_color)
            } else if let Some(rs) = row_style {
                (rs.fill_enabled, rs.fill_color)
            } else {
                (false, Color::ByLayer)
            };
            if fill_on {
                let col = resolve_col(&fill_color, entity_color);
                let buf = &mut fills
                    .entry(key4(col))
                    .or_insert_with(|| (col, Vec::new()))
                    .1;
                for v in [bl, br_, tr, bl, tr, tl] {
                    buf.push(rel(v));
                }
            }

            // ── Borders (per edge: cell override → row style → default) ───
            // (top, right, bottom, left)
            let cell_b = cell.style.as_ref();
            let edge = |which: u8| -> (bool, [f32; 4], f32) {
                if let Some(cs) = cell_b {
                    let b = match which {
                        0 => &cs.top_border,
                        1 => &cs.right_border,
                        2 => &cs.bottom_border,
                        _ => &cs.left_border,
                    };
                    (
                        !b.invisible,
                        if selected {
                            sel_col
                        } else {
                            resolve_col(&b.color, entity_color)
                        },
                        lw_px(&b.line_weight),
                    )
                } else if let Some(rs) = row_style {
                    let b = match which {
                        0 => &rs.top_border,
                        1 => &rs.right_border,
                        2 => &rs.bottom_border,
                        _ => &rs.left_border,
                    };
                    (
                        !b.is_invisible,
                        if selected {
                            sel_col
                        } else {
                            resolve_col(&b.color, entity_color)
                        },
                        lw_px(&b.line_weight),
                    )
                } else {
                    (
                        true,
                        if selected { sel_col } else { entity_color },
                        line_weight_px,
                    )
                }
            };
            let (tv, tc, tw) = edge(0);
            if tv {
                add_edge(tl, tr, tc, tw);
            }
            let (rv, rc, rw) = edge(1);
            if rv {
                add_edge(tr, br_, rc, rw);
            }
            let (bv, bc, bw) = edge(2);
            if bv {
                add_edge(bl, br_, bc, bw);
            }
            let (lv, lc, lw) = edge(3);
            if lv {
                add_edge(tl, bl, lc, lw);
            }

            // ── Text ──────────────────────────────────────────────────────
            let text = cell.text_value();
            if text.is_empty() {
                continue;
            }
            let content = cell.contents.first();
            let cell_h = content
                .map(|c| c.text_height)
                .filter(|h| *h > 1e-6)
                .or_else(|| {
                    cell.style
                        .as_ref()
                        .map(|s| s.text_height)
                        .filter(|h| *h > 1e-6)
                })
                .or_else(|| row_style.map(|s| s.text_height).filter(|h| *h > 1e-6))
                .map(|h| h as f32)
                .unwrap_or(0.18) * anno_scale;
            let m_h = if h_margin > 1e-6 {
                h_margin
            } else {
                cell_h * 0.5
            };
            let m_v = if v_margin > 1e-6 {
                v_margin
            } else {
                cell_h * 0.5
            };

            let style_handle = content
                .and_then(|c| c.text_style_handle)
                .or_else(|| cell.style.as_ref().and_then(|s| s.text_style_handle))
                .or_else(|| row_style.and_then(|s| s.text_style_handle));
            let font_owned = font_for_handle(style_handle).unwrap_or_else(|| "txt".to_string());
            let resolved = resolved_style_for_handle(style_handle, font_owned);

            let align = cell
                .style
                .as_ref()
                .map(|s| s.alignment)
                .filter(|a| *a != 0)
                .or_else(|| row_style.map(|s| s.alignment as i32))
                .unwrap_or(5);
            let horiz = ((align - 1).rem_euclid(3)) + 1;
            let vert = ((align - 1) / 3) + 1;
            let (x_offset, attach_h_anchor) = match horiz {
                1 => (col_left + m_h, 0.0_f32),
                3 => (col_right - m_h, 1.0_f32),
                _ => (col_left + col_width * 0.5, 0.5_f32),
            };
            let (y_offset, v_anchor) = match vert {
                1 => (row_top + m_v, MTextVAnchor::Top),
                3 => (row_bot - m_v, MTextVAnchor::Bottom),
                _ => (row_mid, MTextVAnchor::Middle),
            };
            let to = origin + h * x_offset + v_flow * y_offset;
            let rot = content.map(|c| c.rotation as f32).unwrap_or(0.0) + cell.rotation as f32;
            let layout = layout_mtext(&MTextRenderOpts {
                // Not an MTEXT: text in a fixed box, never columnar.
                columns: Default::default(),
                value: text,
                insertion: [(to.x as f64), (to.y as f64), (to.z as f64)],
                height: cell_h,
                rect_w: 0.0,
                rotation: rot,
                style: &resolved,
                attach_h_anchor,
                v_anchor,
                line_spacing_factor: 1.0,
                vertical_text: false,
                want_glyph_boxes: false,
            });

            let tcol = if selected {
                sel_col
            } else if let Some(cs) = &cell.style {
                resolve_col(&cs.content_color, entity_color)
            } else if let Some(rs) = row_style {
                resolve_col(&rs.text_color, entity_color)
            } else {
                entity_color
            };
            // SDF glyph quads per run at its placed origin (rotation baked in by
            // layout_glyph_quads), coloured by the cell's text colour.
            if let Ok(mut atlas) = crate::scene::text::sdf_atlas::text_atlas().lock() {
                for ts in &layout.strokes {
                    let Some(run) = &ts.run else { continue };
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
                        &mut text_verts,
                        &quads,
                        [ts.origin[0], ts.origin[1], to.z as f64],
                        1.0,
                        tcol,
                        0.0,
                    );
                }
            }
        }
    }

    let name = tab.common.handle.value().to_string();
    let mk =
        |color: [f32; 4], points: Vec<[f32; 3]>, fill_tris: Vec<[f32; 3]>, lw: f32| -> WireModel {
            WireModel {
                taper_widths: Vec::new(),
                world_width: 0.0,
                depth_override: None,
                fill_is_3d: false,
                pick_tris: Vec::new(),
                pick_tris_low: Vec::new(),
            dash_from_start: false,
            dash_align_end: None,
            text_verts: Vec::new(),
                name: name.clone(),
                points,
                points_low: Vec::new(),
                color,
                selected,
                pattern_length: 0.0,
                pattern: [0.0; 8],
                line_weight_px: lw,
                aci: 0,
                snap_pts: vec![],
                tangent_geoms: vec![],
                key_vertices: vec![],
                aabb: WireModel::UNBOUNDED_AABB,
                plinegen: true,
                fill_tris,
                // fill_tris_low intentionally empty: this fill renders on the
                // top-level path, where consumers (face3d_gpu, xclip) treat a
                // short low half as all-zero, so it draws at f32 precision
                // (sub-metre error at UTM scale) — not a crash. Follow-up:
                // double-single-split via points_to_ds to match emit_wire.
                fill_tris_low: Vec::new(),
            }
        };

    let mut out: Vec<WireModel> = Vec::new();
    // Fills first (drawn under borders/text).
    for (_, (color, tris)) in fills {
        if !tris.is_empty() {
            out.push(mk(color, vec![], tris, 1.0));
        }
    }
    for (_, (color, lw, pts)) in borders {
        if !pts.is_empty() {
            out.push(mk(color, pts, vec![], lw));
        }
    }
    // SDF cell text: one wire carrying the glyph quads (per-vertex coloured) +
    // a glyph-bounds AABB (f64 accumulate → f32) so the text draws + picks;
    // empty points so it adds no stroke geometry.
    if !text_verts.is_empty() {
        let (mut nx, mut ny, mut xx, mut xy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for v in &text_verts {
            let x = v.pos[0] as f64 + v.pos_low[0] as f64;
            let y = v.pos[1] as f64 + v.pos_low[1] as f64;
            nx = nx.min(x);
            xx = xx.max(x);
            ny = ny.min(y);
            xy = xy.max(y);
        }
        let mut w = mk(entity_color, vec![], vec![], line_weight_px);
        w.aabb = [nx as f32, ny as f32, xx as f32, xy as f32];
        w.text_verts = text_verts;
        out.push(w);
    }
    out
}

impl Grippable for Table {
    fn grips(&self) -> Vec<GripDef> {
        vec![square_grip(
            0,
            glam::DVec3::new(
                self.insertion_point.x,
                self.insertion_point.y,
                self.insertion_point.z,
            ),
        )]
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if grip_id == 0 {
            match apply {
                GripApply::Translate(d) => {
                    self.insertion_point.x += d.x as f64;
                    self.insertion_point.y += d.y as f64;
                    self.insertion_point.z += d.z as f64;
                }
                GripApply::Absolute(p) => {
                    self.insertion_point.x = p.x as f64;
                    self.insertion_point.y = p.y as f64;
                    self.insertion_point.z = p.z as f64;
                }
            }
        }
    }
}

impl PropertyEditable for Table {
    fn geometry_properties(&self, _text_style_names: &[String]) -> Vec<PropSection> {
        use crate::entities::common::edit_prop as edit;
        use acadrust::entities::table::BreakOptionFlags;

        let fmt_h = |oh: &Option<acadrust::types::Handle>| -> String {
            match oh {
                Some(h) if !h.is_null() => format!("{:X}", h.value()),
                _ => "(none)".to_string(),
            }
        };
        let toggle = |label: &'static str, field: &'static str, b: bool| -> Property {
            Property {
                label: label.into(),
                field,
                value: PropValue::BoolToggle { field, value: b },
            }
        };
        // Direction = angle of the horizontal direction vector in the XY plane.
        let direction_deg =
            (self.horizontal_direction.y.atan2(self.horizontal_direction.x)).to_degrees();

        vec![
            PropSection {
                title: "Table".into(),
                props: vec![
                    ro(
                        "Table style",
                        "tbl_style_handle",
                        fmt_h(&self.table_style_handle),
                    ),
                    ro("Rows", "tbl_rows", self.rows.len().to_string()),
                    ro("Columns", "tbl_cols", self.columns.len().to_string()),
                    ro("Direction", "tbl_direction", format!("{:.4}", direction_deg)),
                    ro(
                        "Table width",
                        "tbl_width",
                        format!("{:.4}", self.total_width()),
                    ),
                    ro(
                        "Table height",
                        "tbl_height",
                        format!("{:.4}", self.total_height()),
                    ),
                ],
            },
            PropSection {
                title: "Table Breaks".into(),
                props: vec![
                    toggle(
                        "Enabled",
                        "tbl_break_enabled",
                        self.break_options.contains(BreakOptionFlags::ENABLE_BREAKS),
                    ),
                    ro(
                        "Direction",
                        "tbl_break_direction",
                        format!("{:?}", self.break_flow_direction),
                    ),
                    toggle(
                        "Repeat top labels",
                        "tbl_break_repeat_top",
                        self.break_options
                            .contains(BreakOptionFlags::REPEAT_TOP_LABELS),
                    ),
                    toggle(
                        "Repeat bottom labels",
                        "tbl_break_repeat_bottom",
                        self.break_options
                            .contains(BreakOptionFlags::REPEAT_BOTTOM_LABELS),
                    ),
                    toggle(
                        "Manual positions",
                        "tbl_break_manual_positions",
                        self.break_options
                            .contains(BreakOptionFlags::ALLOW_MANUAL_POSITIONS),
                    ),
                    toggle(
                        "Manual heights",
                        "tbl_break_manual_heights",
                        self.break_options
                            .contains(BreakOptionFlags::ALLOW_MANUAL_HEIGHTS),
                    ),
                    ro("Break height", "tbl_break_height", String::new()),
                    edit("Spacing", "tbl_break_spacing", self.break_spacing),
                ],
            },
        ]
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        use crate::entities::common::parse_f64;
        use acadrust::entities::table::BreakOptionFlags;
        let flag = match field {
            "tbl_break_enabled" => Some(BreakOptionFlags::ENABLE_BREAKS),
            "tbl_break_repeat_top" => Some(BreakOptionFlags::REPEAT_TOP_LABELS),
            "tbl_break_repeat_bottom" => Some(BreakOptionFlags::REPEAT_BOTTOM_LABELS),
            "tbl_break_manual_positions" => Some(BreakOptionFlags::ALLOW_MANUAL_POSITIONS),
            "tbl_break_manual_heights" => Some(BreakOptionFlags::ALLOW_MANUAL_HEIGHTS),
            _ => None,
        };
        if let Some(flag) = flag {
            let on = if value == "toggle" {
                !self.break_options.contains(flag)
            } else {
                value == "true"
            };
            self.break_options.set(flag, on);
            return;
        }
        if field == "tbl_break_spacing" {
            if let Some(v) = parse_f64(value) {
                self.break_spacing = v;
            }
        }
    }
}

impl Transformable for Table {
    fn apply_transform(&mut self, t: &EntityTransform) {
        transform::apply_standard_entity_transform(self, t, |entity, p1, p2| {
            transform::reflect_xy_point(
                &mut entity.insertion_point.x,
                &mut entity.insertion_point.y,
                p1,
                p2,
            );
            // Reflect the horizontal direction by reflecting a tip point
            let mut tip_x = entity.insertion_point.x + entity.horizontal_direction.x;
            let mut tip_y = entity.insertion_point.y + entity.horizontal_direction.y;
            transform::reflect_xy_point(&mut tip_x, &mut tip_y, p1, p2);
            entity.horizontal_direction.x = tip_x - entity.insertion_point.x;
            entity.horizontal_direction.y = tip_y - entity.insertion_point.y;
        });
    }
}
