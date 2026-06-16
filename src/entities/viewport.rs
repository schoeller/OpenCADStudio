use acadrust::entities::{Viewport, ViewportRenderMode};

use crate::command::EntityTransform;
use crate::entities::common::{
    center_grip, edit_prop as edit, parse_f64, ro_prop as ro, square_grip,
};

use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};

// ── Standard scale options ────────────────────────────────────────────────

const STANDARD_SCALES: &[(&str, f64)] = &[
    ("1:500", 0.002),
    ("1:200", 0.005),
    ("1:100", 0.01),
    ("1:50", 0.02),
    ("1:20", 0.05),
    ("1:10", 0.1),
    ("1:5", 0.2),
    ("1:2", 0.5),
    ("1:1", 1.0),
    ("2:1", 2.0),
    ("5:1", 5.0),
    ("10:1", 10.0),
];

/// Parse a scale ratio name to its paper/drawing factor: "1:50" -> 0.02,
/// "2:1" -> 2.0. Also accepts a plain decimal ("0.02").
fn parse_scale_ratio(label: &str) -> Option<f64> {
    if let Some((a, b)) = label.split_once(':') {
        let n: f64 = a.trim().parse().ok()?;
        let d: f64 = b.trim().parse().ok()?;
        if d.abs() > 1e-12 {
            return Some(n / d);
        }
        return None;
    }
    label.trim().parse::<f64>().ok()
}

fn scale_label(scale: f64) -> String {
    for (label, val) in STANDARD_SCALES {
        if (scale - val).abs() < val * 0.01 {
            return label.to_string();
        }
    }
    format!("{:.6}", scale)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

// ── Render mode options ───────────────────────────────────────────────────

const RENDER_MODES: &[(&str, ViewportRenderMode)] = &[
    ("2D Wireframe", ViewportRenderMode::Wireframe2D),
    ("3D Wireframe", ViewportRenderMode::Wireframe3D),
    ("Hidden Line", ViewportRenderMode::HiddenLine),
    ("Flat Shaded", ViewportRenderMode::FlatShaded),
    ("Gouraud Shaded", ViewportRenderMode::GouraudShaded),
    (
        "Flat Shaded + Edges",
        ViewportRenderMode::FlatShadedWithEdges,
    ),
    (
        "Gouraud Shaded + Edges",
        ViewportRenderMode::GouraudShadedWithEdges,
    ),
];

fn render_mode_label(mode: &ViewportRenderMode) -> &'static str {
    for (label, m) in RENDER_MODES {
        if m == mode {
            return label;
        }
    }
    "2D Wireframe"
}

// ── Shade plot mode labels ────────────────────────────────────────────────

const SHADE_PLOT_LABELS: &[&str] = &["As Displayed", "Wireframe", "Hidden", "Rendered"];

fn shade_plot_label(mode: i16) -> &'static str {
    SHADE_PLOT_LABELS
        .get(mode as usize)
        .copied()
        .unwrap_or("As Displayed")
}

// ── Standard view options ─────────────────────────────────────────────────

const STD_VIEWS: &[&str] = &[
    "Top",
    "Bottom",
    "Front",
    "Back",
    "Left",
    "Right",
    "SW Isometric",
    "SE Isometric",
    "NE Isometric",
    "NW Isometric",
];

fn grips(vp: &Viewport) -> Vec<GripDef> {
    let (cx, cy, cz) = (vp.center.x, vp.center.y, vp.center.z);
    let hw = vp.width / 2.0;
    let hh = vp.height / 2.0;
    vec![
        center_grip(0, glam::DVec3::new(cx, cy, cz)),
        square_grip(1, glam::DVec3::new(cx + hw, cy + hh, cz)),
        square_grip(2, glam::DVec3::new(cx - hw, cy + hh, cz)),
        square_grip(3, glam::DVec3::new(cx - hw, cy - hh, cz)),
        square_grip(4, glam::DVec3::new(cx + hw, cy - hh, cz)),
    ]
}

fn properties(vp: &Viewport) -> PropSection {
    let scale_opts: Vec<String> = STANDARD_SCALES.iter().map(|(s, _)| s.to_string()).collect();
    let effective_scale =
        crate::scene::vp_effective_scale(vp.custom_scale, vp.view_height, vp.height);
    let current_scale_label = scale_label(effective_scale);

    let view_opts: Vec<String> = STD_VIEWS.iter().map(|s| s.to_string()).collect();
    let current_view = viewport_view_label(vp);

    let render_opts: Vec<String> = RENDER_MODES.iter().map(|(s, _)| s.to_string()).collect();
    let current_render = render_mode_label(&vp.render_mode).to_string();

    let shade_opts: Vec<String> = SHADE_PLOT_LABELS.iter().map(|s| s.to_string()).collect();
    let current_shade = shade_plot_label(vp.shade_plot_mode).to_string();

    PropSection {
        title: "Geometry".into(),
        props: vec![
            edit("Center X", "center_x", vp.center.x),
            edit("Center Y", "center_y", vp.center.y),
            edit("Center Z", "center_z", vp.center.z),
            edit("Width", "vp_w", vp.width),
            edit("Height", "vp_h", vp.height),
            // Numeric scale entry (derived from view_height; writes both).
            edit("Scale (num)", "vscale", effective_scale),
            // Standard scale picker.
            Property {
                label: "Scale".into(),
                field: "vscale_std",
                value: PropValue::Choice {
                    selected: current_scale_label,
                    options: scale_opts,
                },
            },
            // Standard view picker.
            Property {
                label: "View".into(),
                field: "vp_view",
                value: PropValue::Choice {
                    selected: current_view,
                    options: view_opts,
                },
            },
            // Render mode picker.
            Property {
                label: "Render Mode".into(),
                field: "vp_render",
                value: PropValue::Choice {
                    selected: current_render,
                    options: render_opts,
                },
            },
            // Shade plot mode picker.
            Property {
                label: "Shade Plot".into(),
                field: "vp_shade_plot",
                value: PropValue::Choice {
                    selected: current_shade,
                    options: shade_opts,
                },
            },
            // Display state toggles.
            Property {
                label: "Locked".into(),
                field: "vp_locked",
                value: PropValue::BoolToggle {
                    field: "vp_locked",
                    value: vp.status.locked,
                },
            },
            Property {
                label: "On".into(),
                field: "vp_on",
                value: PropValue::BoolToggle {
                    field: "vp_on",
                    value: vp.status.is_on,
                },
            },
            Property {
                label: "Perspective".into(),
                field: "vp_perspective",
                value: PropValue::BoolToggle {
                    field: "vp_perspective",
                    value: vp.status.perspective,
                },
            },
            Property {
                label: "Hide Plot".into(),
                field: "vp_hide_plot",
                value: PropValue::BoolToggle {
                    field: "vp_hide_plot",
                    value: vp.status.hide_plot,
                },
            },
            Property {
                label: "UCS Icon".into(),
                field: "vp_ucs_icon",
                value: PropValue::BoolToggle {
                    field: "vp_ucs_icon",
                    value: vp.ucs_icon_visible,
                },
            },
            edit("Target X", "vtgt_x", vp.view_target.x),
            edit("Target Z", "vtgt_z", vp.view_target.z),
            // Perspective lens length.
            edit("Lens Length (mm)", "vp_lens", vp.lens_length),
            // Per-Viewport UCS.
            Property {
                label: "UCS Per Viewport".into(),
                field: "vp_ucs_per_vp",
                value: PropValue::BoolToggle {
                    field: "vp_ucs_per_vp",
                    value: vp.ucs_per_viewport,
                },
            },
            edit("UCS Origin X", "vp_ucs_ox", vp.ucs_origin.x),
            edit("UCS Origin Y", "vp_ucs_oy", vp.ucs_origin.y),
            edit("UCS Origin Z", "vp_ucs_oz", vp.ucs_origin.z),
            edit("UCS X-Axis X", "vp_ucs_xx", vp.ucs_x_axis.x),
            edit("UCS X-Axis Y", "vp_ucs_xy", vp.ucs_x_axis.y),
            edit("UCS X-Axis Z", "vp_ucs_xz", vp.ucs_x_axis.z),
            edit("UCS Y-Axis X", "vp_ucs_yx", vp.ucs_y_axis.x),
            edit("UCS Y-Axis Y", "vp_ucs_yy", vp.ucs_y_axis.y),
            edit("UCS Y-Axis Z", "vp_ucs_yz", vp.ucs_y_axis.z),
            // Snap / grid metadata (drives the snap-aware UI; not rendered
            // into the viewport contents directly).
            edit("Snap Base X", "vp_snap_bx", vp.snap_base.x),
            edit("Snap Base Y", "vp_snap_by", vp.snap_base.y),
            edit("Snap Spacing X", "vp_snap_sx", vp.snap_spacing.x),
            edit("Snap Spacing Y", "vp_snap_sy", vp.snap_spacing.y),
            edit("Snap Angle", "vp_snap_ang", vp.snap_angle.to_degrees()),
            edit("Twist Angle", "vp_twist", vp.twist_angle.to_degrees()),
            edit("Front Clip Z", "vp_front_clip", vp.front_clip_z),
            edit("Back Clip Z", "vp_back_clip", vp.back_clip_z),
            edit("Circle Sides", "vp_circle_sides", vp.circle_sides as f64),
            ro(
                "Grid Flags",
                "vp_grid_flags",
                format!("{:#06b}", vp.grid_flags.to_bits()),
            ),
            ro("Grid Major", "vp_grid_major", vp.grid_major.to_string()),
            ro(
                "Base UCS",
                "vp_base_ucs_handle",
                if vp.base_ucs_handle.is_null() {
                    "(none)".to_string()
                } else {
                    format!("{:X}", vp.base_ucs_handle.value())
                },
            ),
            ro("UCS Ortho", "vp_ucs_ortho", vp.ucs_ortho_type.to_string()),
            ro(
                "Background",
                "vp_background_handle",
                if vp.background_handle.is_null() {
                    "(none)".to_string()
                } else {
                    format!("{:X}", vp.background_handle.value())
                },
            ),
            ro(
                "Shade Plot",
                "vp_shade_plot_handle",
                if vp.shade_plot_handle.is_null() {
                    "(none)".to_string()
                } else {
                    format!("{:X}", vp.shade_plot_handle.value())
                },
            ),
            ro(
                "Visual Style",
                "vp_visual_style_handle",
                if vp.visual_style_handle.is_null() {
                    "(none)".to_string()
                } else {
                    format!("{:X}", vp.visual_style_handle.value())
                },
            ),
            // 3D lighting (not yet applied to the viewport mesh path).
            ro(
                "Default Lighting",
                "vp_default_lighting",
                if vp.default_lighting { "Yes" } else { "No" },
            ),
            ro(
                "Lighting Type",
                "vp_lighting_type",
                vp.default_lighting_type.to_string(),
            ),
            ro(
                "Ambient Color",
                "vp_ambient_color",
                format!("{:#010x}", vp.ambient_color as u32),
            ),
        ],
    }
}

/// Identify which standard view matches the viewport's view direction.
fn viewport_view_label(vp: &Viewport) -> String {
    let d = &vp.view_direction;
    let dx = d.x;
    let dy = d.y;
    let dz = d.z;

    // Use a simple threshold comparison to classify the view direction.
    if dx.abs() < 0.1 && dy.abs() < 0.1 && dz > 0.5 {
        return "Top".into();
    }
    if dx.abs() < 0.1 && dy.abs() < 0.1 && dz < -0.5 {
        return "Bottom".into();
    }
    if dx.abs() < 0.1 && dy < -0.5 && dz.abs() < 0.1 {
        return "Front".into();
    }
    if dx.abs() < 0.1 && dy > 0.5 && dz.abs() < 0.1 {
        return "Back".into();
    }
    if dx < -0.5 && dy.abs() < 0.1 && dz.abs() < 0.1 {
        return "Left".into();
    }
    if dx > 0.5 && dy.abs() < 0.1 && dz.abs() < 0.1 {
        return "Right".into();
    }
    if dx < -0.4 && dy < -0.4 && dz > 0.4 {
        return "SW Isometric".into();
    }
    if dx > 0.4 && dy < -0.4 && dz > 0.4 {
        return "SE Isometric".into();
    }
    if dx > 0.4 && dy > 0.4 && dz > 0.4 {
        return "NE Isometric".into();
    }
    if dx < -0.4 && dy > 0.4 && dz > 0.4 {
        return "NW Isometric".into();
    }
    "Custom".into()
}

fn apply_geom_prop(vp: &mut Viewport, field: &str, value: &str) {
    use acadrust::types::Vector3;

    // Boolean / toggle fields handled first (value = "toggle" or "true"/"false").
    match field {
        "vp_locked" => {
            vp.status.locked = if value == "toggle" {
                !vp.status.locked
            } else {
                value == "true"
            };
            return;
        }
        "vp_on" => {
            vp.status.is_on = if value == "toggle" {
                !vp.status.is_on
            } else {
                value == "true"
            };
            return;
        }
        "vp_perspective" => {
            vp.status.perspective = if value == "toggle" {
                !vp.status.perspective
            } else {
                value == "true"
            };
            return;
        }
        "vp_hide_plot" => {
            vp.status.hide_plot = if value == "toggle" {
                !vp.status.hide_plot
            } else {
                value == "true"
            };
            return;
        }
        "vp_ucs_icon" => {
            vp.ucs_icon_visible = if value == "toggle" {
                !vp.ucs_icon_visible
            } else {
                value == "true"
            };
            return;
        }
        "vp_ucs_per_vp" => {
            vp.ucs_per_viewport = if value == "toggle" {
                !vp.ucs_per_viewport
            } else {
                value == "true"
            };
            return;
        }
        _ => {}
    }

    // Scale picker. The label is a ratio name from the drawing's scale list
    // ("1:50", "2:1"); parse it to the paper/drawing factor so any named
    // scale resolves, not just the built-in set.
    if field == "vscale_std" {
        if let Some(scale) = parse_scale_ratio(value) {
            vp.custom_scale = scale;
            if scale > 1e-9 {
                vp.view_height = vp.height / scale;
            }
        }
        return;
    }

    // Render mode picker.
    if field == "vp_render" {
        if let Some(&(_, mode)) = RENDER_MODES.iter().find(|(label, _)| *label == value) {
            vp.render_mode = mode;
        }
        return;
    }

    // Shade plot mode picker.
    if field == "vp_shade_plot" {
        if let Some(idx) = SHADE_PLOT_LABELS.iter().position(|&s| s == value) {
            vp.shade_plot_mode = idx as i16;
        }
        return;
    }

    // Standard view direction picker.
    if field == "vp_view" {
        let dir: Option<(f64, f64, f64)> = match value {
            "Top" => Some((0.0, 0.0, 1.0)),
            "Bottom" => Some((0.0, 0.0, -1.0)),
            "Front" => Some((0.0, -1.0, 0.0)),
            "Back" => Some((0.0, 1.0, 0.0)),
            "Left" => Some((-1.0, 0.0, 0.0)),
            "Right" => Some((1.0, 0.0, 0.0)),
            "SW Isometric" => Some((-1.0, -1.0, 1.0)),
            "SE Isometric" => Some((1.0, -1.0, 1.0)),
            "NE Isometric" => Some((1.0, 1.0, 1.0)),
            "NW Isometric" => Some((-1.0, 1.0, 1.0)),
            _ => None,
        };
        if let Some((dx, dy, dz)) = dir {
            let len = (dx * dx + dy * dy + dz * dz).sqrt();
            vp.view_direction = Vector3::new(dx / len, dy / len, dz / len);
        }
        return;
    }

    // Numeric fields.
    let Some(v) = parse_f64(value) else { return };
    match field {
        "center_x" => vp.center.x = v,
        "center_y" => vp.center.y = v,
        "center_z" => vp.center.z = v,
        "vp_w" if v > 0.0 => vp.width = v,
        "vp_h" if v > 0.0 => vp.height = v,
        "vscale" if v > 0.0 => {
            vp.custom_scale = v;
            vp.view_height = vp.height / v;
        }
        "vtgt_x" => vp.view_target.x = v,
        "vtgt_z" => vp.view_target.z = v,
        "vp_lens" if v > 0.0 => vp.lens_length = v,
        "vp_ucs_ox" => vp.ucs_origin.x = v,
        "vp_ucs_oy" => vp.ucs_origin.y = v,
        "vp_ucs_oz" => vp.ucs_origin.z = v,
        "vp_ucs_xx" => vp.ucs_x_axis.x = v,
        "vp_ucs_xy" => vp.ucs_x_axis.y = v,
        "vp_ucs_xz" => vp.ucs_x_axis.z = v,
        "vp_ucs_yx" => vp.ucs_y_axis.x = v,
        "vp_ucs_yy" => vp.ucs_y_axis.y = v,
        "vp_ucs_yz" => vp.ucs_y_axis.z = v,
        "vp_snap_bx" => vp.snap_base.x = v,
        "vp_snap_by" => vp.snap_base.y = v,
        "vp_snap_sx" if v >= 0.0 => vp.snap_spacing.x = v,
        "vp_snap_sy" if v >= 0.0 => vp.snap_spacing.y = v,
        "vp_snap_ang" => vp.snap_angle = v.to_radians(),
        "vp_twist" => vp.twist_angle = v.to_radians(),
        "vp_front_clip" => vp.front_clip_z = v,
        "vp_back_clip" => vp.back_clip_z = v,
        "vp_circle_sides" if v >= 0.0 && v <= i16::MAX as f64 => {
            vp.circle_sides = v as i16;
        }
        _ => {}
    }
}

fn apply_grip(vp: &mut Viewport, grip_id: usize, apply: GripApply) {
    match (grip_id, apply) {
        (0, GripApply::Translate(d)) => {
            vp.center.x += d.x as f64;
            vp.center.y += d.y as f64;
            vp.center.z += d.z as f64;
        }
        (0, GripApply::Absolute(p)) => {
            vp.center.x = p.x as f64;
            vp.center.y = p.y as f64;
            vp.center.z = p.z as f64;
        }
        (1..=4, GripApply::Absolute(p)) => {
            let new_hw = (p.x as f64 - vp.center.x).abs();
            let new_hh = (p.y as f64 - vp.center.y).abs();
            if new_hw > 0.01 {
                vp.width = new_hw * 2.0;
            }
            if new_hh > 0.01 {
                let new_h = new_hh * 2.0;
                // Keep scale constant: view_height must scale with the viewport height.
                if vp.height > 1e-9 && vp.view_height.abs() > 1e-9 {
                    vp.view_height = vp.view_height * (new_h / vp.height);
                }
                vp.height = new_h;
            }
        }
        _ => {}
    }
}

fn apply_transform(vp: &mut Viewport, t: &EntityTransform) {
    crate::scene::view::transform::apply_standard_entity_transform(vp, t, |entity, p1, p2| {
        crate::scene::view::transform::reflect_xy_point(
            &mut entity.center.x,
            &mut entity.center.y,
            p1,
            p2,
        );
    });
}

crate::impl_entity_basics!(Viewport);

impl crate::entities::traits::FallbackTess for Viewport {
    fn fallback_geometry(
        &self,
        world_offset: [f64; 3],
    ) -> crate::scene::convert::tess_util::FallbackGeometry {
        let [ox, oy, oz] = world_offset;
        let cx = (self.center.x - ox) as f32;
        let cy = (self.center.y - oy) as f32;
        let cz = (self.center.z - oz) as f32;
        let hw = (self.width / 2.0) as f32;
        let hh = (self.height / 2.0) as f32;
        let pts = vec![
            [cx - hw, cy - hh, cz],
            [cx + hw, cy - hh, cz],
            [cx + hw, cy + hh, cz],
            [cx - hw, cy + hh, cz],
            [cx - hw, cy - hh, cz],
        ];
        (pts, vec![], vec![], vec![])
    }
}
