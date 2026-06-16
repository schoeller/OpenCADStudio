use acadrust::entities::{LeaderContentType, MultiLeader, MultiLeaderPathType, TextAttachmentType};

use crate::entities::text_support::{
    layout_mtext, resolve_text_style, MTextRenderOpts, MTextVAnchor, ResolvedTextStyle,
};

/// Map MLEADER's vertical attachment enum onto the shared `MTextVAnchor`
/// used by `layout_mtext`. Replaces the old `v_offset_for_attachment`
/// (which inlined the offset math); the shared pipeline now derives the
/// offset from the variant + n_lines / line_h.
pub(crate) fn mleader_v_anchor(attach: TextAttachmentType) -> MTextVAnchor {
    match attach {
        TextAttachmentType::TopOfTopLine => MTextVAnchor::Top,
        TextAttachmentType::MiddleOfTopLine => MTextVAnchor::MiddleOfTopLine,
        TextAttachmentType::MiddleOfText
        | TextAttachmentType::CenterOfText
        | TextAttachmentType::CenterOfTextOverline => MTextVAnchor::Middle,
        TextAttachmentType::MiddleOfBottomLine => MTextVAnchor::MiddleOfBottomLine,
        TextAttachmentType::BottomOfBottomLine | TextAttachmentType::BottomLine => {
            MTextVAnchor::Bottom
        }
        TextAttachmentType::BottomOfTopLineUnderlineBottomLine
        | TextAttachmentType::BottomOfTopLineUnderlineTopLine
        | TextAttachmentType::BottomOfTopLineUnderlineAll => MTextVAnchor::BottomOfTopLine,
    }
}
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{
    center_grip, edit_prop as edit, ro_prop as ro, square_grip, triangle_grip,
};
use crate::entities::traits::TruckConvertible;
use crate::scene::convert::acad_to_truck::{TruckEntity, TruckObject};
use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};
use crate::scene::model::wire_model::{SnapHint, TangentGeom};

// ── TruckConvertible ────────────────────────────────────────────────────────

/// Catmull-Rom spline tessellation through `ctrl` points, `segs_per_span` segments each.
/// Operates in f64 so it can be applied to either WCS-direct coordinates (entity path)
/// or offset-relative coordinates (scene path) without precision loss.
pub(crate) fn catmull_rom_pts(ctrl: &[[f64; 3]], segs_per_span: u32) -> Vec<[f64; 3]> {
    let n = ctrl.len();
    let mut out = Vec::new();
    for i in 0..n.saturating_sub(1) {
        let p0 = if i == 0 { ctrl[0] } else { ctrl[i - 1] };
        let p1 = ctrl[i];
        let p2 = ctrl[i + 1];
        let p3 = if i + 2 < n { ctrl[i + 2] } else { ctrl[n - 1] };
        for j in 0..=segs_per_span {
            let t = j as f64 / segs_per_span as f64;
            let t2 = t * t;
            let t3 = t2 * t;
            let mut pt = [0.0_f64; 3];
            for k in 0..3 {
                pt[k] = 0.5
                    * ((2.0 * p1[k])
                        + (-p0[k] + p2[k]) * t
                        + (2.0 * p0[k] - 5.0 * p1[k] + 4.0 * p2[k] - p3[k]) * t2
                        + (-p0[k] + 3.0 * p1[k] - 3.0 * p2[k] + p3[k]) * t3);
            }
            out.push(pt);
        }
    }
    out
}

fn to_truck(ml: &MultiLeader, document: &acadrust::CadDocument) -> Option<TruckEntity> {
    let nan = [f64::NAN; 3];
    let p3 = |v: &acadrust::types::Vector3| -> [f64; 3] { [v.x, v.y, v.z] };

    let arrow_size = ml.arrowhead_size;
    let draw_arrow = arrow_size > 0.0;
    let invisible = ml.path_type == MultiLeaderPathType::Invisible;

    let mut points: Vec<[f64; 3]> = Vec::new();
    let mut tangents: Vec<TangentGeom> = Vec::new();
    let mut key_verts: Vec<[f64; 3]> = Vec::new();
    let mut snap_pts: Vec<(Vec3, SnapHint)> = Vec::new();
    let mut first = true;

    // snap_pts uses f32 (UI-only); cast at construction.
    let node = |arr: [f64; 3]| {
        (
            Vec3::new(arr[0] as f32, arr[1] as f32, arr[2] as f32),
            SnapHint::Node,
        )
    };

    // Text-side geometry, recomputed every frame so dragging the arrow or the
    // text re-mirrors the whole layout. The text block is centred on its grip
    // (text_location); the landing meets the text's near edge and the leader
    // line ends one dogleg before that.
    let text_loc = ml.context.text_location;
    let leader_ref = ml
        .context
        .leader_roots
        .first()
        .and_then(|r| r.lines.first())
        .and_then(|l| l.points.last())
        .copied()
        .or_else(|| ml.context.leader_roots.first().map(|r| r.connection_point))
        .unwrap_or(text_loc);
    let text_sign: f64 = if text_loc.x >= leader_ref.x {
        1.0
    } else {
        -1.0
    };

    // Lay the text out once, up front: its width fixes where the landing meets
    // the text. The same layout is reused below to emit the glyph strokes.
    let text_layout =
        if ml.content_type == LeaderContentType::MText && !ml.context.text_string.is_empty() {
            let ctx = &ml.context;
            let height = if ctx.text_height > 0.0 {
                ctx.text_height as f32
            } else {
                ml.text_height as f32 * ml.scale_factor as f32
            };
            let td = ctx.text_direction;
            let mut rot = if td.x.abs() > 1e-9 || td.y.abs() > 1e-9 {
                (td.y as f32).atan2(td.x as f32)
            } else {
                ctx.text_rotation as f32
            };
            let style_name = ctx
                .text_style_handle
                .as_ref()
                .and_then(|h| {
                    document
                        .text_styles
                        .iter()
                        .find(|s| s.handle == *h)
                        .map(|s| s.name.clone())
                })
                .unwrap_or_else(|| "STANDARD".to_string());
            let resolved = resolve_text_style(&style_name, document);
            if resolved.is_upside_down {
                rot += std::f32::consts::PI;
            }
            Some(layout_mtext(&MTextRenderOpts {
                value: &ctx.text_string,
                insertion: [text_loc.x, text_loc.y, text_loc.z],
                height,
                rect_w: ctx.text_width as f32,
                rotation: rot,
                style: &resolved,
                // Side-anchored on the leader-facing edge so the text reads
                // outward; flips live with the leader/text side.
                attach_h_anchor: if text_sign >= 0.0 { 0.0 } else { 1.0 },
                v_anchor: mleader_v_anchor(ctx.text_left_attachment),
                line_spacing_factor: ctx.line_spacing_factor as f32,
                vertical_text: false,
                want_glyph_boxes: false,
            }))
        } else {
            None
        };
    let dogleg = if ml.enable_landing && ml.enable_dogleg {
        ml.dogleg_length.max(0.0)
    } else {
        0.0
    };
    // text_location is the leader-facing (near) edge; the landing runs one
    // dogleg from there back toward the leader.
    let landing_pt = [text_loc.x - text_sign * dogleg, text_loc.y, text_loc.z];

    for root in &ml.context.leader_roots {
        let cp = &root.connection_point;
        let cp_f = p3(cp);
        snap_pts.push(node(cp_f));

        for line in &root.lines {
            if line.points.is_empty() {
                continue;
            }

            if !invisible {
                if !first {
                    points.push(nan);
                }
                first = false;

                // Build the full control-point list: line.points + landing point
                let mut ctrl: Vec<[f64; 3]> = line.points.iter().map(|p| p3(p)).collect();
                let last_f = *ctrl.last().unwrap_or(&landing_pt);
                let dist = ((last_f[0] - landing_pt[0]).powi(2)
                    + (last_f[1] - landing_pt[1]).powi(2))
                .sqrt();
                if dist > 1e-9 {
                    ctrl.push(landing_pt);
                }
                for &c in &ctrl {
                    key_verts.push(c);
                    snap_pts.push(node(c));
                }

                if ml.path_type == MultiLeaderPathType::Spline && ctrl.len() >= 2 {
                    // Catmull-Rom spline through the bend points.
                    let pts = catmull_rom_pts(&ctrl, 8);
                    for &pt in &pts {
                        points.push(pt);
                    }
                } else {
                    for &c in &ctrl {
                        points.push(c);
                    }
                }

                for i in 0..ctrl.len().saturating_sub(1) {
                    let a = ctrl[i];
                    let b = ctrl[i + 1];
                    tangents.push(TangentGeom::Line {
                        p1: [a[0] as f32, a[1] as f32, a[2] as f32],
                        p2: [b[0] as f32, b[1] as f32, b[2] as f32],
                    });
                }
            }

            // Arrowhead
            if draw_arrow {
                let tip = &line.points[0];
                let tip_f = p3(tip);
                let next = if line.points.len() >= 2 {
                    line.points[1]
                } else {
                    *cp
                };
                let dx = next.x - tip.x;
                let dy = next.y - tip.y;
                let dl = (dx * dx + dy * dy).sqrt().max(1e-9);
                let (dx, dy) = (dx / dl, dy / dl);
                let a = std::f64::consts::PI / 6.0;
                let (s, c) = a.sin_cos();
                points.push(nan);
                points.push([
                    tip_f[0] + (dx * c - dy * s) * arrow_size,
                    tip_f[1] + (dx * s + dy * c) * arrow_size,
                    tip_f[2],
                ]);
                points.push(tip_f);
                points.push([
                    tip_f[0] + (dx * c + dy * s) * arrow_size,
                    tip_f[1] + (-dx * s + dy * c) * arrow_size,
                    tip_f[2],
                ]);
            }
        }

        // Horizontal landing from the leader end to the text's near edge.
        if dogleg > 0.0 {
            points.push(nan);
            points.push(landing_pt);
            points.push([text_loc.x, text_loc.y, text_loc.z]);
        }
    }

    // Text strokes, drawn from the layout computed up front (centred on the
    // text grip). The snap node is the grip itself.
    if let Some(layout) = &text_layout {
        snap_pts.push(node([text_loc.x, text_loc.y, text_loc.z]));
        for ts in &layout.strokes {
            let ox = ts.origin[0];
            let oy = ts.origin[1];
            for stroke in &ts.strokes {
                if stroke.len() < 2 {
                    continue;
                }
                points.push(nan);
                for &[x, y] in stroke {
                    points.push([ox + x as f64, oy + y as f64, text_loc.z]);
                }
            }
        }
    }

    if points.is_empty() {
        return None;
    }

    Some(TruckEntity {
        object: TruckObject::Lines(points),
        snap_pts,
        tangent_geoms: tangents,
        key_vertices: key_verts,
        fill_tris: vec![],
    })
}

// ── Grips ──────────────────────────────────────────────────────────────────
//
// IDs are assigned in two passes:
//   0 .. total_line_pts - 1  : square grips on every LeaderLine vertex
//   total_line_pts            : diamond grip on context.text_location (if MText)

/// Side the text reads toward (+1 right / -1 left) and the text box's far
/// edge (the wrap-width grip position). The box width is the explicit wrap
/// width when set, else the natural laid-out width.
fn text_box_geom(ml: &MultiLeader) -> (f64, [f64; 3]) {
    let tl = ml.context.text_location;
    let leader_ref = ml
        .context
        .leader_roots
        .first()
        .and_then(|r| r.lines.first())
        .and_then(|l| l.points.last())
        .copied()
        .or_else(|| ml.context.leader_roots.first().map(|r| r.connection_point))
        .unwrap_or(tl);
    let sign = if tl.x >= leader_ref.x { 1.0 } else { -1.0 };
    let height = if ml.context.text_height > 0.0 {
        ml.context.text_height as f32
    } else {
        ml.text_height as f32 * ml.scale_factor as f32
    };
    let style = ResolvedTextStyle {
        font_name: "STANDARD".to_string(),
        width_factor: 1.0,
        oblique_angle: 0.0,
        is_backward: false,
        is_upside_down: false,
    };
    let layout = layout_mtext(&MTextRenderOpts {
        value: &ml.context.text_string,
        insertion: [0.0, 0.0, 0.0],
        height,
        rect_w: ml.context.text_width as f32,
        rotation: 0.0,
        style: &style,
        attach_h_anchor: if sign >= 0.0 { 0.0 } else { 1.0 },
        v_anchor: MTextVAnchor::Middle,
        line_spacing_factor: ml.context.line_spacing_factor as f32,
        vertical_text: false,
        want_glyph_boxes: false,
    });
    let nat_w = layout.line_widths.iter().cloned().fold(0.0_f32, f32::max) as f64;
    let box_w = if ml.context.text_width > 1e-6 {
        ml.context.text_width
    } else {
        nat_w.max(height as f64)
    };
    (sign, [tl.x + sign * box_w, tl.y, tl.z])
}

fn grips(ml: &MultiLeader) -> Vec<GripDef> {
    let mut result: Vec<GripDef> = Vec::new();
    let mut id = 0usize;

    for root in &ml.context.leader_roots {
        for line in &root.lines {
            for p in &line.points {
                result.push(square_grip(id, glam::DVec3::new(p.x, p.y, p.z)));
                id += 1;
            }
        }
    }

    if ml.content_type == LeaderContentType::MText {
        let tl = &ml.context.text_location;
        // Text-location grip, then the wrap-width grip at the box's far edge.
        result.push(center_grip(id, glam::DVec3::new(tl.x, tl.y, tl.z)));
        id += 1;
        let (_, far) = text_box_geom(ml);
        result.push(triangle_grip(id, glam::DVec3::new(far[0], far[1], far[2])));
    }

    result
}

/// Sentinel grip id meaning "translate the whole multileader" — used by the
/// text grip's "Move with Leader" action so the leader follows the text.
pub(crate) const MOVE_ALL_GRIP: usize = usize::MAX;

fn apply_grip(ml: &mut MultiLeader, grip_id: usize, apply: GripApply) {
    if grip_id == MOVE_ALL_GRIP {
        let (dx, dy, dz) = match apply {
            GripApply::Translate(d) => (d.x as f64, d.y as f64, d.z as f64),
            GripApply::Absolute(a) => (
                a.x as f64 - ml.context.text_location.x,
                a.y as f64 - ml.context.text_location.y,
                a.z as f64 - ml.context.text_location.z,
            ),
        };
        for root in &mut ml.context.leader_roots {
            for line in &mut root.lines {
                for p in &mut line.points {
                    p.x += dx;
                    p.y += dy;
                    p.z += dz;
                }
            }
            root.connection_point.x += dx;
            root.connection_point.y += dy;
            root.connection_point.z += dz;
        }
        ml.context.text_location.x += dx;
        ml.context.text_location.y += dy;
        ml.context.text_location.z += dz;
        return;
    }

    let mut idx = 0usize;

    for root in &mut ml.context.leader_roots {
        for line in &mut root.lines {
            for p in &mut line.points {
                if idx == grip_id {
                    match apply {
                        GripApply::Absolute(a) => {
                            p.x = a.x as f64;
                            p.y = a.y as f64;
                            p.z = a.z as f64;
                        }
                        GripApply::Translate(d) => {
                            p.x += d.x as f64;
                            p.y += d.y as f64;
                            p.z += d.z as f64;
                        }
                    }
                    return;
                }
                idx += 1;
            }
        }
    }

    // Text-location grip (idx == n_vertices), then the wrap-width grip.
    if ml.content_type == LeaderContentType::MText {
        if idx == grip_id {
            let tl = &mut ml.context.text_location;
            match apply {
                GripApply::Absolute(a) => {
                    tl.x = a.x as f64;
                    tl.y = a.y as f64;
                    tl.z = a.z as f64;
                }
                GripApply::Translate(d) => {
                    tl.x += d.x as f64;
                    tl.y += d.y as f64;
                    tl.z += d.z as f64;
                }
            }
            return;
        }
        idx += 1;
        if idx == grip_id {
            // Dragging the box edge sets the MText wrap width.
            let (sign, far) = text_box_geom(ml);
            let new_far_x = match apply {
                GripApply::Absolute(a) => a.x as f64,
                GripApply::Translate(d) => far[0] + d.x as f64,
            };
            let min_w = ml.text_height.max(1.0) * 0.5;
            ml.context.text_width = (sign * (new_far_x - ml.context.text_location.x)).max(min_w);
        }
    }
}

// ── Properties ─────────────────────────────────────────────────────────────

fn content_type_str(ct: &LeaderContentType) -> &'static str {
    match ct {
        LeaderContentType::None => "None",
        LeaderContentType::Block => "Block",
        LeaderContentType::MText => "MText",
        LeaderContentType::Tolerance => "Tolerance",
    }
}

fn path_type_str(pt: &MultiLeaderPathType) -> &'static str {
    match pt {
        MultiLeaderPathType::Invisible => "Invisible",
        MultiLeaderPathType::StraightLineSegments => "Straight",
        MultiLeaderPathType::Spline => "Spline",
    }
}

fn attachment_str(a: &TextAttachmentType) -> &'static str {
    match a {
        TextAttachmentType::TopOfTopLine => "Top of Top",
        TextAttachmentType::MiddleOfTopLine => "Mid of Top",
        TextAttachmentType::MiddleOfText => "Mid of Text",
        TextAttachmentType::MiddleOfBottomLine => "Mid of Bot",
        TextAttachmentType::BottomOfBottomLine => "Bot of Bot",
        TextAttachmentType::BottomLine => "Bottom Line",
        _ => "Other",
    }
}

fn bool_toggle(label: &str, field: &'static str, value: bool) -> Property {
    Property {
        label: label.into(),
        field,
        value: PropValue::BoolToggle { field, value },
    }
}

fn choice(label: &str, field: &'static str, selected: &str, opts: &[&str]) -> Property {
    Property {
        label: label.into(),
        field,
        value: PropValue::Choice {
            selected: selected.to_string(),
            options: opts.iter().map(|s| s.to_string()).collect(),
        },
    }
}

fn properties(ml: &MultiLeader) -> PropSection {
    let ctx = &ml.context;
    let total_pts: usize = ctx
        .leader_roots
        .iter()
        .flat_map(|r| r.lines.iter())
        .map(|l| l.points.len())
        .sum();

    let mut props = vec![
        // Content
        choice(
            "Content Type",
            "content_type",
            content_type_str(&ml.content_type),
            &["None", "MText", "Block", "Tolerance"],
        ),
        Property {
            label: "Text".into(),
            field: "text_string",
            value: PropValue::EditText(ctx.text_string.clone()),
        },
        edit("Text Height", "text_height", ml.text_height),
        edit("Text X", "text_x", ctx.text_location.x),
        edit("Text Y", "text_y", ctx.text_location.y),
        edit("Text Z", "text_z", ctx.text_location.z),
        bool_toggle("Text Frame", "text_frame", ml.text_frame),
        // Leader line
        choice(
            "Path Type",
            "path_type",
            path_type_str(&ml.path_type),
            &["Straight", "Spline", "Invisible"],
        ),
        bool_toggle("Landing", "enable_landing", ml.enable_landing),
        bool_toggle("Dogleg", "enable_dogleg", ml.enable_dogleg),
        edit("Dogleg Length", "dogleg_length", ml.dogleg_length),
        edit("Arrow Size", "arrowhead_size", ml.arrowhead_size),
        edit("Scale", "scale_factor", ml.scale_factor),
        bool_toggle(
            "Annotation Scale",
            "enable_annotation_scale",
            ml.enable_annotation_scale,
        ),
        // Text attachment
        choice(
            "Left Attach",
            "text_left_attachment",
            attachment_str(&ml.text_left_attachment),
            &[
                "Top of Top",
                "Mid of Top",
                "Mid of Text",
                "Mid of Bot",
                "Bot of Bot",
                "Bottom Line",
            ],
        ),
        choice(
            "Right Attach",
            "text_right_attachment",
            attachment_str(&ml.text_right_attachment),
            &[
                "Top of Top",
                "Mid of Top",
                "Mid of Text",
                "Mid of Bot",
                "Bot of Bot",
                "Bottom Line",
            ],
        ),
        // Top / Bottom attach (used when text_attachment_direction = Vertical)
        choice(
            "Top Attach",
            "text_top_attachment",
            attachment_str(&ml.text_top_attachment),
            &[
                "Top of Top",
                "Mid of Top",
                "Mid of Text",
                "Mid of Bot",
                "Bot of Bot",
                "Bottom Line",
            ],
        ),
        choice(
            "Bottom Attach",
            "text_bottom_attachment",
            attachment_str(&ml.text_bottom_attachment),
            &[
                "Top of Top",
                "Mid of Top",
                "Mid of Text",
                "Mid of Bot",
                "Bot of Bot",
                "Bottom Line",
            ],
        ),
        // Stats
        ro("Leader Pts", "total_pts", total_pts.to_string()),
        ro("Roots", "root_count", ctx.leader_roots.len().to_string()),
        // Style references / handles (read-only — the multileader's own
        // copies of the style values are the authoritative render inputs).
        ro(
            "Style Handle",
            "style_handle",
            match ml.style_handle {
                Some(h) if !h.is_null() => format!("{:X}", h.value()),
                _ => "(none)".to_string(),
            },
        ),
        ro(
            "Text Style Handle",
            "text_style_handle",
            match ml.text_style_handle {
                Some(h) if !h.is_null() => format!("{:X}", h.value()),
                _ => "(none)".to_string(),
            },
        ),
        ro(
            "Arrow Handle",
            "arrowhead_handle",
            match ml.arrowhead_handle {
                Some(h) if !h.is_null() => format!("{:X}", h.value()),
                _ => "(none)".to_string(),
            },
        ),
        ro(
            "Line Type Handle",
            "line_type_handle",
            match ml.line_type_handle {
                Some(h) if !h.is_null() => format!("{:X}", h.value()),
                _ => "(none)".to_string(),
            },
        ),
        ro(
            "Block Content Handle",
            "block_content_handle",
            match ml.block_content_handle {
                Some(h) if !h.is_null() => format!("{:X}", h.value()),
                _ => "(none)".to_string(),
            },
        ),
        // Less common toggles surfaced read-only.
        ro(
            "Extend Leader",
            "extend_leader_to_text",
            if ml.extend_leader_to_text {
                "Yes"
            } else {
                "No"
            },
        ),
        ro(
            "Text Direction Negative",
            "text_direction_negative",
            if ml.text_direction_negative {
                "Yes"
            } else {
                "No"
            },
        ),
        ro(
            "Text Align In IPE",
            "text_align_in_ipe",
            ml.text_align_in_ipe.to_string(),
        ),
        ro(
            "Property Override Flags",
            "property_override_flags",
            format!("{:#018b}", ml.property_override_flags.bits()),
        ),
        ro(
            "Block Scale",
            "block_scale",
            format!(
                "{:.3} × {:.3} × {:.3}",
                ml.block_scale.x, ml.block_scale.y, ml.block_scale.z
            ),
        ),
        ro(
            "Block Rotation",
            "block_rotation",
            format!("{:.3}°", ml.block_rotation.to_degrees()),
        ),
    ];

    // Connection point for first root (most common case)
    if let Some(root) = ctx.leader_roots.first() {
        props.push(edit("Root Conn X", "conn_x", root.connection_point.x));
        props.push(edit("Root Conn Y", "conn_y", root.connection_point.y));
        props.push(edit("Root Conn Z", "conn_z", root.connection_point.z));
    }

    PropSection {
        title: "Geometry".into(),
        props,
    }
}

fn apply_geom_prop(ml: &mut MultiLeader, field: &str, value: &str) {
    let f64 = |s: &str| -> Option<f64> { s.trim().parse().ok() };

    match field {
        "content_type" => {
            ml.content_type = match value {
                "Block" => LeaderContentType::Block,
                "MText" => LeaderContentType::MText,
                "Tolerance" => LeaderContentType::Tolerance,
                _ => LeaderContentType::None,
            };
        }
        "text_string" => ml.context.text_string = value.to_string(),
        "text_height" => {
            if let Some(v) = f64(value) {
                ml.text_height = v;
                ml.context.text_height = v;
            }
        }
        "text_x" => {
            if let Some(v) = f64(value) {
                ml.context.text_location.x = v;
            }
        }
        "text_y" => {
            if let Some(v) = f64(value) {
                ml.context.text_location.y = v;
            }
        }
        "text_z" => {
            if let Some(v) = f64(value) {
                ml.context.text_location.z = v;
            }
        }
        "text_frame" => {
            ml.text_frame = if value == "toggle" {
                !ml.text_frame
            } else {
                value == "true"
            }
        }
        "path_type" => {
            ml.path_type = match value {
                "Spline" => MultiLeaderPathType::Spline,
                "Invisible" => MultiLeaderPathType::Invisible,
                _ => MultiLeaderPathType::StraightLineSegments,
            };
        }
        "enable_landing" => {
            ml.enable_landing = if value == "toggle" {
                !ml.enable_landing
            } else {
                value == "true"
            }
        }
        "enable_dogleg" => {
            ml.enable_dogleg = if value == "toggle" {
                !ml.enable_dogleg
            } else {
                value == "true"
            }
        }
        "enable_annotation_scale" => {
            ml.enable_annotation_scale = if value == "toggle" {
                !ml.enable_annotation_scale
            } else {
                value == "true"
            }
        }
        "dogleg_length" => {
            if let Some(v) = f64(value) {
                ml.dogleg_length = v;
            }
        }
        "arrowhead_size" => {
            if let Some(v) = f64(value) {
                ml.arrowhead_size = v;
            }
        }
        "scale_factor" => {
            if let Some(v) = f64(value) {
                ml.scale_factor = v;
            }
        }
        "conn_x" => {
            if let (Some(v), Some(root)) = (f64(value), ml.context.leader_roots.first_mut()) {
                root.connection_point.x = v;
            }
        }
        "conn_y" => {
            if let (Some(v), Some(root)) = (f64(value), ml.context.leader_roots.first_mut()) {
                root.connection_point.y = v;
            }
        }
        "conn_z" => {
            if let (Some(v), Some(root)) = (f64(value), ml.context.leader_roots.first_mut()) {
                root.connection_point.z = v;
            }
        }
        "text_left_attachment" => {
            ml.text_left_attachment = parse_attachment(value);
            ml.context.text_left_attachment = parse_attachment(value);
        }
        "text_right_attachment" => {
            ml.text_right_attachment = parse_attachment(value);
            ml.context.text_right_attachment = parse_attachment(value);
        }
        "text_top_attachment" => {
            ml.text_top_attachment = parse_attachment(value);
            ml.context.text_top_attachment = parse_attachment(value);
        }
        "text_bottom_attachment" => {
            ml.text_bottom_attachment = parse_attachment(value);
            ml.context.text_bottom_attachment = parse_attachment(value);
        }
        _ => {}
    }
}

fn parse_attachment(s: &str) -> TextAttachmentType {
    match s {
        "Top of Top" => TextAttachmentType::TopOfTopLine,
        "Mid of Top" => TextAttachmentType::MiddleOfTopLine,
        "Mid of Bot" => TextAttachmentType::MiddleOfBottomLine,
        "Bot of Bot" => TextAttachmentType::BottomOfBottomLine,
        "Bottom Line" => TextAttachmentType::BottomLine,
        _ => TextAttachmentType::MiddleOfText,
    }
}

// ── Transform ──────────────────────────────────────────────────────────────

fn apply_transform(ml: &mut MultiLeader, t: &EntityTransform) {
    crate::scene::view::transform::apply_standard_entity_transform(ml, t, |entity, p1, p2| {
        // Reflect every point on the leader (line points, connection points,
        // break-point endpoints) AND every direction vector that drives the
        // text orientation. Without the direction reflection text would keep
        // its original rotation while the leader appears mirrored.
        for root in &mut entity.context.leader_roots {
            for line in &mut root.lines {
                for p in &mut line.points {
                    crate::scene::view::transform::reflect_xy_point(&mut p.x, &mut p.y, p1, p2);
                }
                for bp in &mut line.break_points {
                    crate::scene::view::transform::reflect_xy_point(
                        &mut bp.start_point.x,
                        &mut bp.start_point.y,
                        p1,
                        p2,
                    );
                    crate::scene::view::transform::reflect_xy_point(
                        &mut bp.end_point.x,
                        &mut bp.end_point.y,
                        p1,
                        p2,
                    );
                }
            }
            crate::scene::view::transform::reflect_xy_point(
                &mut root.connection_point.x,
                &mut root.connection_point.y,
                p1,
                p2,
            );
            reflect_xy_direction(&mut root.direction.x, &mut root.direction.y, p1, p2);
            for bp in &mut root.break_points {
                crate::scene::view::transform::reflect_xy_point(
                    &mut bp.start_point.x,
                    &mut bp.start_point.y,
                    p1,
                    p2,
                );
                crate::scene::view::transform::reflect_xy_point(
                    &mut bp.end_point.x,
                    &mut bp.end_point.y,
                    p1,
                    p2,
                );
            }
        }
        crate::scene::view::transform::reflect_xy_point(
            &mut entity.context.text_location.x,
            &mut entity.context.text_location.y,
            p1,
            p2,
        );
        reflect_xy_direction(
            &mut entity.context.text_direction.x,
            &mut entity.context.text_direction.y,
            p1,
            p2,
        );
        reflect_xy_direction(
            &mut entity.context.base_direction.x,
            &mut entity.context.base_direction.y,
            p1,
            p2,
        );
    });
}

/// Reflect a direction (not position) vector across the mirror line p1→p2.
/// Reflecting a direction is the same as reflecting `p1 + dir` then subtracting
/// the reflection of `p1`, which simplifies to reflecting around the origin.
fn reflect_xy_direction(dx: &mut f64, dy: &mut f64, p1: Vec3, p2: Vec3) {
    let zero = Vec3::ZERO;
    let p2_rel = Vec3::new(p2.x - p1.x, p2.y - p1.y, 0.0);
    let mut tip_x = *dx;
    let mut tip_y = *dy;
    crate::scene::view::transform::reflect_xy_point(&mut tip_x, &mut tip_y, zero, p2_rel);
    *dx = tip_x;
    *dy = tip_y;
}

// ── Trait impls ────────────────────────────────────────────────────────────

impl TruckConvertible for MultiLeader {
    fn to_truck(&self, document: &acadrust::CadDocument) -> Option<TruckEntity> {
        to_truck(self, document)
    }
}

impl crate::entities::traits::Grippable for MultiLeader {
    fn grips(&self) -> Vec<GripDef> {
        grips(self)
    }
    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        apply_grip(self, grip_id, apply);
    }
    fn grip_menu(&self, grip_id: usize) -> Vec<crate::scene::model::object::GripMenuItem> {
        use crate::scene::model::object::{GripMenuAction, GripMenuItem};
        let n_vertices: usize = self
            .context
            .leader_roots
            .iter()
            .flat_map(|r| r.lines.iter())
            .map(|l| l.points.len())
            .sum();
        if self.content_type == LeaderContentType::MText && grip_id >= n_vertices {
            if grip_id == n_vertices {
                // Text-location grip.
                vec![
                    GripMenuItem {
                        label: "Stretch",
                        action: GripMenuAction::Stretch,
                    },
                    GripMenuItem {
                        label: "Move with Leader",
                        action: GripMenuAction::MoveWithLeader,
                    },
                    GripMenuItem {
                        label: "Move Independent",
                        action: GripMenuAction::MoveIndependent,
                    },
                ]
            } else {
                // Wrap-width grip: drag only.
                Vec::new()
            }
        } else {
            // Leader-line vertex.
            vec![
                GripMenuItem {
                    label: "Stretch",
                    action: GripMenuAction::Stretch,
                },
                GripMenuItem {
                    label: "Add Leader",
                    action: GripMenuAction::AddLeader,
                },
                GripMenuItem {
                    label: "Remove Leader",
                    action: GripMenuAction::RemoveLeader,
                },
            ]
        }
    }
    fn apply_grip_menu(&mut self, grip_id: usize, action: crate::scene::model::object::GripMenuAction) {
        use crate::scene::model::object::GripMenuAction as A;
        // Locate the (root, line) and vertex position owning this grip id.
        let mut idx = 0usize;
        let mut loc: Option<(usize, usize, acadrust::types::Vector3)> = None;
        'find: for (ri, root) in self.context.leader_roots.iter().enumerate() {
            for (li, line) in root.lines.iter().enumerate() {
                let n = line.points.len();
                if grip_id < idx + n {
                    loc = Some((ri, li, line.points[grip_id - idx]));
                    break 'find;
                }
                idx += n;
            }
        }
        let Some((ri, li, vpos)) = loc else { return };
        match action {
            A::RemoveLeader => {
                let total: usize = self
                    .context
                    .leader_roots
                    .iter()
                    .map(|r| r.lines.len())
                    .sum();
                if total > 1 {
                    self.context.leader_roots[ri].lines.remove(li);
                    if self.context.leader_roots[ri].lines.is_empty()
                        && self.context.leader_roots.len() > 1
                    {
                        self.context.leader_roots.remove(ri);
                    }
                }
            }
            A::AddLeader => {
                // Append to the last root so the new arrow is the last grip id
                // (so the caller can immediately grab it for placement). Seed it
                // below the picked vertex; the user drags it to the final spot.
                let _ = ri;
                let off = self.text_height.max(1.0) * 4.0;
                let arrow = acadrust::types::Vector3::new(vpos.x, vpos.y - off, vpos.z);
                if let Some(root) = self.context.leader_roots.last_mut() {
                    root.create_line(vec![arrow]);
                }
            }
            _ => {}
        }
    }
}

impl crate::entities::traits::PropertyEditable for MultiLeader {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        properties(self)
    }
    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        apply_geom_prop(self, field, value);
    }
}

impl crate::entities::traits::Transformable for MultiLeader {
    fn apply_transform(&mut self, t: &EntityTransform) {
        apply_transform(self, t);
    }
}

/// Per-entity tessellation entry for `MultiLeader`. Returns multiple
/// `WireModel`s (leader/dogleg + arrow fill, optional block content,
/// text strokes, frame, background fill) so each piece can carry its
/// own colour.
pub trait MultiLeaderTess {
    fn tessellate(
        &self,
        document: &acadrust::CadDocument,
        handle: acadrust::Handle,
        selected: bool,
        entity_color: [f32; 4],
        line_weight_px: f32,
        world_offset: [f64; 3],
        anno_scale: f32,
        world_per_pixel: Option<f32>,
    ) -> Vec<crate::scene::model::wire_model::WireModel>;
}

impl MultiLeaderTess for MultiLeader {
    fn tessellate(
        &self,
        document: &acadrust::CadDocument,
        handle: acadrust::Handle,
        selected: bool,
        entity_color: [f32; 4],
        line_weight_px: f32,
        world_offset: [f64; 3],
        anno_scale: f32,
        world_per_pixel: Option<f32>,
    ) -> Vec<crate::scene::model::wire_model::WireModel> {
        use crate::scene::convert::tessellate::{
            append_arrow, arrow_from_block, color_or_inherit, tessellate, ArrowKind, DimGeom,
        };
        use crate::scene::model::wire_model::{SnapHint, TangentGeom, WireModel};
        use glam::Vec3;
        let ml = self;
        // line_color falls back to the entity colour when the MultiLeader's own
        // colour is ByBlock/ByLayer; otherwise the leader uses its stored hue.
        let line_color = if selected {
            WireModel::SELECTED
        } else {
            color_or_inherit(&ml.line_color, entity_color)
        };
        // ml.line_weight is the leader-line weight override. Negative codes
        // (ByLayer/ByBlock/Default) fall through to the entity's already-resolved
        // pixel width.
        let leader_lw_px = match ml.line_weight {
            acadrust::types::LineWeight::Value(v) if v >= 0 => (v as f32 / 100.0) * (96.0 / 25.4),
            _ => line_weight_px,
        };
        // ml.line_type_handle — resolve via line_types table by handle and apply
        // the resulting dash pattern to the leader wire.
        let lt_scale = document.header.linetype_scale as f32 * ml.common.linetype_scale as f32;
        let (leader_pat_len, leader_pat) = match ml.line_type_handle {
            Some(h) if !h.is_null() => {
                let name = document
                    .line_types
                    .iter()
                    .find(|lt| lt.handle == h)
                    .map(|lt| lt.name.clone());
                match name {
                    Some(n) => {
                        crate::scene::view::render::resolve_pattern(&document.line_types, &n, lt_scale)
                    }
                    None => (0.0, [0.0; 8]),
                }
            }
            _ => (0.0, [0.0; 8]),
        };

        let name = handle.value().to_string();
        let nan = [f32::NAN; 3];
        let [ox, oy, oz] = world_offset;
        let p3 = |v: &acadrust::types::Vector3| -> [f32; 3] {
            [(v.x - ox) as f32, (v.y - oy) as f32, (v.z - oz) as f32]
        };

        // ── Scaling ──────────────────────────────────────────────────────────────
        // ml.scale_factor is always applied; anno_scale is only applied when the
        // multileader is marked annotative.
        let effective_scale = (ml.scale_factor as f32)
            * if ml.enable_annotation_scale {
                anno_scale
            } else {
                1.0
            };

        let arrow_size = ml.arrowhead_size as f32 * effective_scale;
        let draw_arrow = arrow_size > 0.0;
        let invisible = ml.path_type == MultiLeaderPathType::Invisible;
        // arrowhead_handle resolves through the block records to a named arrow
        // block (matches DIMLDRBLK on Dimension). Null handle / unknown name →
        // ClosedFilled triangle.
        let arrow_kind = match ml.arrowhead_handle {
            Some(h) if !h.is_null() => arrow_from_block(document, h, arrow_size.max(0.001)),
            _ => ArrowKind::Triangle {
                size: arrow_size.max(0.001),
                filled: true,
                size_mul: 1.0,
            },
        };

        // ── Leader / arrow / dogleg points ───────────────────────────────────────
        let mut points: Vec<[f32; 3]> = Vec::new();
        let mut key_verts: Vec<[f32; 3]> = Vec::new();
        let mut snap_pts: Vec<(Vec3, SnapHint)> = Vec::new();
        let mut tangents: Vec<TangentGeom> = Vec::new();
        let mut arrow_fill: Vec<[f32; 3]> = Vec::new();
        let mut first = true;

        // Which side the text grip sits on, recomputed every frame so the text
        // alignment and the landing mirror live when the arrow or text moves.
        let text_loc_w = ml.context.text_location;
        let leader_ref_w = ml
            .context
            .leader_roots
            .first()
            .and_then(|r| r.lines.first())
            .and_then(|l| l.points.last())
            .copied()
            .or_else(|| ml.context.leader_roots.first().map(|r| r.connection_point))
            .unwrap_or(text_loc_w);
        let text_sign_w: f64 = if text_loc_w.x >= leader_ref_w.x {
            1.0
        } else {
            -1.0
        };

        for root in &ml.context.leader_roots {
            let cp = &root.connection_point;
            let cp_f = p3(cp);
            snap_pts.push((Vec3::from(cp_f), SnapHint::Node));

            for line in &root.lines {
                if line.points.is_empty() {
                    continue;
                }

                if !invisible {
                    if !first {
                        points.push(nan);
                    }
                    first = false;

                    let mut ctrl: Vec<[f32; 3]> = line.points.iter().map(|p| p3(p)).collect();
                    let last_f = *ctrl.last().unwrap_or(&cp_f);
                    let dist =
                        ((last_f[0] - cp_f[0]).powi(2) + (last_f[1] - cp_f[1]).powi(2)).sqrt();
                    if dist > 1e-9 {
                        ctrl.push(cp_f);
                    }
                    for &c in &ctrl {
                        key_verts.push(c);
                        snap_pts.push((Vec3::from(c), SnapHint::Node));
                    }

                    if ml.path_type == MultiLeaderPathType::Spline && ctrl.len() >= 2 {
                        let ctrl_f64: Vec<[f64; 3]> = ctrl
                            .iter()
                            .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
                            .collect();
                        for pt in catmull_rom_pts(&ctrl_f64, 8) {
                            points.push([pt[0] as f32, pt[1] as f32, pt[2] as f32]);
                        }
                    } else {
                        for &c in &ctrl {
                            points.push(c);
                        }
                    }
                    for i in 0..ctrl.len().saturating_sub(1) {
                        tangents.push(TangentGeom::Line {
                            p1: ctrl[i],
                            p2: ctrl[i + 1],
                        });
                    }
                }

                if draw_arrow {
                    let tip = &line.points[0];
                    let tip_f = p3(tip);
                    let next = if line.points.len() >= 2 {
                        line.points[1]
                    } else {
                        *cp
                    };
                    let dx = (next.x - tip.x) as f32;
                    let dy = (next.y - tip.y) as f32;
                    let dl = (dx * dx + dy * dy).sqrt().max(1e-9);
                    let dir = Vec3::new(dx / dl, dy / dl, 0.0);
                    let tip_v = Vec3::new(tip_f[0], tip_f[1], tip_f[2]);
                    // Reuse the dim/leader arrow emitter so MultiLeader's arrow
                    // matches the block referenced by arrowhead_handle.
                    let mut arrow_geom = DimGeom::new();
                    append_arrow(&mut arrow_geom, tip_v, dir, &arrow_kind);
                    if !arrow_geom.dim_lines.is_empty() {
                        points.push(nan);
                        points.extend(arrow_geom.dim_lines);
                    }
                    arrow_fill.extend(arrow_geom.arrow_fill);
                }
            }

            if ml.enable_landing && ml.enable_dogleg && ml.dogleg_length > 0.0 {
                // Horizontal landing toward the text's side (mirrors live).
                let d = ml.dogleg_length * effective_scale as f64;
                let landing_end = [
                    (cp.x + text_sign_w * d - ox) as f32,
                    (cp.y - oy) as f32,
                    cp_f[2],
                ];
                points.push(nan);
                points.push(cp_f);
                points.push(landing_end);
                // Continue the landing to the text's near edge so it stays
                // attached as the text moves.
                if ml.content_type == LeaderContentType::MText && !ml.context.text_string.is_empty()
                {
                    let tx = (ml.context.text_location.x - ox) as f32;
                    let ty = (ml.context.text_location.y - oy) as f32;
                    points.push(landing_end);
                    points.push([tx, ty, cp_f[2]]);
                }
            }
        }

        // The leader/arrow/dogleg wire goes out as a single WireModel. Text, frame,
        // and background fill (each with their own color) are appended as separate
        // WireModels so the renderer respects per-piece coloring.
        let mut wires: Vec<WireModel> = Vec::new();
        wires.push(WireModel {
            name: name.clone(),
            points,
            color: line_color,
            selected,
            aci: 0,
            pattern_length: leader_pat_len,
            pattern: leader_pat,
            line_weight_px: leader_lw_px,
            snap_pts,
            tangent_geoms: tangents,
            key_vertices: key_verts,
            aabb: WireModel::UNBOUNDED_AABB,
            plinegen: true,
            vp_scissor: None,
            fill_tris: arrow_fill,
        });

        // ── Block content ───────────────────────────────────────────────────────
        // When content_type == Block, the MultiLeader displays a block reference
        // at block_content_location with the recorded rotation/scale. Synthesize
        // an Insert and explode it through the standard tessellator. The block
        // resolves via block_content_handle (handle → block_record name).
        if ml.content_type == LeaderContentType::Block && ml.context.has_block_contents {
            let block_name = match ml.block_content_handle {
                Some(h) if !h.is_null() => document
                    .block_records
                    .iter()
                    .find(|br| br.handle == h)
                    .map(|br| br.name.clone()),
                _ => None,
            };
            if let Some(block_name) = block_name {
                let block_color = if selected {
                    line_color
                } else {
                    color_or_inherit(&ml.block_content_color, entity_color)
                };
                let mut synth_ins =
                    acadrust::entities::Insert::new(block_name, ml.context.block_content_location);
                synth_ins.set_x_scale(ml.block_scale.x);
                synth_ins.set_y_scale(ml.block_scale.y);
                synth_ins.set_z_scale(ml.block_scale.z);
                synth_ins.rotation = ml.block_rotation;
                synth_ins.common.layer = ml.common.layer.clone();
                // block_connection_type (BlockExtents vs BasePoint) chooses the
                // anchor when *creating* the multileader; at render time the
                // file's stored leader endpoints already encode that choice.
                let _ = ml.block_connection_type;
                for sub in synth_ins.explode_from_document(document) {
                    let normalized =
                        crate::modules::home::modify::explode::normalize_insert_entity(sub);
                    let mut sub_wires = tessellate(
                        document,
                        handle,
                        &normalized,
                        selected,
                        block_color,
                        leader_pat_len,
                        leader_pat,
                        leader_lw_px,
                        world_offset,
                        1.0,
                    );
                    for w in &mut sub_wires {
                        w.name = name.clone();
                    }
                    wires.extend(sub_wires);
                }
                // Block attributes attached to the multileader — render each as
                // its own attribute entity at WCS location like INSERT does.
                for ba in &ml.block_attributes {
                    let _ = ba; // BlockAttribute carries only the value override
                                // string; we'd need the AttributeDefinition handle
                                // to materialise it as ATTRIB geometry. Skipped
                                // until that wiring exists.
                }
            }
        }

        // ── Text strokes / frame / background fill ──────────────────────────────
        // Strip inline format codes, split / word-wrap into lines, then place each
        // line according to text_attachment_point (horizontal) and
        // text_left_attachment (vertical), with text_rotation/text_direction applied.
        if ml.content_type == LeaderContentType::MText && !ml.context.text_string.is_empty() {
            let ctx = &ml.context;
            // `ctx.text_height` (when > 0) is the already-resolved WCS text height
            // stored in the per-instance context — style × scale_factor × anno
            // scale are all already baked in. Multiplying by `effective_scale`
            // again would double-scale (e.g., a context height of 100 in a file
            // with scale_factor=20 was rendering at 2000 units — 20× too big).
            // Only the fallback path (file omits the context value) needs
            // scale_factor + annotation scale applied.
            let height = if ctx.text_height > 0.0 {
                ctx.text_height as f32
            } else {
                ml.text_height as f32 * effective_scale
            };

            let ins = &ctx.text_location;
            // Subtract world_offset in f64 before casting to f32: drawings often
            // sit at large absolute coordinates and casting first then subtracting
            // throws away the precision needed for the rotated sub-glyph offsets.
            let local_ins_x = (ins.x - ox) as f32;
            let local_ins_y = (ins.y - oy) as f32;
            let z = (ins.z - oz) as f32;

            // Rotation: prefer text_direction (transforms survive rotations /
            // mirrors when acadrust updates it) and fall back to text_rotation.
            // ml.text_angle_type then constrains the final angle:
            //   - Horizontal:        always 0
            //   - ParallelToLastLeaderLine: keep stored direction
            //   - Optimized:        clamp to (-π/2, π/2] (flip upside-down)
            // text_direction_negative finally adds π so reading goes the other way.
            let td = ctx.text_direction;
            let mut rot = if td.x.abs() > 1e-9 || td.y.abs() > 1e-9 {
                (td.y as f32).atan2(td.x as f32)
            } else {
                ctx.text_rotation as f32
            };
            match ml.text_angle_type {
                acadrust::entities::multileader::TextAngleType::Horizontal => rot = 0.0,
                acadrust::entities::multileader::TextAngleType::Optimized => {
                    let pi = std::f32::consts::PI;
                    if rot > pi / 2.0 {
                        rot -= pi;
                    } else if rot <= -pi / 2.0 {
                        rot += pi;
                    }
                }
                acadrust::entities::multileader::TextAngleType::ParallelToLastLeaderLine => {}
            }
            if ml.text_direction_negative {
                rot += std::f32::consts::PI;
            }

            // Resolve text style via handle when available, falling back to STANDARD.
            let style_name = ctx
                .text_style_handle
                .as_ref()
                .and_then(|h| {
                    document
                        .text_styles
                        .iter()
                        .find(|s| s.handle == *h)
                        .map(|s| s.name.clone())
                })
                .unwrap_or_else(|| "STANDARD".to_string());
            let style = resolve_text_style(&style_name, document);
            let mut rot = rot;
            if style.is_upside_down {
                rot += std::f32::consts::PI;
            }
            let (cos_r, sin_r) = (rot.cos(), rot.sin());

            // Side-anchored on the leader-facing edge so the text reads
            // outward; flips live with the leader/text side.
            use acadrust::entities::multileader::TextAttachmentDirectionType;
            let h_anchor: f32 = if text_sign_w >= 0.0 { 0.0 } else { 1.0 };
            // Pick the vertical-anchor attachment based on text_attachment_direction:
            //   Horizontal — leader attaches left/right; use ml.text_left_attachment
            //                (matches the file's stored ctx.text_left_attachment).
            //   Vertical   — leader attaches top/bottom; use ml.text_top_attachment
            //                or ml.text_bottom_attachment depending on which side
            //                the leader is coming from (chosen via the first
            //                root.direction.y sign).
            let leader_from_top = ml
                .context
                .leader_roots
                .first()
                .map(|r| r.direction.y < 0.0)
                .unwrap_or(true);
            let vertical_attach = match ml.text_attachment_direction {
                TextAttachmentDirectionType::Vertical => {
                    if leader_from_top {
                        ml.text_top_attachment
                    } else {
                        ml.text_bottom_attachment
                    }
                }
                TextAttachmentDirectionType::Horizontal => ctx.text_left_attachment,
            };
            let v_anchor = mleader_v_anchor(vertical_attach);

            // Shared MText pipeline — every inline format code (`\f`, `\C`/`\c`,
            // `\H`, `\W`, `\Q`, `\T`, `\A`, `\p…`, decorations, stacked
            // fractions, …) reaches the stroke output. Stroke origins are
            // already in offset-relative space because we pass local_ins_x/y.
            let layout = layout_mtext(&MTextRenderOpts {
                value: &ctx.text_string,
                insertion: [local_ins_x as f64, local_ins_y as f64, z as f64],
                height,
                rect_w: ctx.text_width as f32,
                rotation: rot,
                style: &style,
                attach_h_anchor: h_anchor,
                v_anchor,
                line_spacing_factor: ctx.line_spacing_factor as f32,
                vertical_text: false,
                want_glyph_boxes: false,
            });
            let line_widths = &layout.line_widths;
            let max_line_w = line_widths.iter().cloned().fold(0.0_f32, f32::max);
            let line_h = layout.line_height;
            let v_offset = layout.v_offset;
            let n_lines = layout.line_count.max(1) as f32;

            // Resolve text color (falls back to entity color for ByLayer / ByBlock).
            let text_color = if selected {
                line_color
            } else {
                color_or_inherit(&ctx.text_color, entity_color)
            };

            // Same LOD ladder used for top-level Text / MText (see scene/mod.rs):
            //   h_px < 1   → baseline line (skip glyphs)
            //   1 ≤ h < 5  → greeked rect in text color (skip glyphs)
            //   h_px ≥ 5   → full per-glyph stroke tessellation
            let lod_h_px = world_per_pixel.map(|wpp| height / wpp);
            let lod_mode = match lod_h_px {
                Some(h) if h < 1.0 => 0,
                Some(h) if h < 5.0 => 1,
                _ => 2,
            };

            // Helper: map a (local_x, local_y) in the text's pre-rotation frame
            // (origin at the insertion point) into WCS render space.
            let to_wcs = |lx: f32, ly: f32| -> [f32; 3] {
                [
                    local_ins_x + lx * cos_r - ly * sin_r,
                    local_ins_y + lx * sin_r + ly * cos_r,
                    z,
                ]
            };

            if lod_mode == 0 {
                // Baseline of the top line only.
                let line_w = line_widths.first().copied().unwrap_or(0.0);
                let len_px = world_per_pixel
                    .map(|wpp| line_w / wpp)
                    .unwrap_or(f32::INFINITY);
                if len_px >= 2.0 {
                    let line_y_local = v_offset;
                    let p0 = to_wcs(-line_w * h_anchor, line_y_local);
                    let p1 = to_wcs(line_w * (1.0 - h_anchor), line_y_local);
                    wires.push(WireModel {
                        name: name.clone(),
                        points: vec![p0, p1],
                        color: text_color,
                        selected,
                        aci: 0,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts: vec![(Vec3::new(local_ins_x, local_ins_y, z), SnapHint::Node)],
                        tangent_geoms: vec![],
                        key_vertices: vec![],
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        vp_scissor: None,
                        fill_tris: vec![],
                    });
                }
            } else if lod_mode == 1 {
                // One filled rect per line — keeps the visual "text lives here
                // per row" hint that multi-line MText carries, in the text's
                // own color. Empty `points` opts out of the face3d 0.45 dim so
                // the fill renders at full intensity.
                let mut greek_tris: Vec<[f32; 3]> = Vec::with_capacity(line_widths.len() * 6);
                for (i, &line_w) in line_widths.iter().enumerate() {
                    let li = i as f32;
                    let line_y_bottom = -li * line_h + v_offset;
                    let line_y_top = line_y_bottom + height;
                    if line_w <= 0.0 {
                        continue;
                    }
                    let left = -line_w * h_anchor;
                    let right = line_w * (1.0 - h_anchor);
                    let bl = to_wcs(left, line_y_bottom);
                    let br = to_wcs(right, line_y_bottom);
                    let tr = to_wcs(right, line_y_top);
                    let tl = to_wcs(left, line_y_top);
                    greek_tris.extend_from_slice(&[bl, br, tr, bl, tr, tl]);
                }
                if !greek_tris.is_empty() {
                    wires.push(WireModel {
                        name: name.clone(),
                        points: vec![],
                        color: text_color,
                        selected,
                        aci: 0,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px: 1.0,
                        snap_pts: vec![(Vec3::new(local_ins_x, local_ins_y, z), SnapHint::Node)],
                        tangent_geoms: vec![],
                        key_vertices: vec![],
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        vp_scissor: None,
                        fill_tris: greek_tris,
                    });
                }
            } else {
                // Pre-tessellated by `layout_mtext`. Each TextStroke is in
                // local glyph space with its world origin (already offset-
                // relative because we passed local_ins_x/y) stored as f64.
                let mut text_points: Vec<[f32; 3]> = Vec::new();
                for ts in &layout.strokes {
                    let ox = ts.origin[0] as f32;
                    let oy = ts.origin[1] as f32;
                    for stroke in &ts.strokes {
                        if stroke.len() < 2 {
                            continue;
                        }
                        text_points.push(nan);
                        for &[x, y] in stroke {
                            text_points.push([x + ox, y + oy, z]);
                        }
                    }
                }

                if !text_points.is_empty() {
                    wires.push(WireModel {
                        name: name.clone(),
                        points: text_points,
                        color: text_color,
                        selected,
                        aci: 0,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px,
                        snap_pts: vec![(Vec3::new(local_ins_x, local_ins_y, z), SnapHint::Node)],
                        tangent_geoms: vec![],
                        key_vertices: vec![],
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        vp_scissor: None,
                        fill_tris: vec![],
                    });
                }
            }

            // Text frame / background-fill rectangle in local frame, then rotated to WCS.
            if ml.text_frame || ctx.background_fill_enabled {
                // Visual gap so the frame/fill doesn't touch glyph caps.
                let pad = height * 0.25;
                let block_top = v_offset + height + pad;
                let block_bottom = v_offset - (n_lines - 1.0) * line_h - pad;
                let block_left = -max_line_w * h_anchor - pad;
                let block_right = max_line_w * (1.0 - h_anchor) + pad;
                let local_corners: [[f32; 2]; 4] = [
                    [block_left, block_bottom],
                    [block_right, block_bottom],
                    [block_right, block_top],
                    [block_left, block_top],
                ];
                let wcs_corners: [[f32; 3]; 4] = std::array::from_fn(|i| {
                    let lx = local_corners[i][0];
                    let ly = local_corners[i][1];
                    let wx = local_ins_x + lx * cos_r - ly * sin_r;
                    let wy = local_ins_y + lx * sin_r + ly * cos_r;
                    [wx, wy, z]
                });

                // Background fill — emit two triangles; renders under the text strokes.
                if ctx.background_fill_enabled {
                    let fill_color = if selected {
                        line_color
                    } else {
                        color_or_inherit(&ctx.background_fill_color, entity_color)
                    };
                    let fill_tris: Vec<[f32; 3]> = vec![
                        wcs_corners[0],
                        wcs_corners[1],
                        wcs_corners[2],
                        wcs_corners[0],
                        wcs_corners[2],
                        wcs_corners[3],
                    ];
                    wires.push(WireModel {
                        name: name.clone(),
                        points: vec![],
                        color: fill_color,
                        selected,
                        aci: 0,
                        pattern_length: 0.0,
                        pattern: [0.0; 8],
                        line_weight_px: 1.0,
                        snap_pts: vec![],
                        tangent_geoms: vec![],
                        key_vertices: vec![],
                        aabb: WireModel::UNBOUNDED_AABB,
                        plinegen: true,
                        vp_scissor: None,
                        fill_tris,
                    });
                }

                // Text frame — closed rectangle, matches text color.
                if ml.text_frame {
                    let frame_points: Vec<[f32; 3]> = vec![
                        wcs_corners[0],
                        wcs_corners[1],
                        wcs_corners[2],
                        wcs_corners[3],
                        wcs_corners[0],
                    ];
                    wires.push(WireModel {
                        name,
                        points: frame_points,
                        color: text_color,
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
                        vp_scissor: None,
                        fill_tris: vec![],
                    });
                }
            }
        }

        wires
    }
}
