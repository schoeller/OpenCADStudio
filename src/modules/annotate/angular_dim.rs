use acadrust::entities::{Dimension, DimensionAngular3Pt};
use acadrust::types::Vector3;
use acadrust::EntityType;

use crate::command::{CadCommand, CmdResult};
use crate::modules::{IconKind, ModuleEvent, ToolDef};
use crate::scene::model::wire_model::WireModel;
use glam::{DVec3, Vec3};

pub const ICON: IconKind = IconKind::Svg(include_bytes!("../../../assets/icons/dim_angular.svg"));

pub fn tool() -> ToolDef {
    ToolDef {
        id: "DIMANGULAR",
        label: "Angular",
        icon: ICON,
        event: ModuleEvent::Command("DIMANGULAR".to_string()),
    }
}

enum Step {
    Vertex,
    FirstRay(DVec3),
    SecondRay {
        vertex: DVec3,
        first: DVec3,
    },
    ArcPoint {
        vertex: DVec3,
        first: DVec3,
        second: DVec3,
    },
}

pub struct AngularDimensionCommand {
    step: Step,
}

impl AngularDimensionCommand {
    pub fn new() -> Self {
        Self { step: Step::Vertex }
    }
}

impl CadCommand for AngularDimensionCommand {
    fn name(&self) -> &'static str {
        "DIMANGULAR"
    }

    fn prompt(&self) -> String {
        match self.step {
            Step::Vertex => "DIMANGULAR  Specify angle vertex:".into(),
            Step::FirstRay(_) => "DIMANGULAR  Specify first extension line point:".into(),
            Step::SecondRay { .. } => "DIMANGULAR  Specify second extension line point:".into(),
            Step::ArcPoint { .. } => "DIMANGULAR  Specify dimension arc location:".into(),
        }
    }

    fn on_point(&mut self, pt: DVec3) -> CmdResult {
        match self.step {
            Step::Vertex => {
                self.step = Step::FirstRay(pt);
                CmdResult::NeedPoint
            }
            Step::FirstRay(vertex) => {
                self.step = Step::SecondRay { vertex, first: pt };
                CmdResult::NeedPoint
            }
            Step::SecondRay { vertex, first } => {
                self.step = Step::ArcPoint {
                    vertex,
                    first,
                    second: pt,
                };
                CmdResult::NeedPoint
            }
            Step::ArcPoint {
                vertex,
                first,
                second,
            } => {
                let mut dim = DimensionAngular3Pt::new(v3(vertex), v3(first), v3(second));
                dim.definition_point = v3(pt);
                dim.base.definition_point = v3(pt);
                dim.base.text_middle_point = v3(pt);
                dim.base.insertion_point = v3(pt);
                dim.base.actual_measurement = dim.measurement_degrees();
                CmdResult::CommitAndExit(EntityType::Dimension(Dimension::Angular3Pt(dim)))
            }
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn on_mouse_move(&mut self, pt: DVec3) -> Option<WireModel> {
        let pt = pt.as_vec3();
        match self.step {
            Step::Vertex => None,
            Step::FirstRay(vertex) => Some(preview_wire(vec![vertex.as_vec3(), pt])),
            Step::SecondRay { vertex, first } => Some(preview_wire(vec![
                vertex.as_vec3(),
                first.as_vec3(),
                Vec3::new(f32::NAN, f32::NAN, f32::NAN),
                vertex.as_vec3(),
                pt,
            ])),
            Step::ArcPoint {
                vertex,
                first,
                second,
            } => Some(preview_wire(angular_preview(
                vertex.as_vec3(),
                first.as_vec3(),
                second.as_vec3(),
                pt,
            ))),
        }
    }
}

fn v3(pt: DVec3) -> Vector3 {
    Vector3::new(pt.x, pt.y, pt.z)
}

fn preview_wire(points: Vec<Vec3>) -> WireModel {
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
        name: "dimangular_preview".to_string(),
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
        fill_tris: vec![],
        fill_tris_low: Vec::new(),
    }
}

fn angular_preview(vertex: Vec3, first: Vec3, second: Vec3, arc_pt: Vec3) -> Vec<Vec3> {
    let mut points = vec![
        vertex,
        first,
        Vec3::new(f32::NAN, f32::NAN, f32::NAN),
        vertex,
        second,
        Vec3::new(f32::NAN, f32::NAN, f32::NAN),
    ];
    let r = vertex.distance(arc_pt);
    if r <= 1e-6 {
        return points;
    }
    let a0 = (first.y - vertex.y).atan2(first.x - vertex.x);
    let mut a1 = (second.y - vertex.y).atan2(second.x - vertex.x);
    let mut delta = a1 - a0;
    while delta <= 0.0 {
        delta += std::f32::consts::TAU;
    }
    if delta > std::f32::consts::PI {
        a1 -= std::f32::consts::TAU;
        delta = a1 - a0;
    }
    let steps = 24;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let a = a0 + delta * t;
        points.push(vertex + Vec3::new(a.cos() * r, a.sin() * r, 0.0));
    }
    points
}


// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration { names: &["DIMANGULAR"] });  // AngularDimensionCommand
