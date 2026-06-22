use acadrust::entities::{Dimension, DimensionLinear};
use acadrust::types::Vector3;
use acadrust::EntityType;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::{Mat4, Vec3};

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
}

impl LinearDimensionCommand {
    pub fn new() -> Self {
        Self {
            step: Step::FirstPoint,
            ucs: Mat4::IDENTITY,
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
        match self.step {
            Step::FirstPoint => "DIMLINEAR  Specify first extension line origin:".into(),
            Step::SecondPoint(_) => "DIMLINEAR  Specify second extension line origin:".into(),
            Step::DimensionLine { .. } => "DIMLINEAR  Specify dimension line location:".into(),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
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
                CmdResult::CommitAndExit(EntityType::Dimension(Dimension::Linear(dim)))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
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
    }
}

fn linear_dimension_preview(first: Vec3, second: Vec3, def: Vec3, axis: Vec3) -> Vec<Vec3> {
    let perp = Vec3::new(-axis.y, axis.x, 0.0);
    let offset = (def - first).dot(perp);
    let d1 = first + perp * offset;
    let d2 = second + perp * offset;
    vec![
        first,
        d1,
        Vec3::new(f32::NAN, f32::NAN, f32::NAN),
        second,
        d2,
        Vec3::new(f32::NAN, f32::NAN, f32::NAN),
        d1,
        d2,
    ]
}

fn linear_text_pos(first: Vec3, second: Vec3, def: Vec3, axis: Vec3) -> Vec3 {
    let perp = Vec3::new(-axis.y, axis.x, 0.0);
    let offset = (def - first).dot(perp);
    let d1 = first + perp * offset;
    let d2 = second + perp * offset;
    (d1 + d2) * 0.5 + perp * 0.15
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["DIMLINEAR"] });  // LinearDimensionCommand
