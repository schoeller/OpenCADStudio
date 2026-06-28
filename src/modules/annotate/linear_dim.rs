use acadrust::entities::{Dimension, DimensionLinear};
use acadrust::types::Vector3;
use acadrust::EntityType;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::{DVec3, Mat4, Vec3};

/// World-space measurement axis for a linear dimension between `first` and
/// `second`, chosen as the UCS X or Y axis (whichever the span is closer to).
/// `ucs` is the UCS→wire affine; identity gives the world X/Y behaviour.
fn measure_axis(first: Vec3, second: Vec3, ucs: Mat4) -> Vec3 {
    let ux = ucs.transform_vector3(Vec3::X).normalize_or(Vec3::X);
    let uy = ucs.transform_vector3(Vec3::Y).normalize_or(Vec3::Y);
    let du = ucs.inverse().transform_vector3(second - first);
    if du.x.abs() >= du.y.abs() {
        ux
    } else {
        uy
    }
}

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/dim_linear.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "DIMLINEAR",
        label: "Linear",
        icon: ICON,
        event: ModuleEvent::Command("DIMLINEAR".to_string()),
    }
}

enum Step {
    FirstPoint,
    SecondPoint(Vec3),
    DimensionLine { first: Vec3, second: Vec3 },
}

pub struct LinearDimensionCommand {
    step: Step,
    ucs: Mat4,
    /// Optional text that replaces the measured value (None = measurement).
    text_override: Option<String>,
    /// True while the next typed line is captured as the text override.
    awaiting_text: bool,
    /// Explicit text rotation in radians (None = follow the UCS/style).
    text_angle: Option<f64>,
    /// True while the next typed value is captured as the text angle.
    awaiting_angle: bool,
}

impl LinearDimensionCommand {
    pub fn new() -> Self {
        Self {
            step: Step::FirstPoint,
            ucs: Mat4::IDENTITY,
            text_override: None,
            awaiting_text: false,
            text_angle: None,
            awaiting_angle: false,
        }
    }
}

impl CadCommand for LinearDimensionCommand {
    fn name(&self) -> &'static str {
        "DIMLINEAR"
    }

    fn set_ucs(&mut self, ucs: Mat4) {
        self.ucs = ucs;
    }

    fn prompt(&self) -> String {
        if self.awaiting_text {
            return "DIMLINEAR  Enter dimension text (blank = measured value):".into();
        }
        if self.awaiting_angle {
            return "DIMLINEAR  Specify text angle (degrees):".into();
        }
        match self.step {
            Step::FirstPoint => "DIMLINEAR  Specify first extension line origin:".into(),
            Step::SecondPoint(_) => {
                "DIMLINEAR  Specify second extension line origin  [Text/Angle]:".into()
            }
            Step::DimensionLine { .. } => {
                "DIMLINEAR  Specify dimension line location  [Text/Angle]:".into()
            }
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult { let pt = pt.as_vec3();
        match self.step {
            Step::FirstPoint => {
                self.step = Step::SecondPoint(pt);
                CmdResult::NeedPoint
            }
            Step::SecondPoint(first) => {
                self.step = Step::DimensionLine { first, second: pt };
                CmdResult::NeedPoint
            }
            Step::DimensionLine { first, second } => {
                let mut dim = DimensionLinear::new(v3(first), v3(second));
                // Measure along the UCS axis the span is closest to; the DXF
                // rotation is that axis's angle in world space.
                let axis = measure_axis(first, second, self.ucs);
                dim.rotation = (axis.y as f64).atan2(axis.x as f64);
                // Bake the UCS X-axis angle as the text rotation so the
                // measurement text reads horizontally in the UCS (i.e. square on
                // screen) regardless of the style's force-horizontal flags,
                // which would otherwise pin it to world-horizontal and tilt it
                // under a rotated UCS. Left at 0 (natural) when there's no UCS.
                let ux = self.ucs.transform_vector3(Vec3::X);
                let ucs_ang = (ux.y as f64).atan2(ux.x as f64);
                if ucs_ang.abs() > 1e-9 {
                    dim.base.text_rotation = ucs_ang;
                }
                dim.definition_point = v3(pt);
                dim.base.definition_point = v3(pt);
                dim.base.text_middle_point = v3(linear_text_pos(first, second, pt, axis));
                dim.base.insertion_point = dim.base.text_middle_point;
                dim.base.actual_measurement = dim.measurement();
                dim.base.user_text = self.text_override.clone();
                // An explicit text angle overrides the UCS-derived rotation.
                if let Some(a) = self.text_angle {
                    dim.base.text_rotation = a;
                }
                CmdResult::CommitAndExit(EntityType::Dimension(Dimension::Linear(dim)))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        // A bare Enter while entering override text/angle accepts the default.
        if self.awaiting_text {
            self.awaiting_text = false;
            return CmdResult::NeedPoint;
        }
        if self.awaiting_angle {
            self.awaiting_angle = false;
            return CmdResult::NeedPoint;
        }
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        true
    }

    fn point_step_accepts_keywords(&self) -> bool {
        // While typing the override text or angle, route input as a value, not
        // a point pick / keyword.
        !self.awaiting_text && !self.awaiting_angle
    }

    fn wants_text_with_spaces(&self) -> bool {
        // The override text may contain spaces.
        self.awaiting_text
    }

    fn on_text_input(&mut self, text: &str) -> Option<CmdResult> {
        if self.awaiting_text {
            let t = text.trim();
            // Blank (or the "<>" placeholder) keeps the measured value.
            self.text_override = if t.is_empty() || t == "<>" {
                None
            } else {
                Some(t.to_string())
            };
            self.awaiting_text = false;
            return Some(CmdResult::NeedPoint);
        }
        if self.awaiting_angle {
            let t = text.trim();
            // Blank clears any explicit angle (follow the UCS/style again).
            self.text_angle = if t.is_empty() {
                None
            } else {
                t.parse::<f64>().ok().map(f64::to_radians)
            };
            self.awaiting_angle = false;
            return Some(CmdResult::NeedPoint);
        }
        match text.trim().to_uppercase().as_str() {
            "T" | "TEXT" | "M" | "MTEXT" => {
                self.awaiting_text = true;
                Some(CmdResult::NeedPoint)
            }
            "A" | "ANGLE" => {
                self.awaiting_angle = true;
                Some(CmdResult::NeedPoint)
            }
            _ => None,
        }
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> { let pt = pt.as_vec3();
        match self.step {
            Step::FirstPoint => None,
            Step::SecondPoint(first) => Some(preview_wire(vec![first, pt])),
            Step::DimensionLine { first, second } => {
                let axis = measure_axis(first, second, self.ucs);
                Some(preview_wire(linear_dimension_preview(first, second, pt, axis)))
            }
        }
    }
}

fn v3(pt: Vec3) -> Vector3 {
    Vector3::new(pt.x as f64, pt.y as f64, pt.z as f64)
}

fn preview_wire(points: Vec<Vec3>) -> WireModel {
    WireModel {
        name: "dimlinear_preview".to_string(),
        points: points.into_iter().map(|p| [p.x, p.y, p.z]).collect(),
        points_low: Vec::new(),
        color: WireModel::CYAN,
        selected: false,
        pattern_length: 0.0,
        pattern: [0.0; 8],
        line_weight_px: 1.0,
        snap_pts: vec![],
        tangent_geoms: vec![],
        aci: 0,
        key_vertices: vec![],
        aabb: WireModel::UNBOUNDED_AABB,
        plinegen: true,
        vp_scissor: None,
        fill_tris: vec![],
        fill_tris_low: Vec::new(),
    }
}

/// Project the two extension origins onto the dimension line, which passes
/// through `def` along `axis`. Each origin is projected *independently*: a
/// single shared offset only lands both on the line when they are level, and
/// tilts the dimension line when they are not (e.g. measuring across sloped
/// points). See #181.
fn dim_line_endpoints(first: Vec3, second: Vec3, def: Vec3, axis: Vec3) -> (Vec3, Vec3) {
    let perp = Vec3::new(-axis.y, axis.x, 0.0);
    let dperp = def.dot(perp);
    let d1 = first + perp * (dperp - first.dot(perp));
    let d2 = second + perp * (dperp - second.dot(perp));
    (d1, d2)
}

fn linear_dimension_preview(first: Vec3, second: Vec3, def: Vec3, axis: Vec3) -> Vec<Vec3> {
    let (d1, d2) = dim_line_endpoints(first, second, def, axis);
    let nan = Vec3::new(f32::NAN, f32::NAN, f32::NAN);
    vec![first, d1, nan, second, d2, nan, d1, d2]
}

fn linear_text_pos(first: Vec3, second: Vec3, def: Vec3, axis: Vec3) -> Vec3 {
    let (d1, d2) = dim_line_endpoints(first, second, def, axis);
    let perp = Vec3::new(-axis.y, axis.x, 0.0);
    (d1 + d2) * 0.5 + perp * 0.15
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["DIMLINEAR", "DLI"] });  // LinearDimensionCommand
