//! Convert a closed triangle mesh into a truck `Solid` of planar triangular faces.
//!
//! This is used to make the geometrically-bounded surface STEP files in `input/`
//! readable by `truck-stepio`: cadrum tessellates the surface model, and the
//! resulting triangle soup is rebuilt as a planar B-rep solid.

use crate::metrics::Mesh;
use crate::ocs::OcsSolid;
use std::collections::HashMap;
use truck_stepio::r#in::alias::{Curve3D, ElementarySurface, Line, Plane, Point3, Surface};
use truck_topology::{Edge, Face, Shell, Solid, Vertex, Wire};

/// Build a truck `Solid` from a triangle mesh.
///
/// Vertices are merged within `tol` and edges are shared between adjacent
/// triangles. The resulting shell is stored as a solid without checking closure,
/// because geometrically-bounded surface models may not form a closed
/// manifold after tessellation.
pub fn solid_from_mesh(mesh: &Mesh, tol: f64) -> Result<OcsSolid, String> {
    let (positions, indices) = merge_mesh_vertices(mesh, tol);

    let vertices: Vec<Vertex<Point3>> = positions.iter().map(|p| Vertex::new(*p)).collect();

    let mut edge_cache: HashMap<(usize, usize), Edge<Point3, Curve3D>> = HashMap::new();
    let mut faces: Vec<Face<Point3, Curve3D, Surface>> =
        Vec::with_capacity(indices.len() / 3);

    for tri in indices.chunks(3) {
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;

        if i0 == i1 || i1 == i2 || i2 == i0 {
            continue;
        }

        let e01 = get_or_create_edge(&mut edge_cache, i0, i1, &vertices);
        let e12 = get_or_create_edge(&mut edge_cache, i1, i2, &vertices);
        let e20 = get_or_create_edge(&mut edge_cache, i2, i0, &vertices);

        let wire = Wire::from(vec![e01, e12, e20]);
        let p0 = positions[i0];
        let p1 = positions[i1];
        let p2 = positions[i2];
        let plane = Plane::new(p0, p1, p2);
        let surface = Surface::ElementarySurface(Box::new(ElementarySurface::Plane(plane)));

        let face = Face::try_new(vec![wire], surface)
            .map_err(|e| format!("invalid triangular face ({i0},{i1},{i2}): {e:?}"))?;
        faces.push(face);
    }

    let shell: Shell<Point3, Curve3D, Surface> = Shell::from(faces);
    Ok(Solid::new_unchecked(vec![shell]))
}

/// Merge mesh positions that are coincident within `tol` and return the
/// deduplicated positions together with remapped triangle indices.
fn merge_mesh_vertices(mesh: &Mesh, tol: f64) -> (Vec<Point3>, Vec<u32>) {
    let mut positions: Vec<Point3> = Vec::new();
    let mut index_map: Vec<usize> = Vec::with_capacity(mesh.positions.len());

    for p in &mesh.positions {
        let p = Point3::new(p[0], p[1], p[2]);
        let mut matched = None;
        for (i, candidate) in positions.iter().enumerate() {
            if (candidate.x - p.x).abs() <= tol
                && (candidate.y - p.y).abs() <= tol
                && (candidate.z - p.z).abs() <= tol
            {
                matched = Some(i);
                break;
            }
        }
        match matched {
            Some(idx) => index_map.push(idx),
            None => {
                index_map.push(positions.len());
                positions.push(p);
            }
        }
    }

    let indices = mesh
        .indices
        .iter()
        .map(|&i| index_map[i as usize] as u32)
        .collect();
    (positions, indices)
}

/// Return a shared edge between `i0` and `i1`, oriented to match the
/// triangle winding. The cache stores edges in canonical (low, high) order.
fn get_or_create_edge(
    cache: &mut HashMap<(usize, usize), Edge<Point3, Curve3D>>,
    i0: usize,
    i1: usize,
    vertices: &[Vertex<Point3>],
) -> Edge<Point3, Curve3D> {
    let key = (i0.min(i1), i0.max(i1));
    if let Some(edge) = cache.get(&key) {
        return if i0 < i1 {
            edge.clone()
        } else {
            edge.inverse()
        };
    }

    let v0 = &vertices[key.0];
    let v1 = &vertices[key.1];
    let line = Curve3D::Line(Line(v0.point(), v1.point()));
    let edge = Edge::new(v0, v1, line);
    cache.insert(key, edge.clone());

    if i0 < i1 {
        edge.clone()
    } else {
        edge.inverse()
    }
}
