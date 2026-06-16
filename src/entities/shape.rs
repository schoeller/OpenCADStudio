// SHAPE entity — reference to an .SHX shape-file glyph.
//
// Since .SHX binary files are not parsed, we render a small diamond marker
// at the insertion point (same approach as unknown/unsupported entities)
// but apply the SHAPE's own rotation / oblique / width factor so the marker
// gives a rough indication of the glyph orientation.

use acadrust::entities::Shape;
use glam::Vec3;

use crate::command::EntityTransform;
use crate::entities::common::{edit_prop as edit, ro_prop as ro, square_grip};
use crate::entities::traits::{Grippable, PropertyEditable, Transformable, TruckConvertible};
use crate::scene::convert::acad_to_truck::{TruckEntity, TruckObject};
use crate::scene::model::object::{GripApply, GripDef, PropSection};
use crate::scene::view::transform;
use crate::scene::model::wire_model::SnapHint;

// ── Marker geometry ───────────────────────────────────────────────────────────

/// Small diamond marker at the shape insertion point. The diamond is sized
/// by `size`, stretched horizontally by `relative_x_scale`, sheared by
/// `oblique_angle`, and rotated by `rotation`.
fn shape_marker(
    ox: f64,
    oy: f64,
    oz: f64,
    size: f64,
    rotation: f64,
    rel_x_scale: f64,
    oblique_angle: f64,
) -> Vec<[f64; 3]> {
    let s = size.abs().max(0.001) * 0.5;
    let rx = s * if rel_x_scale.abs() < 1e-9 {
        1.0
    } else {
        rel_x_scale
    };
    let ry = s;
    let local = [(0.0, ry), (rx, 0.0), (0.0, -ry), (-rx, 0.0)];
    let (sin_r, cos_r) = (rotation.sin(), rotation.cos());
    let ob = oblique_angle.tan();
    let mut out: Vec<[f64; 3]> = Vec::with_capacity(6);
    for &(x, y) in &local {
        let sx = x + y * ob;
        let lx = sx * cos_r - y * sin_r + ox;
        let ly = sx * sin_r + y * cos_r + oy;
        out.push([lx, ly, oz]);
    }
    // Close polygon + segment separator.
    let first = out[0];
    out.push(first);
    out.push([f64::NAN; 3]);
    out
}

// ── TruckConvertible ──────────────────────────────────────────────────────────

impl TruckConvertible for Shape {
    fn to_truck(&self, _document: &acadrust::CadDocument) -> Option<TruckEntity> {
        let ox = self.insertion_point.x;
        let oy = self.insertion_point.y;
        let oz = self.insertion_point.z;
        let size = self.size.abs().max(0.5);

        let snap_pt = Vec3::new(ox as f32, oy as f32, oz as f32);
        let pts = shape_marker(
            ox,
            oy,
            oz,
            size,
            self.rotation,
            self.relative_x_scale,
            self.oblique_angle,
        );

        Some(TruckEntity {
            object: TruckObject::Lines(pts),
            snap_pts: vec![(snap_pt, SnapHint::Insertion)],
            tangent_geoms: vec![],
            key_vertices: vec![[ox, oy, oz]],
            fill_tris: vec![],
        })
    }
}

// ── Grippable ─────────────────────────────────────────────────────────────────

impl Grippable for Shape {
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

// ── PropertyEditable ──────────────────────────────────────────────────────────

impl PropertyEditable for Shape {
    fn geometry_properties(&self, _text_style_names: &[String]) -> PropSection {
        let style_handle_display = match self.style_handle {
            Some(h) if !h.is_null() => format!("{:X}", h.value()),
            _ => "(none)".to_string(),
        };
        PropSection {
            title: "Geometry".into(),
            props: vec![
                ro("Name", "shp_name", self.shape_name.clone()),
                ro("Number", "shp_number", self.shape_number.to_string()),
                ro("Style", "shp_style", self.style_name.clone()),
                ro("Style Handle", "shp_style_handle", style_handle_display),
                edit("Insert X", "shp_ix", self.insertion_point.x),
                edit("Insert Y", "shp_iy", self.insertion_point.y),
                edit("Insert Z", "shp_iz", self.insertion_point.z),
                edit("Size", "shp_sz", self.size),
                edit("Rotation", "shp_rot", self.rotation.to_degrees()),
                edit("Width Factor", "shp_xs", self.relative_x_scale),
                edit("Oblique Angle", "shp_ob", self.oblique_angle.to_degrees()),
                edit("Thickness", "shp_th", self.thickness),
                edit("Normal X", "shp_nx", self.normal.x),
                edit("Normal Y", "shp_ny", self.normal.y),
                edit("Normal Z", "shp_nz", self.normal.z),
            ],
        }
    }

    fn apply_geom_prop(&mut self, field: &str, value: &str) {
        let Ok(v) = value.trim().parse::<f64>() else {
            return;
        };
        match field {
            "shp_ix" => self.insertion_point.x = v,
            "shp_iy" => self.insertion_point.y = v,
            "shp_iz" => self.insertion_point.z = v,
            "shp_sz" => self.size = v.max(0.001),
            "shp_rot" => self.rotation = v.to_radians(),
            "shp_xs" if v.abs() > 1e-9 => self.relative_x_scale = v,
            "shp_ob" => self.oblique_angle = v.to_radians(),
            "shp_th" => self.thickness = v,
            "shp_nx" => self.normal.x = v,
            "shp_ny" => self.normal.y = v,
            "shp_nz" => self.normal.z = v,
            _ => {}
        }
    }
}

// ── Transformable ─────────────────────────────────────────────────────────────

impl Transformable for Shape {
    fn apply_transform(&mut self, t: &EntityTransform) {
        transform::apply_standard_entity_transform(self, t, |entity, p1, p2| {
            transform::reflect_xy_point(
                &mut entity.insertion_point.x,
                &mut entity.insertion_point.y,
                p1,
                p2,
            );
        });
    }
}
