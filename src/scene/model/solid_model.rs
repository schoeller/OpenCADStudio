// truck B-rep construction for the Model tab's primitives, plus tessellation
// into the renderer's `MeshLodSet`. Each builder follows truck's own example
// recipes (tsweep / rsweep / cone / try_attach_plane), oriented Z-up with the
// footprint on the z = `base_z` plane to match acadrust's `acis::primitives`.
//
// The resulting truck `Solid` is cached per entity handle on the Scene so the
// Design-group boolean tools can run truck-shapeops on it.

use truck_modeling::{builder, Point3, Rad, Solid, Vector3, Wire};

use crate::scene::model::mesh_model::{MeshLodSet, MeshModel};

const FULL: f64 = 7.0; // > 2π → builder closes the revolution

/// Axis-aligned box from its center and full extents.
pub fn box_solid(center: [f64; 3], length: f64, width: f64, height: f64) -> Solid {
    let min = Point3::new(
        center[0] - length / 2.0,
        center[1] - width / 2.0,
        center[2] - height / 2.0,
    );
    let v = builder::vertex(min);
    let e = builder::tsweep(&v, Vector3::new(length, 0.0, 0.0));
    let f = builder::tsweep(&e, Vector3::new(0.0, width, 0.0));
    builder::tsweep(&f, Vector3::new(0.0, 0.0, height))
}

/// Right triangular prism (wedge): right-triangle cross-section in XZ,
/// extruded along Y. `origin` is the min corner, ramp rising in +X/+Z.
pub fn wedge_solid(origin: [f64; 3], length: f64, width: f64, height: f64) -> Solid {
    let o = Point3::new(origin[0], origin[1], origin[2]);
    let a = builder::vertex(o);
    let b = builder::vertex(Point3::new(o.x + length, o.y, o.z));
    let c = builder::vertex(Point3::new(o.x, o.y, o.z + height));
    let wire: Wire = vec![
        builder::line(&a, &b),
        builder::line(&b, &c),
        builder::line(&c, &a),
    ]
    .into();
    let face = builder::try_attach_plane(&[wire]).expect("wedge profile");
    builder::tsweep(&face, Vector3::new(0.0, width, 0.0))
}

/// Solid cylinder: disk on the z = base plane, extruded up by `height`.
pub fn cylinder_solid(center: [f64; 3], radius: f64, height: f64) -> Solid {
    let v = builder::vertex(Point3::new(center[0] + radius, center[1], center[2]));
    let circle = builder::rsweep(
        &v,
        Point3::new(center[0], center[1], center[2]),
        Vector3::unit_z(),
        Rad(FULL),
    );
    let disk = builder::try_attach_plane(&[circle]).expect("cylinder cap");
    builder::tsweep(&disk, Vector3::new(0.0, 0.0, height))
}

/// Solid cone: profile (apex → rim → base-center) revolved about the axis.
pub fn cone_solid(center: [f64; 3], radius: f64, height: f64) -> Solid {
    let cx = center[0];
    let cy = center[1];
    let cz = center[2];
    let apex = builder::vertex(Point3::new(cx, cy, cz + height));
    let rim = builder::vertex(Point3::new(cx + radius, cy, cz));
    let base = builder::vertex(Point3::new(cx, cy, cz));
    let wire: Wire = vec![builder::line(&apex, &rim), builder::line(&rim, &base)].into();
    let shell = builder::cone(&wire, Vector3::unit_z(), Rad(FULL));
    Solid::new(vec![shell])
}

/// Solid sphere: meridian semicircle revolved about the polar (Z) axis.
pub fn sphere_solid(center: [f64; 3], radius: f64) -> Solid {
    let c = Point3::new(center[0], center[1], center[2]);
    let top = builder::vertex(Point3::new(c.x, c.y, c.z + radius));
    // Rotate the top point about the X axis by π → meridian from +Z to −Z.
    let meridian: Wire = builder::rsweep(&top, c, Vector3::unit_x(), Rad(std::f64::consts::PI));
    let shell = builder::cone(&meridian, Vector3::unit_z(), Rad(FULL));
    Solid::new(vec![shell])
}

/// Solid torus in the z = base plane (tube revolved about the Z axis).
pub fn torus_solid(center: [f64; 3], major: f64, minor: f64) -> Solid {
    let c = Point3::new(center[0], center[1], center[2]);
    let v = builder::vertex(Point3::new(c.x + major, c.y, c.z + minor));
    let tube = builder::rsweep(
        &v,
        Point3::new(c.x + major, c.y, c.z),
        Vector3::unit_y(),
        Rad(FULL),
    );
    let shell = builder::rsweep(&tube, c, Vector3::unit_z(), Rad(FULL));
    Solid::new(vec![shell])
}

// ── Edge extraction (pick geometry + wireframe overlay) ─────────────────────

/// Tessellate the solid's B-rep edges into acadrust `Wire`s. Stored on the
/// `Solid3D`/result entity so it is click-pickable (the renderer's wire
/// fallback draws these as a wireframe over the shaded mesh, and hit-testing
/// uses their points).
pub fn edge_wires(solid: &Solid) -> Vec<acadrust::entities::Wire> {
    use crate::scene::convert::truck_tess::{tessellate_edge, TruckTessResult};
    use acadrust::types::Vector3;
    let mut wires = Vec::new();
    for shell in solid.boundaries() {
        for edge in shell.edge_iter() {
            if let TruckTessResult::Lines(pts, pts_low) =
                tessellate_edge(&edge)
            {
                if pts.len() < 2 {
                    continue;
                }
                let pts3: Vec<Vector3> = pts
                    .iter()
                    .zip(pts_low.iter())
                    .map(|(p, l)| {
                        Vector3::new(
                            p[0] as f64 + l[0] as f64,
                            p[1] as f64 + l[1] as f64,
                            p[2] as f64 + l[2] as f64,
                        )
                    })
                    .collect();
                wires.push(acadrust::entities::Wire::from_points(pts3));
            }
        }
    }
    wires
}

// ── Boolean operations (truck-shapeops) ─────────────────────────────────────

/// Which CSG to apply. Mirrors `model::boolean_cmd::BoolOp` but kept local so
/// this scene module has no dependency on the UI module.
#[derive(Clone, Copy)]
pub enum Bool {
    Union,
    Subtract,
    Intersect,
}

#[cfg(feature = "solid3d")]
const BOOL_TOL: f64 = 0.05;

/// Combine two solids. `Subtract` removes `b` from `a`. Returns `None` when the
/// operation fails (e.g. the solids don't actually overlap).
#[cfg(feature = "solid3d")]
pub fn boolean(op: Bool, a: &Solid, b: &Solid) -> Option<Solid> {
    match op {
        Bool::Union => truck_shapeops::or(a, b, BOOL_TOL),
        Bool::Intersect => truck_shapeops::and(a, b, BOOL_TOL),
        Bool::Subtract => {
            let mut bn = b.clone();
            bn.not();
            truck_shapeops::and(a, &bn, BOOL_TOL)
        }
    }
}

/// Without `solid3d` (e.g. wasm) there is no boolean kernel.
#[cfg(not(feature = "solid3d"))]
pub fn boolean(_op: Bool, _a: &Solid, _b: &Solid) -> Option<Solid> {
    None
}

// ── Tessellation ────────────────────────────────────────────────────────────

/// Tessellate a truck `Solid` into a single-LOD `MeshLodSet` (world-space,
/// before world_offset is applied by the caller).
pub fn mesh_from_solid(solid: &Solid, color: [f32; 4]) -> Option<MeshLodSet> {
    use crate::scene::convert::truck_tess::{tessellate_solid, TruckTessResult};
    match tessellate_solid(solid) {
        TruckTessResult::Mesh {
            verts,
            verts_low,
            normals,
            indices,
        } if !indices.is_empty() => {
            let mesh = MeshModel {
                name: String::new(),
                verts,
                verts_low,
                normals,
                indices,
                color,
                selected: false,
            };
            Some(MeshLodSet::from_single(mesh))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_count(s: &Solid) -> usize {
        mesh_from_solid(s, [0.7, 0.7, 0.7, 1.0])
            .map(|m| m.lods[0].indices.len() / 3)
            .unwrap_or(0)
    }

    #[test]
    fn all_primitives_triangulate() {
        let c = [0.0, 0.0, 0.0];
        assert!(tri_count(&box_solid(c, 10.0, 10.0, 10.0)) >= 12, "box");
        assert!(tri_count(&wedge_solid(c, 10.0, 10.0, 10.0)) >= 6, "wedge");
        assert!(tri_count(&cylinder_solid(c, 5.0, 12.0)) > 20, "cylinder");
        assert!(tri_count(&cone_solid(c, 5.0, 12.0)) > 10, "cone");
        assert!(tri_count(&sphere_solid(c, 5.0)) > 50, "sphere");
        assert!(tri_count(&torus_solid(c, 8.0, 2.0)) > 50, "torus");
    }

    #[test]
    fn booleans_produce_solids() {
        let a = box_solid([0.0, 0.0, 0.0], 10.0, 10.0, 10.0);
        let b = box_solid([5.0, 5.0, 5.0], 10.0, 10.0, 10.0);
        for (op, label) in [
            (Bool::Union, "union"),
            (Bool::Subtract, "subtract"),
            (Bool::Intersect, "intersect"),
        ] {
            let r = boolean(op, &a, &b);
            let n = r.as_ref().map(tri_count).unwrap_or(0);
            eprintln!("{label}: tris={n}");
            assert!(r.is_some() && n > 0, "{label} produced nothing");
        }
    }

    #[test]
    fn box_exposes_edges() {
        assert!(edge_wires(&box_solid([0.0, 0.0, 0.0], 10.0, 10.0, 10.0)).len() >= 12);
    }
}
