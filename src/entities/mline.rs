use acadrust::entities::MLine;
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{edit_prop as edit, ro_prop as ro, square_grip};
use crate::entities::traits::{Grippable, PropertyEditable, Transformable, TruckConvertible};
use crate::scene::convert::acad_to_truck::{TruckEntity, TruckObject};
use crate::scene::model::object::{GripApply, GripDef, PropSection, PropValue, Property};
use crate::scene::model::wire_model::SnapHint;

impl TruckConvertible for MLine {
    fn to_truck(&self, _document: &acadrust::CadDocument) -> Option<TruckEntity> {
        if self.vertices.is_empty() {
            return None;
        }

        let n = self.vertices.len();
        let closed = self.flags.contains(acadrust::entities::MLineFlags::CLOSED);

        // Spine: center line connecting all vertex positions.
        // Also attempt to draw parallel offset lines (±scale/2 in miter direction)
        // when scale_factor is non-zero.
        let scale = self.scale_factor;

        let mut pts: Vec<[f64; 3]> = Vec::new();

        // Center spine.
        for v in &self.vertices {
            pts.push([v.position.x, v.position.y, v.position.z]);
        }
        if closed && n >= 2 {
            pts.push([
                self.vertices[0].position.x,
                self.vertices[0].position.y,
                self.vertices[0].position.z,
            ]);
        }

        // Parallel offset lines — one at +scale/2 and one at -scale/2
        // along each vertex's miter direction.
        if scale.abs() > 1e-6 {
            let half = scale * 0.5;
            for sign in [-1.0_f64, 1.0_f64] {
                let offset = half * sign;
                pts.push([f64::NAN; 3]);
                for v in &self.vertices {
                    let mx = v.miter.x;
                    let my = v.miter.y;
                    let mz = v.miter.z;
                    pts.push([
                        v.position.x + mx * offset,
                        v.position.y + my * offset,
                        v.position.z + mz * offset,
                    ]);
                }
                if closed && n >= 2 {
                    let v0 = &self.vertices[0];
                    let mx = v0.miter.x;
                    let my = v0.miter.y;
                    let mz = v0.miter.z;
                    pts.push([
                        v0.position.x + mx * offset,
                        v0.position.y + my * offset,
                        v0.position.z + mz * offset,
                    ]);
                }
            }

            // Start and end caps: perpendicular line connecting the two offset lines
            // at the first and last vertex of an open MLine.
            if !closed {
                let cap_v = |v: &acadrust::entities::MLineVertex| {
                    let mx = v.miter.x;
                    let my = v.miter.y;
                    let mz = v.miter.z;
                    let px = v.position.x;
                    let py = v.position.y;
                    let pz = v.position.z;
                    [
                        [f64::NAN; 3],
                        [px + mx * (-half), py + my * (-half), pz + mz * (-half)],
                        [px + mx * half, py + my * half, pz + mz * half],
                    ]
                };
                pts.extend_from_slice(&cap_v(&self.vertices[0]));
                pts.extend_from_slice(&cap_v(&self.vertices[n - 1]));
            }
        }

        let key_verts: Vec<[f64; 3]> = self
            .vertices
            .iter()
            .map(|v| [v.position.x, v.position.y, v.position.z])
            .collect();

        let snap_pts = self
            .vertices
            .iter()
            .map(|v| {
                (
                    Vec3::new(
                        v.position.x as f32,
                        v.position.y as f32,
                        v.position.z as f32,
                    ),
                    SnapHint::Node,
                )
            })
            .collect();

        Some(TruckEntity {
            object: TruckObject::Lines(pts),
            snap_pts,
            tangent_geoms: vec![],
            key_vertices: key_verts,
            fill_tris: vec![],
        })
    }
}

impl Grippable for MLine {
    fn grips(&self) -> Vec<GripDef> {
        self.vertices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                square_grip(
                    i,
                    glam::DVec3::new(v.position.x, v.position.y, v.position.z),
                )
            })
            .collect()
    }

    fn apply_grip(&mut self, grip_id: usize, apply: GripApply) {
        if let Some(v) = self.vertices.get_mut(grip_id) {
            match apply {
                GripApply::Translate(d) => {
                    v.position.x += d.x as f64;
                    v.position.y += d.y as f64;
                    v.position.z += d.z as f64;
                }
                GripApply::Absolute(p) => {
                    v.position.x = p.x as f64;
                    v.position.y = p.y as f64;
                    v.position.z = p.z as f64;
                }
            }
        }
    }

    fn grip_menu(&self, _grip_id: usize) -> Vec<crate::scene::model::object::GripMenuItem> {
        use crate::scene::model::object::{GripMenuAction, GripMenuItem};
        vec![
            GripMenuItem {
                label: "Stretch",
                action: GripMenuAction::Stretch,
            },
            GripMenuItem {
                label: "Add Vertex",
                action: GripMenuAction::AddVertex,
            },
            GripMenuItem {
                label: "Remove Vertex",
                action: GripMenuAction::RemoveVertex,
            },
        ]
    }

    fn apply_grip_menu(&mut self, grip_id: usize, action: crate::scene::model::object::GripMenuAction) {
        use crate::scene::model::object::GripMenuAction as A;
        let n = self.vertices.len();
        match action {
            A::AddVertex if grip_id < n => {
                let i1 = (grip_id + 1).min(n - 1);
                if i1 == grip_id {
                    return;
                }
                let v0 = &self.vertices[grip_id];
                let v1 = &self.vertices[i1];
                let mut new_v = v0.clone();
                new_v.position.x = (v0.position.x + v1.position.x) * 0.5;
                new_v.position.y = (v0.position.y + v1.position.y) * 0.5;
                new_v.position.z = (v0.position.z + v1.position.z) * 0.5;
                self.vertices.insert(i1, new_v);
            }
            A::RemoveVertex if grip_id < n && n > 2 => {
                self.vertices.remove(grip_id);
            }
            _ => {}
        }
    }
}

impl PropertyEditable for MLine {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        let just_str = match self.justification {
            acadrust::entities::MLineJustification::Top => "Top",
            acadrust::entities::MLineJustification::Zero => "Zero",
            acadrust::entities::MLineJustification::Bottom => "Bottom",
        };
        PropSection {
            title: "Geometry".into(),
            props: vec![
                ro("Style", "ml_style", self.style_name.clone()),
                ro(
                    "Style Handle",
                    "ml_style_handle",
                    match self.style_handle {
                        Some(h) if !h.is_null() => format!("{:X}", h.value()),
                        _ => "(none)".to_string(),
                    },
                ),
                ro("Vertices", "ml_verts", self.vertices.len().to_string()),
                ro(
                    "Style Elements",
                    "ml_style_elem_count",
                    self.style_element_count.to_string(),
                ),
                edit("Scale", "ml_scale", self.scale_factor),
                Property {
                    label: "Justification".into(),
                    field: "ml_justification",
                    value: PropValue::Choice {
                        selected: just_str.to_string(),
                        options: ["Top", "Zero", "Bottom"]
                            .into_iter()
                            .map(str::to_string)
                            .collect(),
                    },
                },
                Property {
                    label: "Closed".into(),
                    field: "ml_closed",
                    value: PropValue::BoolToggle {
                        field: "ml_closed",
                        value: self.flags.contains(acadrust::entities::MLineFlags::CLOSED),
                    },
                },
            ],
        }
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        match field {
            "ml_closed" => {
                let closed = if value == "toggle" {
                    !self.flags.contains(acadrust::entities::MLineFlags::CLOSED)
                } else {
                    value == "true"
                };
                self.flags
                    .set(acadrust::entities::MLineFlags::CLOSED, closed);
                return;
            }
            "ml_justification" => {
                self.justification = match value {
                    "Top" => acadrust::entities::MLineJustification::Top,
                    "Bottom" => acadrust::entities::MLineJustification::Bottom,
                    _ => acadrust::entities::MLineJustification::Zero,
                };
                return;
            }
            _ => {}
        }
        let Ok(v) = value.trim().parse::<f64>() else {
            return;
        };
        if field == "ml_scale" && v != 0.0 {
            self.scale_factor = v;
        }
    }
}

impl Transformable for MLine {
    fn apply_transform(&mut self, t: &EntityTransform) {
        crate::scene::view::transform::apply_standard_entity_transform(self, t, |entity, p1, p2| {
            for v in &mut entity.vertices {
                crate::scene::view::transform::reflect_xy_point(
                    &mut v.position.x,
                    &mut v.position.y,
                    p1,
                    p2,
                );
            }
            crate::scene::view::transform::reflect_xy_point(
                &mut entity.start_point.x,
                &mut entity.start_point.y,
                p1,
                p2,
            );
        });
    }
}
