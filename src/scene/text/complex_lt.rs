//! CPU-side complex linetype tessellation.
//!
//! Walks the entity path and applies LT elements (dashes, gaps, dots, shapes),
//! returning one `WireModel` per continuous stroke.
//!
//! Coordinate convention: 2D entities live in the **XZ** plane (Y = elevation).
//! Shape X → along the linetype direction; Shape Y → perpendicular in XZ.

use crate::entities::text_support::resolve_dxf_special_chars;
use crate::linetypes::{ComplexLt, LtSegment};
use crate::scene::text::lff;
use crate::scene::model::wire_model::WireModel;

// ── Public entry point ────────────────────────────────────────────────────

/// Apply `lt` along `path_pts` (the entity's tessellated world-space strip).
///
/// Returns one `WireModel` per continuous stroke. Pass the entity's `name`,
/// `color`, `selected` flag, and `line_weight_px` so the WireModels inherit
/// the entity's visual properties.
pub fn apply_along(
    name: &str,
    path_pts: &[[f32; 3]],
    lt: &ComplexLt,
    scale: f32,
    color: [f32; 4],
    selected: bool,
    line_weight_px: f32,
) -> Vec<WireModel> {
    if path_pts.len() < 2 || lt.segments.is_empty() {
        return vec![];
    }

    let scaled: Vec<LtSeg> = scale_segments(&lt.segments, scale);
    if scaled.is_empty() {
        return vec![];
    }

    let pattern_len: f32 = scaled
        .iter()
        .map(|s| match s {
            LtSeg::Dash(l) | LtSeg::Space(l) => *l,
            LtSeg::Dot | LtSeg::Shape { .. } | LtSeg::Text { .. } => 0.0,
        })
        .sum();
    if pattern_len < 1e-10 {
        return vec![];
    }

    // Guard against pattern blow-up. The per-segment walk below emits at least
    // one stroke vertex per pattern element, so the total vertex count scales
    // with `path_len / pattern_len`. On a large-extent drawing (e.g. a city/
    // street map) carrying a finely-scaled complex linetype, that ratio can
    // reach billions — the strokes Vec grows until the process is OOM-killed,
    // single-threaded, before the drawing ever finishes loading. Past a sane
    // repeat count the dashes are sub-pixel anyway, so render the base wire
    // solid: returning empty here makes the caller fall back to the solid base.
    const MAX_PATTERN_REPEATS: f32 = 1_000_000.0;
    let path_len: f32 = path_pts
        .windows(2)
        .map(|w| {
            let d = [w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .sum();
    if path_len / pattern_len > MAX_PATTERN_REPEATS {
        return vec![];
    }

    let mut strokes: Vec<Vec<[f32; 3]>> = Vec::new();
    let mut cur_stroke: Vec<[f32; 3]> = Vec::new();

    let mut elem_idx: usize = 0;
    let mut elem_consumed: f32 = 0.0;

    for i in 0..path_pts.len() - 1 {
        let ps = path_pts[i];
        let pe = path_pts[i + 1];

        let dx = pe[0] - ps[0];
        let dy = pe[1] - ps[1];
        let dz = pe[2] - ps[2];
        let seg_len = (dx * dx + dy * dy + dz * dz).sqrt();
        if seg_len < 1e-10 {
            continue;
        }

        let fwd = [dx / seg_len, dy / seg_len, dz / seg_len];
        let perp = [-fwd[1], fwd[0], fwd[2]];

        let mut pos = 0.0f32;

        while pos < seg_len - 1e-6 {
            let idx = elem_idx % scaled.len();

            match &scaled[idx] {
                LtSeg::Dash(dash_len) => {
                    let remaining_dash = dash_len - elem_consumed;
                    let remaining_seg = seg_len - pos;
                    let advance = remaining_dash.min(remaining_seg);

                    let p_start = lerp(ps, pe, pos / seg_len);
                    let p_end = lerp(ps, pe, (pos + advance) / seg_len);

                    if cur_stroke.is_empty() {
                        cur_stroke.push(p_start);
                    }
                    cur_stroke.push(p_end);

                    pos += advance;
                    elem_consumed += advance;
                    if (elem_consumed - dash_len).abs() < 1e-6 {
                        elem_idx += 1;
                        elem_consumed = 0.0;
                    }
                }

                LtSeg::Space(space_len) => {
                    let remaining = space_len - elem_consumed;
                    let advance = remaining.min(seg_len - pos);

                    flush(&mut cur_stroke, &mut strokes);

                    pos += advance;
                    elem_consumed += advance;
                    if (elem_consumed - space_len).abs() < 1e-6 {
                        elem_idx += 1;
                        elem_consumed = 0.0;
                    }
                }

                LtSeg::Dot => {
                    flush(&mut cur_stroke, &mut strokes);
                    let p = lerp(ps, pe, pos / seg_len);
                    strokes.push(vec![p, p]);
                    elem_idx += 1;
                    elem_consumed = 0.0;
                    if pos < 1e-6 {
                        break;
                    }
                }

                LtSeg::Shape {
                    name: sh_name,
                    x,
                    y,
                    scale: sh_scale,
                    rot_deg,
                } => {
                    flush(&mut cur_stroke, &mut strokes);

                    let insert = lerp(ps, pe, pos / seg_len);
                    let insert = offset_pt(insert, fwd, perp, *x, *y);

                    let shape_strokes = emit_shape(sh_name, insert, fwd, perp, *sh_scale, *rot_deg);
                    strokes.extend(shape_strokes);

                    elem_idx += 1;
                    elem_consumed = 0.0;
                    if pos < 1e-6 {
                        break;
                    }
                }
                LtSeg::Text {
                    text,
                    style,
                    x,
                    y,
                    scale: tx_scale,
                    rot_deg,
                } => {
                    flush(&mut cur_stroke, &mut strokes);

                    let insert = lerp(ps, pe, pos / seg_len);
                    let insert = offset_pt(insert, fwd, perp, *x, *y);
                    let fwd_angle = fwd[1].atan2(fwd[0]) + rot_deg.to_radians();
                    let resolved = resolve_dxf_special_chars(text);
                    let text_strokes = lff::tessellate_text_ex(
                        [insert[0], insert[1]],
                        *tx_scale,
                        fwd_angle,
                        1.0,
                        0.0,
                        style,
                        &resolved,
                    );
                    for stroke in &text_strokes {
                        if stroke.len() >= 2 {
                            let pts: Vec<[f32; 3]> =
                                stroke.iter().map(|&[sx, sy]| [sx, sy, insert[2]]).collect();
                            strokes.push(pts);
                        }
                    }

                    elem_idx += 1;
                    elem_consumed = 0.0;
                    if pos < 1e-6 {
                        break;
                    }
                }
            }
        }
    }

    flush(&mut cur_stroke, &mut strokes);

    strokes
        .into_iter()
        .filter(|s| s.len() >= 2)
        .map(|pts| WireModel {
            name: name.to_string(),
            points: pts,
            color,
            selected,
            pattern_length: 0.0,
            pattern: [0.0; 8],
            line_weight_px,
            snap_pts: vec![],
            tangent_geoms: vec![],
            aci: 0,
            key_vertices: vec![],
            aabb: WireModel::UNBOUNDED_AABB,
            plinegen: true,
            vp_scissor: None,
            fill_tris: vec![],
        })
        .collect()
}

// ── Helpers ───────────────────────────────────────────────────────────────

#[derive(Clone)]
enum LtSeg {
    Dash(f32),
    Space(f32),
    Dot,
    Shape {
        name: String,
        x: f32,
        y: f32,
        scale: f32,
        rot_deg: f32,
    },
    Text {
        text: String,
        style: String,
        x: f32,
        y: f32,
        scale: f32,
        rot_deg: f32,
    },
}

fn scale_segments(segs: &[LtSegment], scale: f32) -> Vec<LtSeg> {
    segs.iter()
        .map(|s| match s {
            LtSegment::Dash(l) => LtSeg::Dash(l * scale),
            LtSegment::Space(l) => LtSeg::Space(l * scale),
            LtSegment::Dot => LtSeg::Dot,
            LtSegment::Shape {
                name,
                x,
                y,
                scale: sh_scale,
                rot_deg,
            } => LtSeg::Shape {
                name: name.clone(),
                x: x * scale,
                y: y * scale,
                scale: *sh_scale * scale,
                rot_deg: *rot_deg,
            },
            LtSegment::Text {
                text,
                style,
                x,
                y,
                scale: tx_scale,
                rot_deg,
            } => LtSeg::Text {
                text: text.clone(),
                style: style.clone(),
                x: x * scale,
                y: y * scale,
                scale: *tx_scale * scale,
                rot_deg: *rot_deg,
            },
        })
        .collect()
}

fn flush(cur: &mut Vec<[f32; 3]>, strokes: &mut Vec<Vec<[f32; 3]>>) {
    if !cur.is_empty() {
        strokes.push(std::mem::take(cur));
    }
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn offset_pt(pt: [f32; 3], fwd: [f32; 3], perp: [f32; 3], dx: f32, dy: f32) -> [f32; 3] {
    [
        pt[0] + fwd[0] * dx + perp[0] * dy,
        pt[1] + fwd[1] * dx + perp[1] * dy,
        pt[2] + fwd[2] * dx + perp[2] * dy,
    ]
}

/// Transform a named linetype shape (from the converted `ltypeshp` LFF font)
/// into world-space strokes at the pen position.
fn emit_shape(
    name: &str,
    insert: [f32; 3],
    fwd: [f32; 3],
    perp: [f32; 3],
    scale: f32,
    rot_deg: f32,
) -> Vec<Vec<[f32; 3]>> {
    let shape = match lff::shape(name) {
        Some(s) => s,
        None => return vec![],
    };

    let rot_r = rot_deg.to_radians();
    let (cos_r, sin_r) = (rot_r.cos(), rot_r.sin());

    shape
        .strokes
        .iter()
        .map(|stroke| {
            stroke
                .iter()
                .map(|&[lx, ly]| {
                    let scaled_x = lx * scale;
                    let scaled_y = ly * scale;
                    // Rotate in the shape's local frame, then place along the
                    // line tangent (fwd) and perpendicular (perp).
                    let along_fwd = cos_r * scaled_x - sin_r * scaled_y;
                    let along_perp = sin_r * scaled_x + cos_r * scaled_y;
                    [
                        insert[0] + fwd[0] * along_fwd + perp[0] * along_perp,
                        insert[1] + fwd[1] * along_fwd + perp[1] * along_perp,
                        insert[2] + fwd[2] * along_fwd + perp[2] * along_perp,
                    ]
                })
                .collect()
        })
        .collect()
}
