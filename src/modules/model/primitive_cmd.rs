// 3D primitive creation — BOX / CYLINDER / CONE / SPHERE / WEDGE / PYRAMID /
// TORUS. Each is placed CAD-style with a few clicks (planar footprint first,
// then a height value), then built as a real ACIS `Solid3D` via acadrust's
// `acis::primitives` builders. `Scene::add_entity` tessellates the SAT B-rep
// into the 3D mesh pipeline, so the solid renders, selects, and saves to DXF.
//
// A matching truck `Solid` is cached on the scene (see model/mod.rs) when the
// entity is committed, so the Design-group boolean tools can combine it.

use acadrust::entities::Solid3D;
use acadrust::{primitives, EntityType};
use glam::Vec3;
use truck_modeling::Solid;

use crate::command::{CadCommand, CmdResult};
use crate::scene::model::model_solid;
use crate::scene::model::wire_model::WireModel;

/// Which primitive a `PrimitiveCommand` builds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Box,
    Wedge,
    Cylinder,
    Cone,
    Sphere,
    Torus,
}

impl Shape {
    fn from_id(id: &str) -> Option<Shape> {
        Some(match id {
            "BOX" => Shape::Box,
            "WEDGE" => Shape::Wedge,
            "CYLINDER" => Shape::Cylinder,
            "CONE" => Shape::Cone,
            "SPHERE" => Shape::Sphere,
            "TORUS" => Shape::Torus,
            _ => return None,
        })
    }
    fn name(self) -> &'static str {
        match self {
            Shape::Box => "BOX",
            Shape::Wedge => "WEDGE",
            Shape::Cylinder => "CYLINDER",
            Shape::Cone => "CONE",
            Shape::Sphere => "SPHERE",
            Shape::Torus => "TORUS",
        }
    }
    /// True for footprints picked as a centre + radius (round shapes); false
    /// for corner-to-corner footprints (box/wedge).
    fn radial(self) -> bool {
        !matches!(self, Shape::Box | Shape::Wedge)
    }
    /// Whether a height value is collected after the footprint.
    fn needs_height(self) -> bool {
        !matches!(self, Shape::Sphere | Shape::Torus)
    }
}

pub struct PrimitiveCommand {
    shape: Shape,
    /// Footprint points collected so far (local/world XY, z = 0).
    pts: Vec<Vec3>,
    /// True once the footprint is set and we are collecting the height.
    height_step: bool,
}

impl PrimitiveCommand {
    pub fn new(id: &str) -> Self {
        Self {
            shape: Shape::from_id(id).unwrap_or(Shape::Box),
            pts: Vec::new(),
            height_step: false,
        }
    }

    /// Number of footprint points the shape needs before the height step.
    fn footprint_pts(&self) -> usize {
        match self.shape {
            Shape::Torus => 3, // centre, major-radius, minor-radius
            _ => 2,            // corner/corner  or  centre/radius
        }
    }

    /// A reasonable default height when the user just presses Enter.
    fn default_height(&self) -> f64 {
        match self.shape {
            Shape::Box | Shape::Wedge => {
                let d = self.pts[1] - self.pts[0];
                (d.x.abs().max(d.y.abs())) as f64
            }
            _ => (self.pts[1] - self.pts[0]).length() as f64,
        }
        .max(1.0)
    }

    /// Build both the acadrust `Solid3D` (ACIS, for persistence) and the truck
    /// `Solid` B-rep (rendering + booleans) from the footprint + `height`.
    fn build(&self, height: f64) -> Option<(EntityType, Solid)> {
        let (doc, solid) = match self.shape {
            Shape::Box | Shape::Wedge => {
                let (a, b) = (self.pts[0], self.pts[1]);
                let length = (b.x - a.x).abs() as f64;
                let width = (b.y - a.y).abs() as f64;
                if length < 1e-6 || width < 1e-6 || height < 1e-6 {
                    return None;
                }
                if self.shape == Shape::Box {
                    let center = [
                        (a.x + b.x) as f64 / 2.0,
                        (a.y + b.y) as f64 / 2.0,
                        height / 2.0,
                    ];
                    (
                        primitives::build_box(center, length, width, height),
                        model_solid::box_solid(center, length, width, height),
                    )
                } else {
                    let origin = [a.x.min(b.x) as f64, a.y.min(b.y) as f64, 0.0];
                    (
                        primitives::build_wedge(origin, length, width, height),
                        model_solid::wedge_solid(origin, length, width, height),
                    )
                }
            }
            Shape::Cylinder | Shape::Cone => {
                let c = self.pts[0];
                let r = (self.pts[1] - c).length() as f64;
                if r < 1e-6 || height < 1e-6 {
                    return None;
                }
                let center = [c.x as f64, c.y as f64, 0.0];
                if self.shape == Shape::Cylinder {
                    (
                        primitives::build_cylinder(center, r, height),
                        model_solid::cylinder_solid(center, r, height),
                    )
                } else {
                    (
                        primitives::build_cone(center, r, height),
                        model_solid::cone_solid(center, r, height),
                    )
                }
            }
            Shape::Sphere => {
                let c = self.pts[0];
                let r = (self.pts[1] - c).length() as f64;
                if r < 1e-6 {
                    return None;
                }
                let center = [c.x as f64, c.y as f64, 0.0];
                (
                    primitives::build_sphere(center, r),
                    model_solid::sphere_solid(center, r),
                )
            }
            Shape::Torus => {
                let c = self.pts[0];
                let major = (self.pts[1] - c).length() as f64;
                let minor = (self.pts[2] - self.pts[1]).length() as f64;
                if major < 1e-6 || minor < 1e-6 {
                    return None;
                }
                let center = [c.x as f64, c.y as f64, 0.0];
                (
                    primitives::build_torus(center, major, minor),
                    model_solid::torus_solid(center, major, minor),
                )
            }
        };
        let mut s3d = Solid3D::new();
        s3d.set_sat_document(&doc);
        // Edge wires make the solid click-pickable and draw a wireframe over
        // the shaded mesh.
        s3d.wires = model_solid::edge_wires(&solid);
        Some((EntityType::Solid3D(s3d), solid))
    }

    fn commit(&self, height: f64) -> CmdResult {
        match self.build(height) {
            Some((entity, solid)) => CmdResult::CommitSolid {
                entity,
                solid: Box::new(solid),
            },
            None => CmdResult::Cancel,
        }
    }
}

impl CadCommand for PrimitiveCommand {
    fn name(&self) -> &'static str {
        self.shape.name()
    }

    fn prompt(&self) -> String {
        let n = self.shape.name();
        if self.height_step {
            return format!("{n}  Specify height <Enter for default>:");
        }
        match (self.shape.radial(), self.pts.len()) {
            (false, 0) => format!("{n}  Specify first corner:"),
            (false, _) => format!("{n}  Specify opposite corner:"),
            (true, 0) => format!("{n}  Specify center point:"),
            (true, 1) => format!("{n}  Specify radius:"),
            (true, _) => format!("{n}  Specify tube radius:"),
        }
    }

    fn on_point(&mut self, pt: Vec3) -> CmdResult {
        if self.height_step {
            // A click in the ground plane has no Z; use its distance from the
            // footprint centre as the height magnitude.
            let h = (pt - self.pts[0]).length() as f64;
            return self.commit(h.max(1e-6));
        }
        self.pts.push(pt);
        if self.pts.len() < self.footprint_pts() {
            return CmdResult::NeedPoint;
        }
        // Footprint complete.
        if self.shape.needs_height() {
            self.height_step = true;
            CmdResult::NeedPoint
        } else {
            self.commit(0.0)
        }
    }

    fn on_enter(&mut self) -> CmdResult {
        if self.height_step {
            let h = self.default_height();
            return self.commit(h);
        }
        CmdResult::Cancel
    }

    fn on_escape(&mut self) -> CmdResult {
        CmdResult::Cancel
    }

    fn wants_text_input(&self) -> bool {
        self.height_step
    }

    fn on_text_input(&mut self, raw: &str) -> Option<CmdResult> {
        if !self.height_step {
            return None;
        }
        let h: f64 = raw.trim().parse().ok().filter(|v| *v > 0.0)?;
        Some(self.commit(h))
    }

    fn on_mouse_move(&mut self, pt: Vec3) -> Option<WireModel> {
        if self.height_step || self.pts.is_empty() {
            return None;
        }
        let mut foot = self.pts.clone();
        foot.push(pt);
        Some(footprint_wire(self.shape, &foot))
    }
}

// ── Footprint preview ───────────────────────────────────────────────────────

fn footprint_wire(shape: Shape, pts: &[Vec3]) -> WireModel {
    let mut points: Vec<[f32; 3]> = Vec::new();
    if shape.radial() {
        let c = pts[0];
        let r = (pts[1] - c).length();
        circle_points(&mut points, c, r);
        if shape == Shape::Torus && pts.len() >= 3 {
            // outer ring at major + minor for a quick torus hint
            let minor = (pts[2] - pts[1]).length();
            points.push([f32::NAN; 3]);
            circle_points(&mut points, c, r + minor);
        }
    } else {
        let (a, b) = (pts[0], pts[1]);
        points.extend_from_slice(&[
            [a.x, a.y, 0.0],
            [b.x, a.y, 0.0],
            [b.x, b.y, 0.0],
            [a.x, b.y, 0.0],
            [a.x, a.y, 0.0],
        ]);
    }
    wire("primitive_preview", points)
}

fn circle_points(out: &mut Vec<[f32; 3]>, c: Vec3, r: f32) {
    const SEG: usize = 48;
    for i in 0..=SEG {
        let t = i as f32 / SEG as f32 * std::f32::consts::TAU;
        out.push([c.x + r * t.cos(), c.y + r * t.sin(), 0.0]);
    }
}

// ── Autocomplete registry ─────────────────────────────────
inventory::submit!(crate::command::CommandRegistration {
    names: &["BOX", "WEDGE", "CYLINDER", "CONE", "SPHERE", "TORUS"]
});

fn wire(name: &str, points: Vec<[f32; 3]>) -> WireModel {
    WireModel {
        name: name.into(),
        points,
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
