// ACIS B-rep → truck B-rep → mesh.
//
// Rebuilds every ACIS face as a truck topological `Face` and lets truck's
// meshing kernel triangulate it, routing loaded 3DSOLID / BODY / REGION /
// SURFACE geometry through the same NURBS/B-rep kernel the Model tab builds on
// instead of the bespoke per-surface sampler in `solid3d_tess`.
//
//   plane-surface   → planar face from the sampled boundary loop
//   cone-surface    → surface of revolution (cylinder / cone) via rsweep
//   sphere-surface  → meridian revolved about the pole
//   torus-surface   → tube circle revolved about the spine
//   spline-surface  → truck BSplineSurface, grid-sampled (see spline_tess)
//
// Each face is meshed independently and its triangles are oriented outward
// using an analytic per-surface normal — truck's own face orientation is not
// consistent across independently built faces, so normals/winding are derived
// from geometry instead. Faces whose surface type isn't handled are skipped;
// the caller falls back to `solid3d_tess` when this returns `None`.

use truck_meshalgo::tessellation::{MeshableShape, MeshedShape};
use truck_modeling::{builder, Face, InnerSpace, Point3, Rad, Shell, Vector3, Wire};
use truck_polymesh::PolygonMesh;

use acadrust::entities::acis::types::Sense;
use acadrust::entities::acis::{
    SatConeSurface, SatDocument, SatFace, SatPlaneSurface, SatSphereSurface, SatTorusSurface,
};

use crate::scene::model::mesh_model::{MeshLodSet, MeshModel};
use crate::scene::convert::solid3d_tess::{
    apply_body_transform, body_transform, collect_face_polygon, cone_axis_span, mesh_aabb,
};

/// Slightly over 2π so revolution builders close the loop.
const FULL: f64 = std::f64::consts::TAU + 0.2;
/// Boundary sampling density for planar faces with curved (circular) edges.
const BOUNDARY_SEGS: usize = 64;
/// Triangle mesh chord tolerance (world units).
const MESH_TOL: f64 = 0.01;

/// Analytic outward-normal rule for a face, used to orient triangles and
/// supply smooth per-vertex normals (truck's face orientation is unreliable
/// for independently built faces).
enum Outward {
    /// Constant normal (planar face).
    Const([f64; 3]),
    /// Cone/cylinder: radial away from the axis, tilted by the half-angle.
    Cone {
        center: [f64; 3],
        axis: [f64; 3],
        sin: f64,
        cos: f64,
    },
    /// Sphere: radial from the centre.
    Sphere { center: [f64; 3] },
    /// Torus: from the nearest point on the spine circle.
    Torus {
        center: [f64; 3],
        axis: [f64; 3],
        major: f64,
    },
}

impl Outward {
    fn at(&self, p: [f64; 3]) -> [f64; 3] {
        match self {
            Outward::Const(n) => *n,
            Outward::Cone {
                center,
                axis,
                sin,
                cos,
            } => {
                let d = vsub(p, *center);
                let h = vdot(d, *axis);
                let radial = vnorm(vsub(d, vscale(*axis, h)));
                vnorm(vsub(vscale(radial, *cos), vscale(*axis, *sin)))
            }
            Outward::Sphere { center } => vnorm(vsub(p, *center)),
            Outward::Torus {
                center,
                axis,
                major,
            } => {
                let d = vsub(p, *center);
                let h = vdot(d, *axis);
                let radial = vsub(d, vscale(*axis, h));
                let rl = vlen(radial);
                let spine = if rl > 1e-9 {
                    vadd(*center, vscale(radial, major / rl))
                } else {
                    *center
                };
                vnorm(vsub(p, spine))
            }
        }
    }
}

/// Tessellate an ACIS SAT document by rebuilding it as truck faces.
///
/// Returns `None` when no face could be rebuilt (caller should fall back to
/// the bespoke sampler).
pub fn tessellate_sat_truck(
    sat: &SatDocument,
    name: String,
    color: [f32; 4],
    _facet_res: f64,
) -> Option<MeshLodSet> {
    let mut mesh = MeshModel {
        name: name.clone(),
        verts: Vec::new(),
        normals: Vec::new(),
        indices: Vec::new(),
        color,
        selected: false,
    };

    for face in sat.faces() {
        let Some(surf_rec) = sat.resolve(face.surface()) else {
            continue;
        };
        let Some((faces, outward)) = build_face_group(sat, &face, surf_rec) else {
            continue;
        };
        if faces.is_empty() {
            continue;
        }
        let shell: Shell = faces.into();
        let poly = shell.triangulation(MESH_TOL).to_polygon();
        append_group(&mut mesh, &poly, &outward);
    }

    // Spline (NURBS) faces are meshed by direct grid sampling of the truck
    // BSplineSurface — see spline_tess — and merged into the same buffers.
    append_spline_faces(sat, &mut mesh);

    if mesh.indices.is_empty() {
        return None;
    }

    if let Some((m, tr, scale)) = body_transform(sat) {
        apply_body_transform(&mut mesh, &m, &tr, scale);
    }
    let world_aabb = mesh_aabb(&mesh);
    Some(MeshLodSet {
        lods: vec![mesh],
        world_aabb,
    })
}

/// Build the truck face(s) + outward rule for one analytic ACIS face.
fn build_face_group(
    sat: &SatDocument,
    face: &SatFace,
    surf_rec: &acadrust::entities::acis::SatRecord,
) -> Option<(Vec<Face>, Outward)> {
    match surf_rec.entity_type.as_str() {
        "plane-surface" => {
            let plane = SatPlaneSurface::from_record(surf_rec)?;
            let f = plane_face(sat, face)?;
            let (nx, ny, nz) = plane.normal();
            let n = if matches!(face.sense(), Sense::Reversed) {
                [-nx, -ny, -nz]
            } else {
                [nx, ny, nz]
            };
            Some((vec![f], Outward::Const(vnorm(n))))
        }
        "cone-surface" => {
            let cone = SatConeSurface::from_record(surf_rec)?;
            let (faces, out) = cone_faces(sat, &cone)?;
            Some((faces, out))
        }
        "sphere-surface" => {
            let sphere = SatSphereSurface::from_record(surf_rec)?;
            let (cx, cy, cz) = sphere.center();
            Some((sphere_faces(&sphere), Outward::Sphere { center: [cx, cy, cz] }))
        }
        "torus-surface" => {
            let torus = SatTorusSurface::from_record(surf_rec)?;
            let (cx, cy, cz) = torus.center();
            let (nx, ny, nz) = torus.normal();
            let out = Outward::Torus {
                center: [cx, cy, cz],
                axis: vnorm([nx, ny, nz]),
                major: torus.major_radius(),
            };
            Some((torus_faces(&torus), out))
        }
        _ => None,
    }
}

// ── Planar face ────────────────────────────────────────────────────────────

/// Build a planar truck face from a face's sampled boundary loop. Curved
/// boundary edges (circles) are sampled into line segments, which keeps the
/// wire planar so `try_attach_plane` can fit the plane.
fn plane_face(sat: &SatDocument, face: &SatFace) -> Option<Face> {
    let poly = collect_face_polygon(sat, face, BOUNDARY_SEGS);
    if poly.len() < 3 {
        return None;
    }
    let verts: Vec<_> = poly
        .iter()
        .map(|p| builder::vertex(Point3::new(p[0], p[1], p[2])))
        .collect();
    let n = verts.len();
    let edges: Vec<_> = (0..n)
        .map(|i| builder::line(&verts[i], &verts[(i + 1) % n]))
        .collect();
    let wire: Wire = edges.into();
    builder::try_attach_plane(&[wire]).ok()
}

// ── Cone / cylinder face ─────────────────────────────────────────────────────

/// Build the lateral surface of a cone/cylinder as revolution faces. The
/// height span comes from the solid's coaxial rims (plus a true cone's apex).
fn cone_faces(sat: &SatDocument, cone: &SatConeSurface) -> Option<(Vec<Face>, Outward)> {
    let (cx, cy, cz) = cone.center();
    let (ax, ay, az) = cone.axis();
    let (ux, uy, uz) = cone.major_axis();
    let radius = cone.radius();
    let sin = cone.sin_half_angle();
    let cos = cone.cos_half_angle();

    let axis = norm(Vector3::new(ax, ay, az));
    let udir = norm(Vector3::new(ux, uy, uz));
    let center = Point3::new(cx, cy, cz);

    let (hmin, hmax) = cone_axis_span(sat, cone, [axis.x, axis.y, axis.z], [cx, cy, cz])?;
    let r_at = |h: f64| {
        if cos.abs() > 1e-9 {
            radius + h * sin / cos
        } else {
            radius
        }
    };

    let p0 = center + udir * r_at(hmin) + axis * hmin;
    let p1 = center + udir * r_at(hmax) + axis * hmax;
    let profile = builder::line(&builder::vertex(p0), &builder::vertex(p1));
    let shell: Shell = builder::rsweep(&profile, center, axis, Rad(FULL));

    let out = Outward::Cone {
        center: [cx, cy, cz],
        axis: [axis.x, axis.y, axis.z],
        sin,
        cos,
    };
    Some((shell.face_iter().cloned().collect(), out))
}

// ── Sphere face ──────────────────────────────────────────────────────────────

fn sphere_faces(sphere: &SatSphereSurface) -> Vec<Face> {
    let (cx, cy, cz) = sphere.center();
    let r = sphere.radius();
    let (px, py, pz) = sphere.pole();
    let (ux, uy, uz) = sphere.u_direction();

    let center = Point3::new(cx, cy, cz);
    let pole = norm(Vector3::new(px, py, pz));
    let mut perp = Vector3::new(ux, uy, uz);
    if perp.magnitude2() < 1e-12 || perp.dot(pole).abs() > 0.999 {
        perp = perpendicular(pole);
    } else {
        perp = norm(perp - pole * perp.dot(pole));
    }

    let top = builder::vertex(center + pole * r);
    let meridian: Wire = builder::rsweep(&top, center, perp, Rad(std::f64::consts::PI));
    let shell: Shell = builder::cone(&meridian, pole, Rad(FULL));
    shell.face_iter().cloned().collect()
}

// ── Torus face ───────────────────────────────────────────────────────────────

fn torus_faces(torus: &SatTorusSurface) -> Vec<Face> {
    let (cx, cy, cz) = torus.center();
    let (nx, ny, nz) = torus.normal();
    let (ux, uy, uz) = torus.u_direction();
    let major = torus.major_radius();
    let minor = torus.minor_radius();

    let center = Point3::new(cx, cy, cz);
    let axis = norm(Vector3::new(nx, ny, nz));
    let udir = norm(Vector3::new(ux, uy, uz));
    let binormal = norm(axis.cross(udir));

    let ring_center = center + udir * major;
    let tube_start = builder::vertex(ring_center + axis * minor);
    let tube: Wire = builder::rsweep(&tube_start, ring_center, binormal, Rad(FULL));
    let shell: Shell = builder::rsweep(&tube, center, axis, Rad(FULL));
    shell.face_iter().cloned().collect()
}

// ── Spline faces (NURBS) ─────────────────────────────────────────────────────

/// Append meshes for every `spline-surface` face, reusing the truck
/// BSplineSurface grid sampler in `spline_tess`.
fn append_spline_faces(sat: &SatDocument, mesh: &mut MeshModel) {
    use crate::scene::convert::solid3d_tess::LodConfig;
    for face in sat.faces() {
        let Some(surf_rec) = sat.resolve(face.surface()) else {
            continue;
        };
        if surf_rec.entity_type != "spline-surface" {
            continue;
        }
        let mut verts: Vec<[f32; 3]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        crate::scene::convert::spline_tess::tess_spline_face(
            sat,
            &face,
            LodConfig::HIGH,
            &mut verts,
            &mut normals,
            &mut indices,
        );
        let base = mesh.verts.len() as u32;
        mesh.verts.extend_from_slice(&verts);
        mesh.normals.extend_from_slice(&normals);
        mesh.indices.extend(indices.iter().map(|i| i + base));
    }
}

// ── Mesh append with analytic outward normals ────────────────────────────────

/// Append one face's triangulation to `mesh`, computing smooth per-vertex
/// normals from the outward rule and flipping any triangle whose winding
/// disagrees with that outward direction.
fn append_group(mesh: &mut MeshModel, poly: &PolygonMesh, outward: &Outward) {
    let positions = poly.positions();
    let base = mesh.verts.len() as u32;
    for p in positions {
        let pos = [p.x, p.y, p.z];
        mesh.verts.push([p.x as f32, p.y as f32, p.z as f32]);
        let n = outward.at(pos);
        mesh.normals.push([n[0] as f32, n[1] as f32, n[2] as f32]);
    }
    for tri in poly.tri_faces() {
        let (i0, i1, i2) = (tri[0].pos, tri[1].pos, tri[2].pos);
        let a = pt(positions[i0]);
        let b = pt(positions[i1]);
        let c = pt(positions[i2]);
        let gn = vcross(vsub(b, a), vsub(c, a));
        let cen = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        let out = outward.at(cen);
        let (j0, j1, j2) = (base + i0 as u32, base + i1 as u32, base + i2 as u32);
        if vdot(gn, out) < 0.0 {
            mesh.indices.extend_from_slice(&[j0, j2, j1]);
        } else {
            mesh.indices.extend_from_slice(&[j0, j1, j2]);
        }
    }
}

#[inline]
fn pt(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

// ── Vector helpers (cgmath Vector3) ──────────────────────────────────────────

#[inline]
fn norm(v: Vector3) -> Vector3 {
    let len = v.magnitude();
    if len < 1e-12 {
        Vector3::unit_z()
    } else {
        v / len
    }
}

/// Any unit vector perpendicular to `v`.
fn perpendicular(v: Vector3) -> Vector3 {
    let a = if v.x.abs() < 0.9 {
        Vector3::unit_x()
    } else {
        Vector3::unit_y()
    };
    norm(v.cross(a))
}

// ── Vector helpers ([f64; 3]) ────────────────────────────────────────────────

#[inline]
fn vsub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn vadd(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn vscale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
fn vdot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn vcross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
fn vlen(a: [f64; 3]) -> f64 {
    vdot(a, a).sqrt()
}
#[inline]
fn vnorm(a: [f64; 3]) -> [f64; 3] {
    let l = vlen(a);
    if l < 1e-12 {
        [0.0, 0.0, 1.0]
    } else {
        [a[0] / l, a[1] / l, a[2] / l]
    }
}
