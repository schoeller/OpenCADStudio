// B-spline (NURBS) surface tessellation for ACIS `spline-surface` faces.
//
// Lofted / swept / revolved surfaces store their geometry as an ACIS
// `nubs` (non-uniform B-spline) block inside the `spline-surface` record.
// Rather than evaluate the basis functions by hand, we parse the control net
// and knot vectors out of the SAT tokens, hand them to truck's
// `BSplineSurface` (the same NURBS kernel the Model tab already builds on),
// and sample its parametric grid into triangles.

use acadrust::entities::acis::{SatDocument, SatFace, SatRecord, SatToken};
use truck_modeling::{BSplineSurface, KnotVec, ParametricSurface, ParametricSurface3D, Point3};

use crate::scene::convert::solid3d_tess::LodConfig;

/// Tessellate one `spline-surface` face by sampling its B-spline surface.
/// Appends triangles to the shared mesh buffers; a no-op when the surface
/// record can't be parsed into a B-spline.
pub fn tess_spline_face(
    sat: &SatDocument,
    face: &SatFace,
    lod: LodConfig,
    verts: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    let Some(surf_rec) = sat.resolve(face.surface()) else {
        return;
    };
    let Some(surface) = build_bspline_surface(surf_rec) else {
        return;
    };

    // Sample over the knot domain. The B-spline patches stored by loft/sweep
    // are already trimmed to the face, so the full parametric rectangle is the
    // visible surface — no separate boundary trim needed.
    let (u_range, v_range) = surface.parameter_range();
    let (u0, u1) = range_bounds(u_range);
    let (v0, v1) = range_bounds(v_range);
    if !(u1 > u0) || !(v1 > v0) {
        return;
    }

    let su = lod.grid_u.max(8);
    let sv = lod.grid_v.max(8);

    let base = verts.len() as u32;
    for j in 0..=sv {
        let v = v0 + (v1 - v0) * (j as f64 / sv as f64);
        for i in 0..=su {
            let u = u0 + (u1 - u0) * (i as f64 / su as f64);
            let p = surface.subs(u, v);
            let n = surface.normal(u, v);
            verts.push([p.x as f32, p.y as f32, p.z as f32]);
            normals.push([n.x as f32, n.y as f32, n.z as f32]);
        }
    }

    let row = (su + 1) as u32;
    for j in 0..sv as u32 {
        for i in 0..su as u32 {
            let a = base + j * row + i;
            let b = a + 1;
            let c = a + row;
            let d = c + 1;
            indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }
}

/// Extract the inclusive `[start, end]` bounds from a truck parameter range.
fn range_bounds(r: (std::ops::Bound<f64>, std::ops::Bound<f64>)) -> (f64, f64) {
    use std::ops::Bound::*;
    let lo = match r.0 {
        Included(v) | Excluded(v) => v,
        Unbounded => 0.0,
    };
    let hi = match r.1 {
        Included(v) | Excluded(v) => v,
        Unbounded => 1.0,
    };
    (lo, hi)
}

/// Parse the `nubs` control net + knot vectors out of a `spline-surface`
/// record's token stream into a truck `BSplineSurface`.
fn build_bspline_surface(rec: &SatRecord) -> Option<BSplineSurface<Point3>> {
    let toks = &rec.tokens;
    // Locate the real B-spline block. `nullbs` placeholders (for absent
    // rail/path surfaces) precede it; the actual surface is `nubs`.
    let start = toks.iter().position(|t| {
        matches!(t, SatToken::Ident(s) if s == "nubs" || s == "nurbs")
    })?;

    let mut p = start + 1;
    let deg_u = read_int(toks, &mut p)? as usize;
    let deg_v = read_int(toks, &mut p)? as usize;
    // Four form flags (closure / singularity in u and v) — skip.
    for _ in 0..4 {
        read_int(toks, &mut p)?;
    }
    let n_uknot = read_int(toks, &mut p)? as usize;
    let n_vknot = read_int(toks, &mut p)? as usize;

    let u_knots = read_knot_vec(toks, &mut p, n_uknot, deg_u)?;
    let v_knots = read_knot_vec(toks, &mut p, n_vknot, deg_v)?;

    let n_ctrl_u = u_knots.len().checked_sub(deg_u + 1)?;
    let n_ctrl_v = v_knots.len().checked_sub(deg_v + 1)?;
    if n_ctrl_u == 0 || n_ctrl_v == 0 {
        return None;
    }

    // Control points are stored row-major with u varying fastest (a full row
    // of u control points per v step). truck wants `ctrl[i_u][j_v]`.
    let total = n_ctrl_u * n_ctrl_v;
    let mut flat: Vec<Point3> = Vec::with_capacity(total);
    for _ in 0..total {
        let x = read_float(toks, &mut p)?;
        let y = read_float(toks, &mut p)?;
        let z = read_float(toks, &mut p)?;
        flat.push(Point3::new(x, y, z));
    }
    let mut ctrl = vec![Vec::with_capacity(n_ctrl_v); n_ctrl_u];
    for v in 0..n_ctrl_v {
        for u in 0..n_ctrl_u {
            ctrl[u].push(flat[v * n_ctrl_u + u]);
        }
    }

    let uk = KnotVec::from(u_knots);
    let vk = KnotVec::from(v_knots);
    BSplineSurface::try_new((uk, vk), ctrl).ok()
}

/// Read `count` `(knot value, multiplicity)` pairs into an expanded knot
/// vector. ACIS stores the end knots with multiplicity = degree; a clamped
/// B-spline needs degree + 1, so the first and last multiplicities are bumped
/// by one.
fn read_knot_vec(
    toks: &[SatToken],
    p: &mut usize,
    count: usize,
    degree: usize,
) -> Option<Vec<f64>> {
    let mut knots: Vec<f64> = Vec::new();
    for i in 0..count {
        let value = read_float(toks, p)?;
        let mut mult = read_int(toks, p)? as usize;
        if i == 0 || i == count - 1 {
            mult += 1;
        }
        let _ = degree;
        for _ in 0..mult {
            knots.push(value);
        }
    }
    if knots.len() < 2 {
        return None;
    }
    Some(knots)
}

fn read_int(toks: &[SatToken], p: &mut usize) -> Option<i64> {
    while *p < toks.len() {
        let t = &toks[*p];
        *p += 1;
        match t {
            SatToken::Integer(v) => return Some(*v),
            SatToken::Float(v) => return Some(*v as i64),
            // Skip block delimiters / idents that may appear inline.
            SatToken::Ident(_) | SatToken::Enum(_) => continue,
            _ => return None,
        }
    }
    None
}

fn read_float(toks: &[SatToken], p: &mut usize) -> Option<f64> {
    while *p < toks.len() {
        let t = &toks[*p];
        *p += 1;
        match t {
            SatToken::Float(v) => return Some(*v),
            SatToken::Integer(v) => return Some(*v as f64),
            SatToken::Ident(_) | SatToken::Enum(_) => continue,
            _ => return None,
        }
    }
    None
}
